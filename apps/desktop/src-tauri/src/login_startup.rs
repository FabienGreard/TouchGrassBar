//! Read macOS authorization for the LaunchAgent written by the autostart plugin.
//! A plist on disk records intent; it does not prove macOS permits startup.

#[cfg(target_os = "macos")]
use crate::lifecycle::LaunchAtLoginState;

/// Link the plugin's existing entry to the signed app. Keep this on every enable:
/// the plugin replaces the plist and does not write AssociatedBundleIdentifiers.
/// Apple documents this key under "Connect services to app names in System Settings".
#[cfg(target_os = "macos")]
pub(crate) fn associate_legacy_entry(
    path: &std::path::Path,
    label: &str,
    executable: &std::path::Path,
    bundle_identifier: &str,
) -> Result<bool, ()> {
    use plist::Value;
    use std::fs;

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(()),
    };
    if !metadata.is_file() {
        return Err(());
    }
    let mut value = Value::from_file(path).map_err(|_| ())?;
    let entry = value.as_dictionary_mut().ok_or(())?;
    let program = entry
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .and_then(|arguments| arguments.first())
        .and_then(Value::as_string);
    if entry.get("Label").and_then(Value::as_string) != Some(label)
        || program.map(std::path::Path::new) != Some(executable)
        || bundle_identifier.is_empty()
    {
        return Err(());
    }
    if entry
        .get("AssociatedBundleIdentifiers")
        .and_then(Value::as_string)
        == Some(bundle_identifier)
    {
        return Ok(false);
    }
    entry.insert(
        "AssociatedBundleIdentifiers".into(),
        Value::String(bundle_identifier.into()),
    );
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().ok_or(())?).map_err(|_| ())?;
    value.to_writer_xml(&mut temporary).map_err(|_| ())?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())
        .map_err(|_| ())?;
    temporary.as_file().sync_all().map_err(|_| ())?;
    temporary.persist(path).map_err(|_| ())?;
    Ok(true)
}

#[cfg(target_os = "macos")]
pub(crate) fn legacy_state(path: &std::path::Path) -> LaunchAtLoginState {
    use objc2_foundation::{NSString, NSURL};
    use objc2_service_management::SMAppService;

    let Some(path) = path.to_str() else {
        return LaunchAtLoginState::Unavailable;
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // The app requires macOS 15. This read-only API is available from macOS 13.
    state_from_status(unsafe { SMAppService::statusForLegacyURL(&url) })
}

#[cfg(target_os = "macos")]
fn state_from_status(status: objc2_service_management::SMAppServiceStatus) -> LaunchAtLoginState {
    use objc2_service_management::SMAppServiceStatus;

    match status {
        SMAppServiceStatus::Enabled => LaunchAtLoginState::Available { enabled: true },
        SMAppServiceStatus::RequiresApproval => LaunchAtLoginState::RequiresApproval,
        // A newly written or missing service must not be reported as enabled.
        _ => LaunchAtLoginState::Unavailable,
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn open_system_settings() {
    // Called on the main thread after the user selects the Settings action.
    unsafe { objc2_service_management::SMAppService::openSystemSettingsLoginItems() };
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2_service_management::SMAppServiceStatus;

    const EXECUTABLE: &str = "/Applications/TouchGrassBar.app/Contents/MacOS/touchgrassbar";
    const BUNDLE: &str = "app.touchgrass.bar";

    fn legacy_fixture(path: &std::path::Path) {
        let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
            ("Label", plist::Value::String("TouchGrassBar".into())),
            (
                "ProgramArguments",
                plist::Value::Array(vec![
                    plist::Value::String(EXECUTABLE.into()),
                    plist::Value::String("--background".into()),
                ]),
            ),
            ("RunAtLoad", plist::Value::Boolean(true)),
        ]));
        value.to_file_xml(path).unwrap();
    }

    fn associate(path: &std::path::Path) -> Result<bool, ()> {
        associate_legacy_entry(
            path,
            "TouchGrassBar",
            std::path::Path::new(EXECUTABLE),
            BUNDLE,
        )
    }

    #[test]
    fn associates_existing_entry_without_changing_startup_behavior() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("TouchGrassBar.plist");
        legacy_fixture(&path);
        let before = plist::Value::from_file(&path).unwrap();
        assert_eq!(associate(&path), Ok(true));
        let mut after = plist::Value::from_file(&path).unwrap();
        assert_eq!(
            after
                .as_dictionary_mut()
                .unwrap()
                .remove("AssociatedBundleIdentifiers"),
            Some(plist::Value::String(BUNDLE.into()))
        );
        assert_eq!(after, before);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(associate(&path), Ok(false));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        // The autostart plugin rewrites the entry after the user enables it again.
        legacy_fixture(&path);
        assert_eq!(associate(&path), Ok(true));
    }

    #[test]
    fn leaves_disabled_startup_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("TouchGrassBar.plist");
        assert_eq!(associate(&path), Ok(false));
        assert!(!path.exists());
    }

    #[test]
    fn rejects_entries_for_another_label_or_executable_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("TouchGrassBar.plist");
        legacy_fixture(&path);
        let bytes = std::fs::read(&path).unwrap();
        for (label, executable) in [
            ("OtherApp", EXECUTABLE),
            ("TouchGrassBar", "/tmp/other-app"),
        ] {
            assert_eq!(
                associate_legacy_entry(&path, label, std::path::Path::new(executable), BUNDLE),
                Err(())
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn rejects_malformed_or_linked_entries_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("TouchGrassBar.plist");
        std::fs::write(&path, b"invalid plist").unwrap();
        assert_eq!(associate(&path), Err(()));
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid plist");
        let link = directory.path().join("linked.plist");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(associate(&link), Err(()));
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid plist");
    }

    #[test]
    fn reports_os_authorization_instead_of_file_presence() {
        for (status, expected) in [
            (
                SMAppServiceStatus::Enabled,
                r#"{"availability":"available","enabled":true}"#,
            ),
            (
                SMAppServiceStatus::RequiresApproval,
                r#"{"availability":"requiresApproval"}"#,
            ),
            (
                SMAppServiceStatus::NotRegistered,
                r#"{"availability":"unavailable"}"#,
            ),
            (
                SMAppServiceStatus::NotFound,
                r#"{"availability":"unavailable"}"#,
            ),
            (SMAppServiceStatus(99), r#"{"availability":"unavailable"}"#),
        ] {
            assert_eq!(
                serde_json::to_string(&state_from_status(status)).unwrap(),
                expected
            );
        }
    }
}
