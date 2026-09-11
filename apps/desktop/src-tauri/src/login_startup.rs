//! Read macOS authorization for the LaunchAgent written by the autostart plugin.
//! A plist on disk records intent; it does not prove macOS permits startup.

#[cfg(target_os = "macos")]
use crate::lifecycle::LaunchAtLoginState;

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
