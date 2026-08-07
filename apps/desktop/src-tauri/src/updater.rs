use std::{
    io::ErrorKind,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, params};
use schemars::{JsonSchema, Schema, schema_for};
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const UPDATE_CONTRACT_VERSION: u8 = 1;
pub const UPDATE_STATE_CHANGED_EVENT: &str = "update-state-changed";
pub const LATEST_DMG_RECOVERY_URL: &str =
    "https://github.com/FabienGreard/TouchGrassBar/releases/latest";
const AUTOMATIC_CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;
const MAX_VERSION_LENGTH: usize = 64;

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePresentation {
    Row,
    Sheet,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateFailure {
    Download,
    Interrupted,
    LowDisk,
    Network,
    Permission,
    Replacement,
    Signature,
    Unavailable,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "status"
)]
pub enum UpdateStatus {
    Unavailable,
    Idle,
    Checking,
    UpToDate,
    Available {
        #[schemars(length(min = 1, max = 64))]
        version: String,
        presentation: UpdatePresentation,
    },
    Downloading {
        #[schemars(length(min = 1, max = 64))]
        version: String,
        #[schemars(range(min = 0, max = 100))]
        progress_percent: Option<u8>,
    },
    Installing {
        #[schemars(length(min = 1, max = 64))]
        version: String,
    },
    Failed {
        #[schemars(length(min = 1, max = 64))]
        version: Option<String>,
        failure: UpdateFailure,
    },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStateV1 {
    pub contract_version: u8,
    #[schemars(length(min = 1, max = 64))]
    pub current_version: String,
    pub online_features_paused: bool,
    pub update: UpdateStatus,
}

impl UpdateStateV1 {
    fn new(current_version: String, update: UpdateStatus) -> Self {
        Self {
            contract_version: UPDATE_CONTRACT_VERSION,
            current_version,
            online_features_paused: false,
            update,
        }
    }
}

#[derive(Clone, Default)]
pub struct OnlineFeatureGate(Arc<std::sync::atomic::AtomicBool>);

impl OnlineFeatureGate {
    pub fn is_paused(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }

    fn set_paused(&self, paused: bool) {
        self.0.store(paused, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CheckKind {
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryAction {
    Check,
    Install,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPolicy {
    minimum_supported_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedPolicy {
    minimum_supported_version: Version,
}

fn validated_policy(
    raw_json: &serde_json::Value,
    current_version: &Version,
    offered_version: &Version,
) -> Option<ValidatedPolicy> {
    let value = raw_json.get("touchgrassbar")?.clone();
    let policy: ManifestPolicy = serde_json::from_value(value).ok()?;
    let minimum_supported_version = Version::parse(&policy.minimum_supported_version).ok()?;
    (minimum_supported_version > *current_version && minimum_supported_version <= *offered_version)
        .then_some(ValidatedPolicy {
            minimum_supported_version,
        })
}

struct UpdatePersistence {
    connection: Mutex<Connection>,
    memory: Mutex<PersistedUpdateState>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PersistedUpdateState {
    deferred_version: Option<String>,
    last_automatic_check_at: Option<i64>,
    minimum_required_version: Option<String>,
}

impl UpdatePersistence {
    fn open(path: Option<&Path>) -> Result<Self, ()> {
        let path = path.ok_or(())?;
        let connection = Connection::open(path).map_err(|_| ())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 CREATE TABLE IF NOT EXISTS touchgrassbar_update_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   last_automatic_check_at INTEGER,
                   deferred_version TEXT CHECK (
                     deferred_version IS NULL OR
                     length(deferred_version) BETWEEN 1 AND 64
                   ),
                   minimum_required_version TEXT CHECK (
                     minimum_required_version IS NULL OR
                     length(minimum_required_version) BETWEEN 1 AND 64
                   )
                 );",
            )
            .map_err(|_| ())?;
        let has_minimum_column = connection
            .query_row(
                "SELECT EXISTS (
                   SELECT 1 FROM pragma_table_info('touchgrassbar_update_state')
                   WHERE name = 'minimum_required_version'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| ())?;
        if !has_minimum_column {
            connection
                .execute_batch(
                    "ALTER TABLE touchgrassbar_update_state
                     ADD COLUMN minimum_required_version TEXT CHECK (
                       minimum_required_version IS NULL OR
                       length(minimum_required_version) BETWEEN 1 AND 64
                     );",
                )
                .map_err(|_| ())?;
        }
        connection
            .execute(
                "INSERT OR IGNORE INTO touchgrassbar_update_state (
                   singleton, last_automatic_check_at, deferred_version,
                   minimum_required_version
                 ) VALUES (1, NULL, NULL, NULL)",
                [],
            )
            .map_err(|_| ())?;
        let persisted = connection
            .query_row(
                "SELECT last_automatic_check_at, deferred_version,
                        minimum_required_version
                 FROM touchgrassbar_update_state WHERE singleton = 1",
                [],
                |row| {
                    Ok(PersistedUpdateState {
                        last_automatic_check_at: row.get(0)?,
                        deferred_version: row.get(1)?,
                        minimum_required_version: row.get(2)?,
                    })
                },
            )
            .map_err(|_| ())?;
        Ok(Self {
            connection: Mutex::new(connection),
            memory: Mutex::new(persisted),
        })
    }

    fn snapshot(&self) -> Result<PersistedUpdateState, ()> {
        self.memory
            .lock()
            .map(|state| state.clone())
            .map_err(|_| ())
    }

    fn claim_automatic_check(&self, now: i64) -> Result<bool, ()> {
        let mut memory = self.memory.lock().map_err(|_| ())?;
        if memory
            .last_automatic_check_at
            .is_some_and(|last| now.saturating_sub(last) < AUTOMATIC_CHECK_INTERVAL_SECONDS)
        {
            return Ok(false);
        }
        let connection = self.connection.lock().map_err(|_| ())?;
        let changed = connection
            .execute(
                "UPDATE touchgrassbar_update_state
                 SET last_automatic_check_at = ?1 WHERE singleton = 1",
                [now],
            )
            .map_err(|_| ())?;
        if changed != 1 {
            return Err(());
        }
        memory.last_automatic_check_at = Some(now);
        Ok(true)
    }

    fn set_offer(
        &self,
        deferred_version: Option<&str>,
        minimum_required_version: Option<&str>,
    ) -> Result<(), ()> {
        let mut memory = self.memory.lock().map_err(|_| ())?;
        let connection = self.connection.lock().map_err(|_| ())?;
        let changed = connection
            .execute(
                "UPDATE touchgrassbar_update_state
                 SET deferred_version = ?1, minimum_required_version = ?2
                 WHERE singleton = 1",
                params![deferred_version, minimum_required_version],
            )
            .map_err(|_| ())?;
        if changed != 1 {
            return Err(());
        }
        memory.deferred_version = deferred_version.map(str::to_owned);
        memory.minimum_required_version = minimum_required_version.map(str::to_owned);
        Ok(())
    }

    fn set_deferred_version(&self, version: Option<&str>) -> Result<(), ()> {
        let mut memory = self.memory.lock().map_err(|_| ())?;
        let connection = self.connection.lock().map_err(|_| ())?;
        let changed = connection
            .execute(
                "UPDATE touchgrassbar_update_state
                 SET deferred_version = ?1 WHERE singleton = 1",
                params![version],
            )
            .map_err(|_| ())?;
        if changed != 1 {
            return Err(());
        }
        memory.deferred_version = version.map(str::to_owned);
        Ok(())
    }
}

#[derive(Clone)]
struct PendingUpdate {
    update: Update,
    version: String,
    minimum_required: bool,
}

#[derive(Clone)]
pub struct UpdateRuntime {
    app: AppHandle,
    current_version: Version,
    gate: OnlineFeatureGate,
    install_after_check: Arc<std::sync::atomic::AtomicBool>,
    pending: Arc<Mutex<Option<PendingUpdate>>>,
    persistence: Option<Arc<UpdatePersistence>>,
    retry: Arc<Mutex<RetryAction>>,
    state: Arc<Mutex<UpdateStateV1>>,
}

impl UpdateRuntime {
    pub fn open(
        app: AppHandle,
        database_path: Option<&Path>,
        gate: OnlineFeatureGate,
        available: bool,
    ) -> Self {
        let current_version = app.package_info().version.clone();
        let persistence = available
            .then(|| UpdatePersistence::open(database_path))
            .and_then(Result::ok)
            .map(Arc::new);
        let persisted = persistence
            .as_ref()
            .and_then(|persistence| persistence.snapshot().ok());
        let minimum_required = persisted
            .as_ref()
            .is_some_and(|persisted| persisted_minimum_required(persisted, &current_version));
        gate.set_paused(minimum_required);
        let initial_update = if !available || persistence.is_none() || persisted.is_none() {
            UpdateStatus::Unavailable
        } else if let Some(version) = persisted
            .and_then(|persisted| persisted.deferred_version)
            .filter(|version| version_is_newer(version, &current_version))
        {
            UpdateStatus::Available {
                version,
                presentation: UpdatePresentation::Row,
            }
        } else {
            UpdateStatus::Idle
        };
        let mut state = UpdateStateV1::new(current_version.to_string(), initial_update);
        state.online_features_paused = minimum_required;
        Self {
            app,
            current_version: current_version.clone(),
            gate,
            install_after_check: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending: Arc::new(Mutex::new(None)),
            persistence,
            retry: Arc::new(Mutex::new(RetryAction::Check)),
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn state(&self) -> UpdateStateV1 {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| {
                UpdateStateV1::new(self.current_version.to_string(), UpdateStatus::Unavailable)
            })
    }

    pub fn request_automatic_check(&self) -> UpdateStateV1 {
        self.request_check(CheckKind::Automatic)
    }

    pub fn request_manual_check(&self) -> UpdateStateV1 {
        self.request_check(CheckKind::Manual)
    }

    fn request_check(&self, kind: CheckKind) -> UpdateStateV1 {
        if matches!(self.state().update, UpdateStatus::Unavailable) {
            return self.state();
        }
        if matches!(
            self.state().update,
            UpdateStatus::Checking
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Installing { .. }
        ) {
            return self.state();
        }
        if kind == CheckKind::Automatic {
            let claim = self
                .persistence
                .as_ref()
                .ok_or(())
                .and_then(|persistence| persistence.claim_automatic_check(unix_timestamp()));
            match claim {
                Ok(true) => {}
                Ok(false) => return self.state(),
                Err(()) => {
                    self.fail(None, UpdateFailure::Unavailable, RetryAction::Check);
                    return self.state();
                }
            }
        }
        self.publish(UpdateStatus::Checking, self.gate.is_paused());
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            runtime.check_now().await;
        });
        self.state()
    }

    async fn check_now(&self) {
        let result = match self.app.updater() {
            Ok(updater) => updater.check().await,
            Err(error) => {
                self.fail(None, classify_error(&error, false), RetryAction::Check);
                return;
            }
        };
        match result {
            Ok(Some(update)) => {
                let Ok(version) = Version::parse(&update.version) else {
                    self.fail(None, UpdateFailure::Unavailable, RetryAction::Check);
                    return;
                };
                if update.version.len() > MAX_VERSION_LENGTH {
                    self.fail(None, UpdateFailure::Unavailable, RetryAction::Check);
                    return;
                }
                let minimum_required_version =
                    validated_policy(&update.raw_json, &self.current_version, &version)
                        .map(|policy| policy.minimum_supported_version.to_string());
                let minimum_required = minimum_required_version.is_some();
                self.gate.set_paused(minimum_required);
                let Some(persistence) = &self.persistence else {
                    self.fail(
                        Some(version.to_string()),
                        UpdateFailure::Unavailable,
                        RetryAction::Check,
                    );
                    return;
                };
                let deferred_version = Some(version.to_string());
                if persistence
                    .set_offer(
                        deferred_version.as_deref(),
                        minimum_required_version.as_deref(),
                    )
                    .is_err()
                {
                    self.fail(
                        Some(version.to_string()),
                        UpdateFailure::Unavailable,
                        RetryAction::Check,
                    );
                    return;
                }
                if let Ok(mut pending) = self.pending.lock() {
                    *pending = Some(PendingUpdate {
                        update,
                        version: version.to_string(),
                        minimum_required,
                    });
                }
                self.publish(
                    UpdateStatus::Available {
                        version: version.to_string(),
                        presentation: UpdatePresentation::Sheet,
                    },
                    minimum_required,
                );
                if self
                    .install_after_check
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    self.request_install();
                }
            }
            Ok(None) => {
                self.install_after_check
                    .store(false, std::sync::atomic::Ordering::Release);
                self.gate.set_paused(false);
                if let Ok(mut pending) = self.pending.lock() {
                    *pending = None;
                }
                let cleared = self
                    .persistence
                    .as_ref()
                    .ok_or(())
                    .and_then(|persistence| persistence.set_offer(None, None));
                if cleared.is_err() {
                    self.fail(None, UpdateFailure::Unavailable, RetryAction::Check);
                    return;
                }
                self.publish(UpdateStatus::UpToDate, false);
            }
            Err(error) => self.fail(None, classify_error(&error, false), RetryAction::Check),
        }
    }

    pub fn defer(&self) -> UpdateStateV1 {
        let current = self.state();
        if let UpdateStatus::Available { version, .. } = current.update {
            let persisted = self
                .persistence
                .as_ref()
                .ok_or(())
                .and_then(|persistence| persistence.set_deferred_version(Some(&version)));
            if persisted.is_err() {
                self.fail(
                    Some(version),
                    UpdateFailure::Unavailable,
                    RetryAction::Check,
                );
                return self.state();
            }
            self.publish(
                UpdateStatus::Available {
                    version,
                    presentation: UpdatePresentation::Row,
                },
                current.online_features_paused,
            );
        }
        self.state()
    }

    pub fn request_install(&self) -> UpdateStateV1 {
        if matches!(
            self.state().update,
            UpdateStatus::Downloading { .. } | UpdateStatus::Installing { .. }
        ) {
            return self.state();
        }
        let pending = self.pending.lock().ok().and_then(|pending| pending.clone());
        let Some(pending) = pending else {
            self.install_after_check
                .store(true, std::sync::atomic::Ordering::Release);
            self.request_manual_check();
            return self.state();
        };
        self.publish(
            UpdateStatus::Downloading {
                version: pending.version.clone(),
                progress_percent: Some(0),
            },
            pending.minimum_required,
        );
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            runtime.download_and_install(pending).await;
        });
        self.state()
    }

    async fn download_and_install(&self, pending: PendingUpdate) {
        let progress_runtime = self.clone();
        let version = pending.version.clone();
        let mut downloaded = 0_u64;
        let bytes = pending
            .update
            .download(
                move |chunk, total| {
                    downloaded = downloaded.saturating_add(chunk as u64);
                    let progress_percent = total
                        .filter(|total| *total > 0)
                        .map(|total| ((downloaded.saturating_mul(100) / total).min(100)) as u8);
                    progress_runtime.publish(
                        UpdateStatus::Downloading {
                            version: version.clone(),
                            progress_percent,
                        },
                        pending.minimum_required,
                    );
                },
                || {},
            )
            .await;
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(
                    Some(pending.version),
                    classify_error(&error, true),
                    RetryAction::Install,
                );
                return;
            }
        };

        self.publish(
            UpdateStatus::Installing {
                version: pending.version.clone(),
            },
            pending.minimum_required,
        );
        let Some(profile_runtime) = self.app.try_state::<crate::ProfileRuntime>() else {
            self.fail(
                Some(pending.version),
                UpdateFailure::Replacement,
                RetryAction::Install,
            );
            return;
        };
        let Some(core) = self.app.try_state::<crate::sanitized::NativeCore>() else {
            self.fail(
                Some(pending.version),
                UpdateFailure::Replacement,
                RetryAction::Install,
            );
            return;
        };
        let Some(lifecycle) = self.app.try_state::<crate::lifecycle::DesktopLifecycle>() else {
            self.fail(
                Some(pending.version),
                UpdateFailure::Replacement,
                RetryAction::Install,
            );
            return;
        };
        let result = finish_verified_install(
            || {
                let profile_pause = profile_runtime.pause_for_update();
                let core_pause = core.pause_for_update();
                lifecycle.flush().map_err(|_| UpdateFailure::Replacement)?;
                core.flush().map_err(|_| UpdateFailure::Replacement)?;
                Ok((profile_pause, core_pause))
            },
            || {
                pending
                    .update
                    .install(bytes)
                    .map_err(|error| classify_error(&error, true))
            },
            |(profile_pause, core_pause)| {
                profile_pause.keep_paused();
                core_pause.keep_paused();
            },
            || {
                self.app.request_restart();
            },
        );
        if let Err(failure) = result {
            self.fail(Some(pending.version), failure, RetryAction::Install);
        }
    }

    pub fn retry(&self) -> UpdateStateV1 {
        let action = self
            .retry
            .lock()
            .map(|action| *action)
            .unwrap_or(RetryAction::Check);
        match action {
            RetryAction::Check => self.request_manual_check(),
            RetryAction::Install => self.request_install(),
        }
    }

    pub fn open_latest_dmg(&self) -> Result<(), &'static str> {
        #[cfg(target_os = "macos")]
        {
            Command::new("/usr/bin/open")
                .arg(LATEST_DMG_RECOVERY_URL)
                .spawn()
                .map(|_| ())
                .map_err(|_| "recovery download unavailable")
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("recovery download unavailable")
        }
    }

    fn fail(&self, version: Option<String>, failure: UpdateFailure, retry: RetryAction) {
        if let Ok(mut current_retry) = self.retry.lock() {
            *current_retry = retry;
        }
        self.publish(
            UpdateStatus::Failed { version, failure },
            self.gate.is_paused(),
        );
    }

    fn publish(&self, update: UpdateStatus, online_features_paused: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.online_features_paused = online_features_paused;
            state.update = update;
        }
        let _ = self.app.emit(UPDATE_STATE_CHANGED_EVENT, ());
    }
}

fn finish_verified_install<Guard>(
    prepare: impl FnOnce() -> Result<Guard, UpdateFailure>,
    install: impl FnOnce() -> Result<(), UpdateFailure>,
    keep_paused: impl FnOnce(Guard),
    relaunch: impl FnOnce(),
) -> Result<(), UpdateFailure> {
    let guard = prepare()?;
    install()?;
    keep_paused(guard);
    relaunch();
    Ok(())
}

fn classify_error(
    error: &tauri_plugin_updater::Error,
    during_download_or_install: bool,
) -> UpdateFailure {
    use tauri_plugin_updater::Error;
    match error {
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => UpdateFailure::Signature,
        Error::Reqwest(error)
            if during_download_or_install && (error.is_timeout() || error.is_body()) =>
        {
            UpdateFailure::Interrupted
        }
        Error::Reqwest(_) | Error::Network(_) => {
            if during_download_or_install {
                UpdateFailure::Download
            } else {
                UpdateFailure::Network
            }
        }
        Error::Io(error) if error.kind() == ErrorKind::UnexpectedEof => UpdateFailure::Interrupted,
        Error::Io(error) if error.kind() == ErrorKind::StorageFull => UpdateFailure::LowDisk,
        Error::Io(error) if error.kind() == ErrorKind::PermissionDenied => {
            UpdateFailure::Permission
        }
        Error::AuthenticationFailed => UpdateFailure::Permission,
        Error::Io(_) | Error::PackageInstallFailed | Error::FailedToDetermineExtractPath => {
            UpdateFailure::Replacement
        }
        Error::EmptyEndpoints
        | Error::Semver(_)
        | Error::Serialization(_)
        | Error::ReleaseNotFound
        | Error::UnsupportedArch
        | Error::UnsupportedOs
        | Error::UrlParse(_)
        | Error::TargetNotFound(_)
        | Error::TargetsNotFound(_)
        | Error::BinaryNotFoundInArchive
        | Error::TempDirNotFound
        | Error::DebInstallFailed
        | Error::InvalidUpdaterFormat
        | Error::Http(_)
        | Error::InvalidHeaderValue(_)
        | Error::InvalidHeaderName(_)
        | Error::FormatDate
        | Error::InsecureTransportProtocol
        | Error::Tauri(_)
        | _ => UpdateFailure::Unavailable,
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
fn valid_version(version: &str) -> bool {
    version.len() <= MAX_VERSION_LENGTH && Version::parse(version).is_ok()
}

fn version_is_newer(version: &str, current_version: &Version) -> bool {
    version.len() <= MAX_VERSION_LENGTH
        && Version::parse(version).is_ok_and(|version| version > *current_version)
}

fn persisted_minimum_required(persisted: &PersistedUpdateState, current_version: &Version) -> bool {
    persisted
        .minimum_required_version
        .as_deref()
        .is_some_and(|version| version_is_newer(version, current_version))
}

pub fn update_state_schema() -> Schema {
    schema_for!(UpdateStateV1)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use serde_json::json;

    use super::*;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDatabase(PathBuf);

    impl TestDatabase {
        fn new() -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "touchgrassbar-updater-{}-{id}.sqlite3",
                process::id()
            )))
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(self.0.with_extension("sqlite3-shm"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3-wal"));
        }
    }

    #[test]
    fn automatic_check_claim_is_persistent_and_manual_checks_are_independent() {
        let database = TestDatabase::new();
        let first = UpdatePersistence::open(Some(&database.0)).unwrap();
        assert!(first.claim_automatic_check(10_000).unwrap());
        assert!(!first.claim_automatic_check(10_001).unwrap());
        assert!(
            !first
                .claim_automatic_check(10_000 + AUTOMATIC_CHECK_INTERVAL_SECONDS - 1)
                .unwrap()
        );
        drop(first);

        let reopened = UpdatePersistence::open(Some(&database.0)).unwrap();
        assert!(
            !reopened
                .claim_automatic_check(10_000 + AUTOMATIC_CHECK_INTERVAL_SECONDS - 1)
                .unwrap()
        );
        assert!(
            reopened
                .claim_automatic_check(10_000 + AUTOMATIC_CHECK_INTERVAL_SECONDS)
                .unwrap()
        );
        assert_eq!(CheckKind::Manual, CheckKind::Manual);
    }

    #[test]
    fn available_offer_persists_only_a_bounded_newer_semver() {
        let database = TestDatabase::new();
        let persistence = UpdatePersistence::open(Some(&database.0)).unwrap();
        persistence.set_deferred_version(Some("1.4.0")).unwrap();
        drop(persistence);

        let reopened = UpdatePersistence::open(Some(&database.0)).unwrap();
        assert_eq!(
            reopened.snapshot().unwrap().deferred_version.as_deref(),
            Some("1.4.0")
        );
        assert!(valid_version("1.4.0"));
        assert!(!valid_version("latest"));
        assert!(!valid_version(&"1".repeat(MAX_VERSION_LENGTH + 1)));
        let recovered_version = Version::parse("1.5.0").unwrap();
        assert!(!version_is_newer("1.4.0", &recovered_version));
    }

    #[test]
    fn minimum_version_policy_and_deferred_offer_survive_restart() {
        let database = TestDatabase::new();
        let persistence = UpdatePersistence::open(Some(&database.0)).unwrap();
        persistence.set_offer(Some("1.4.0"), Some("1.3.0")).unwrap();
        drop(persistence);

        let reopened = UpdatePersistence::open(Some(&database.0)).unwrap();
        assert_eq!(
            reopened.snapshot().unwrap(),
            PersistedUpdateState {
                deferred_version: Some("1.4.0".to_owned()),
                last_automatic_check_at: None,
                minimum_required_version: Some("1.3.0".to_owned()),
            }
        );
        assert!(persisted_minimum_required(
            &reopened.snapshot().unwrap(),
            &Version::parse("1.2.0").unwrap(),
        ));
        assert!(!persisted_minimum_required(
            &reopened.snapshot().unwrap(),
            &Version::parse("1.4.0").unwrap(),
        ));
    }

    #[test]
    fn legacy_update_state_adds_minimum_column_without_losing_state() {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_update_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   last_automatic_check_at INTEGER,
                   deferred_version TEXT
                 );
                 INSERT INTO touchgrassbar_update_state (
                   singleton, last_automatic_check_at, deferred_version
                 ) VALUES (1, 10000, '1.4.0');",
            )
            .unwrap();
        drop(connection);

        let migrated = UpdatePersistence::open(Some(&database.0)).unwrap();
        assert_eq!(
            migrated.snapshot().unwrap(),
            PersistedUpdateState {
                deferred_version: Some("1.4.0".to_owned()),
                last_automatic_check_at: Some(10_000),
                minimum_required_version: None,
            }
        );
    }

    #[test]
    fn persistence_failures_do_not_claim_or_change_durable_state() {
        let database = TestDatabase::new();
        let persistence = UpdatePersistence::open(Some(&database.0)).unwrap();
        persistence
            .connection
            .lock()
            .unwrap()
            .execute("DROP TABLE touchgrassbar_update_state", [])
            .unwrap();

        assert_eq!(persistence.claim_automatic_check(10_000), Err(()));
        assert_eq!(persistence.set_deferred_version(Some("1.4.0")), Err(()));
        assert_eq!(
            persistence.snapshot().unwrap(),
            PersistedUpdateState::default()
        );
    }

    #[test]
    fn minimum_version_policy_accepts_only_strict_reachable_semver_metadata() {
        let current = Version::parse("1.2.0").unwrap();
        let offered = Version::parse("1.4.0").unwrap();
        assert_eq!(
            validated_policy(
                &json!({"touchgrassbar": {"minimum_supported_version": "1.3.0"}}),
                &current,
                &offered,
            ),
            Some(ValidatedPolicy {
                minimum_supported_version: Version::parse("1.3.0").unwrap()
            })
        );
        for invalid in [
            json!({"touchgrassbar": {"minimum_supported_version": "latest"}}),
            json!({"touchgrassbar": {"minimum_supported_version": "1.5.0"}}),
            json!({"touchgrassbar": {"minimum_supported_version": "1.3.0", "raw": "/private"}}),
            json!({"touchgrassbar": "1.3.0"}),
            json!({}),
        ] {
            assert_eq!(validated_policy(&invalid, &current, &offered), None);
        }
    }

    #[test]
    fn public_update_state_has_closed_failures_and_no_source_material() {
        let serialized = serde_json::to_value(UpdateStateV1 {
            contract_version: UPDATE_CONTRACT_VERSION,
            current_version: "1.2.0".to_owned(),
            online_features_paused: true,
            update: UpdateStatus::Failed {
                version: Some("1.4.0".to_owned()),
                failure: UpdateFailure::Signature,
            },
        })
        .unwrap();
        assert_eq!(
            serialized,
            json!({
                "contractVersion": 1,
                "currentVersion": "1.2.0",
                "onlineFeaturesPaused": true,
                "update": {
                    "status": "failed",
                    "version": "1.4.0",
                    "failure": "signature"
                }
            })
        );
        let text = serialized.to_string();
        for forbidden in [
            "downloadUrl",
            "rawSignature",
            "body",
            "path",
            "endpoint",
            "bytes",
        ] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn every_required_failure_has_a_closed_recovery_class() {
        let required = [
            UpdateFailure::Network,
            UpdateFailure::Download,
            UpdateFailure::Signature,
            UpdateFailure::Interrupted,
            UpdateFailure::LowDisk,
            UpdateFailure::Permission,
            UpdateFailure::Replacement,
        ];
        assert_eq!(required.len(), 7);
        assert!(
            required
                .into_iter()
                .all(|failure| failure != UpdateFailure::Unavailable)
        );
    }

    #[test]
    fn verified_install_pauses_flushes_installs_and_relaunches_in_order() {
        let calls = Mutex::new(vec!["verified"]);
        finish_verified_install(
            || {
                calls.lock().unwrap().extend(["paused", "flushed"]);
                Ok(())
            },
            || {
                calls.lock().unwrap().push("installed");
                Ok(())
            },
            |()| calls.lock().unwrap().push("kept-paused"),
            || calls.lock().unwrap().push("relaunched"),
        )
        .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "verified",
                "paused",
                "flushed",
                "installed",
                "kept-paused",
                "relaunched"
            ]
        );
    }

    #[test]
    fn failed_install_resumes_work_and_does_not_relaunch() {
        struct ResumeOnDrop<'a>(&'a Mutex<Vec<&'static str>>);
        impl Drop for ResumeOnDrop<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap().push("resumed");
            }
        }

        let calls = Mutex::new(vec!["verified"]);
        let result = finish_verified_install(
            || {
                calls.lock().unwrap().extend(["paused", "flushed"]);
                Ok(ResumeOnDrop(&calls))
            },
            || {
                calls.lock().unwrap().push("install-failed");
                Err(UpdateFailure::Replacement)
            },
            |_| calls.lock().unwrap().push("kept-paused"),
            || calls.lock().unwrap().push("relaunched"),
        );
        assert_eq!(result, Err(UpdateFailure::Replacement));
        assert_eq!(
            *calls.lock().unwrap(),
            ["verified", "paused", "flushed", "install-failed", "resumed"]
        );
    }
}
