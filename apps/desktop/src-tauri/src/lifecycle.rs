use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};

use crate::providers::{
    CodingProvider, PROVIDER_REGISTRY, ProviderEnablementPolicy, detect_provider_presence,
};
use crate::sanitized::SanitizedProfileOutcome;

pub use crate::providers::ProviderPresenceStatus;

pub const LIFECYCLE_CONTRACT_VERSION: u8 = 3;
pub const SETTINGS_CONTRACT_VERSION: u8 = 4;
pub const SETTINGS_NAVIGATION_EVENT: &str = "settings-navigation-requested";
pub const SETTINGS_RECOVERY_CLEAR_EVENT: &str = "settings-recovery-clear-requested";
pub(crate) const DATABASE_SCHEMA_VERSION: i64 = 5;
pub(crate) const DATABASE_SCHEMA_MODULE: &str = "desktop-lifecycle";
const PUBLIC_BACKFILL_WINDOW_DAYS: u8 = 30;

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootstrapStatus {
    Required,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileProvisioningStatus {
    NotAuthorized,
    ProfilePending,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PersistenceStatus {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsSection {
    General,
    Providers,
    Profile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SettingsSelection {
    pub section: SettingsSection,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SettingsProfileAuthorization {
    revision: u64,
}

impl SettingsProfileAuthorization {
    pub(crate) fn from_selection(selection: SettingsSelection) -> Option<Self> {
        (selection.section == SettingsSection::Profile).then_some(Self {
            revision: selection.revision,
        })
    }
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "availability"
)]
pub enum LaunchAtLoginState {
    Available { enabled: bool },
    Unavailable,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPresence {
    pub provider: CodingProvider,
    #[schemars(length(min = 1, max = 40))]
    pub display_name: String,
    pub status: ProviderPresenceStatus,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsProvider {
    pub provider: CodingProvider,
    #[schemars(length(min = 1, max = 40))]
    pub display_name: String,
    pub status: ProviderPresenceStatus,
    pub enabled: bool,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStateV3 {
    pub contract_version: u8,
    pub bootstrap: BootstrapStatus,
    pub profile_provisioning: ProfileProvisioningStatus,
    pub persistence: PersistenceStatus,
    pub display_name: Option<String>,
    pub touch_grass_id: Option<String>,
    #[schemars(length(max = 16))]
    pub providers: Vec<ProviderPresence>,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsStateV4 {
    pub contract_version: u8,
    pub section: SettingsSection,
    pub launch_at_login: LaunchAtLoginState,
    pub profile_provisioning: ProfileProvisioningStatus,
    pub display_name: Option<String>,
    #[schemars(regex(
        pattern = r"^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{3}$"
    ))]
    pub recovery_key_suffix: Option<String>,
    pub touch_grass_id: Option<String>,
    #[schemars(length(max = 16))]
    pub providers: Vec<SettingsProvider>,
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
pub struct SettingsNavigationRequest {
    pub section: SettingsSection,
}

#[derive(Clone, Debug)]
struct LifecycleRecord {
    bootstrap: BootstrapStatus,
    profile_provisioning: ProfileProvisioningStatus,
    public_participation_authorized: bool,
    profile_retry_pending: bool,
    backfill_window_days: Option<u8>,
    display_name: Option<String>,
    touch_grass_id: Option<String>,
    recovery_disclosure_pending: bool,
}

impl LifecycleRecord {
    fn required() -> Self {
        Self {
            bootstrap: BootstrapStatus::Required,
            profile_provisioning: ProfileProvisioningStatus::NotAuthorized,
            public_participation_authorized: false,
            profile_retry_pending: false,
            backfill_window_days: None,
            display_name: None,
            touch_grass_id: None,
            recovery_disclosure_pending: false,
        }
    }
}

struct SqliteLifecycleStore {
    connection: Mutex<Connection>,
}

pub(crate) fn lifecycle_schema_version(
    connection: &Connection,
) -> Result<Option<i64>, &'static str> {
    let schema_table_exists = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type = 'table' AND name = 'touchgrassbar_schema_versions'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| "lifecycle persistence unavailable")?;
    if !schema_table_exists {
        return Ok(None);
    }
    connection
        .query_row(
            "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1",
            [DATABASE_SCHEMA_MODULE],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| "lifecycle persistence unavailable")
}

impl SqliteLifecycleStore {
    fn open(path: &Path) -> Result<Self, &'static str> {
        let Some(parent) = path.parent() else {
            return Err("lifecycle persistence unavailable");
        };
        fs::create_dir_all(parent).map_err(|_| "lifecycle persistence unavailable")?;
        let connection = Connection::open(path).map_err(|_| "lifecycle persistence unavailable")?;
        Self::migrate(&connection, path)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn migrate(connection: &Connection, path: &Path) -> Result<(), &'static str> {
        let explicit_version = lifecycle_schema_version(connection)?;
        let version = explicit_version.unwrap_or(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .map_err(|_| "lifecycle persistence unavailable")?,
        );
        if version > DATABASE_SCHEMA_VERSION {
            return Err("lifecycle persistence unavailable");
        }
        if version == DATABASE_SCHEMA_VERSION && explicit_version.is_some() {
            return Ok(());
        }
        if version == 0 {
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     CREATE TABLE lifecycle_state (
                       singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                       bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                       profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')),
                       public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                       profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
                       backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                       display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                       touch_grass_id TEXT,
                       recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                     );
                     INSERT INTO lifecycle_state (
                       singleton,
                       bootstrap_completed,
                       profile_provisioning,
                       public_participation_authorized,
                       profile_retry_pending,
                       backfill_window_days,
                       display_name
                     ) VALUES (1, 0, 'not-authorized', 0, 0, NULL, NULL);
                     PRAGMA user_version = 4;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        } else if version == 1 {
            Self::backup_before_migration(connection, path, version)?;
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE lifecycle_state RENAME TO lifecycle_state_v1;
                     CREATE TABLE lifecycle_state (
                       singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                       bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                       profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')),
                       public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                       profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
                       backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                       display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                       touch_grass_id TEXT,
                       recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                     );
                     INSERT INTO lifecycle_state (
                       singleton,
                       bootstrap_completed,
                       profile_provisioning,
                       public_participation_authorized,
                       profile_retry_pending,
                       backfill_window_days,
                       display_name,
                       touch_grass_id,
                       recovery_disclosure_pending
                     )
                     SELECT
                       singleton,
                       bootstrap_completed,
                       CASE profile_provisioning
                         WHEN 'identity-pending' THEN 'profile-pending'
                         ELSE profile_provisioning
                       END,
                       public_participation_authorized,
                       identity_retry_pending,
                       backfill_window_days,
                       display_name,
                       NULL,
                       0
                     FROM lifecycle_state_v1;
                     DROP TABLE lifecycle_state_v1;
                     PRAGMA user_version = 4;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        } else if version == 2 {
            Self::backup_before_migration(connection, path, version)?;
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE lifecycle_state RENAME TO lifecycle_state_v2;
                     CREATE TABLE lifecycle_state (
                       singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                       bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                       profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')),
                       public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                       profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
                       backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                       display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                       touch_grass_id TEXT,
                       recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                     );
                     INSERT INTO lifecycle_state (
                       singleton,
                       bootstrap_completed,
                       profile_provisioning,
                       public_participation_authorized,
                       profile_retry_pending,
                       backfill_window_days,
                       display_name,
                       touch_grass_id,
                       recovery_disclosure_pending
                     )
                     SELECT
                       singleton,
                       bootstrap_completed,
                       CASE profile_provisioning
                         WHEN 'identity-pending' THEN 'profile-pending'
                         ELSE profile_provisioning
                       END,
                       public_participation_authorized,
                       identity_retry_pending,
                       backfill_window_days,
                       display_name,
                       touch_grass_id,
                       recovery_disclosure_pending
                     FROM lifecycle_state_v2;
                     DROP TABLE lifecycle_state_v2;
                     PRAGMA user_version = 4;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        } else if version == 3 {
            Self::backup_before_migration(connection, path, version)?;
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     UPDATE lifecycle_state
                     SET recovery_disclosure_pending = 0;
                     PRAGMA user_version = 4;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        }
        if version <= 4 {
            if version == 4 {
                Self::backup_before_migration(connection, path, version)?;
            }
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     CREATE TABLE provider_settings (
                       provider TEXT PRIMARY KEY CHECK (provider IN ('codex', 'claude')),
                       enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
                     );
                     INSERT INTO provider_settings (provider, enabled)
                     VALUES ('codex', 1), ('claude', 1);
                     CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
                       module TEXT PRIMARY KEY,
                       version INTEGER NOT NULL CHECK (version >= 1)
                     );
                     INSERT INTO touchgrassbar_schema_versions(module, version)
                     VALUES ('desktop-lifecycle', 5)
                     ON CONFLICT(module) DO UPDATE SET version = excluded.version;
                     PRAGMA user_version = 5;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        } else if explicit_version.is_none() {
            Self::backup_before_migration(connection, path, version)?;
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
                       module TEXT PRIMARY KEY,
                       version INTEGER NOT NULL CHECK (version >= 1)
                     );
                     INSERT INTO touchgrassbar_schema_versions(module, version)
                     VALUES ('desktop-lifecycle', 5)
                     ON CONFLICT(module) DO UPDATE SET version = excluded.version;
                     COMMIT;",
                )
                .map_err(|_| "lifecycle persistence unavailable")?;
        }
        Ok(())
    }

    fn backup_before_migration(
        connection: &Connection,
        path: &Path,
        version: i64,
    ) -> Result<(), &'static str> {
        let backup_path = path.with_extension(format!("sqlite3.backup-v{version}"));
        if backup_path.exists() {
            let backup = Connection::open_with_flags(
                backup_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
            let backup_version = backup
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .map_err(|_| "lifecycle persistence unavailable")?;
            return (backup_version == version)
                .then_some(())
                .ok_or("lifecycle persistence unavailable");
        }

        let partial_path = path.with_extension(format!("sqlite3.backup-v{version}.partial"));
        if partial_path.exists() {
            fs::remove_file(&partial_path).map_err(|_| "lifecycle persistence unavailable")?;
        }
        connection
            .backup(rusqlite::MAIN_DB, &partial_path, None)
            .map_err(|_| "lifecycle persistence unavailable")?;
        fs::File::open(&partial_path)
            .and_then(|file| file.sync_all())
            .map_err(|_| "lifecycle persistence unavailable")?;
        fs::rename(partial_path, backup_path).map_err(|_| "lifecycle persistence unavailable")
    }

    fn read(&self) -> Result<LifecycleRecord, &'static str> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?;
        let record = connection
            .query_row(
                "SELECT
                   bootstrap_completed,
                   profile_provisioning,
                   public_participation_authorized,
                   profile_retry_pending,
                   backfill_window_days,
                   display_name,
                   touch_grass_id,
                   recovery_disclosure_pending
                 FROM lifecycle_state
                 WHERE singleton = 1",
                [],
                |row| {
                    let bootstrap_completed = row.get::<_, i64>(0)?;
                    let profile_provisioning = row.get::<_, String>(1)?;
                    let public_participation_authorized = row.get::<_, i64>(2)?;
                    let profile_retry_pending = row.get::<_, i64>(3)?;
                    let backfill_window_days = row.get::<_, Option<u8>>(4)?;
                    let display_name = row.get::<_, Option<String>>(5)?;
                    let touch_grass_id = row.get::<_, Option<String>>(6)?;
                    let recovery_disclosure_pending = row.get::<_, i64>(7)?;
                    Ok((
                        bootstrap_completed,
                        profile_provisioning,
                        public_participation_authorized,
                        profile_retry_pending,
                        backfill_window_days,
                        display_name,
                        touch_grass_id,
                        recovery_disclosure_pending,
                    ))
                },
            )
            .map_err(|_| "lifecycle persistence unavailable")?;

        let bootstrap = match record.0 {
            0 => BootstrapStatus::Required,
            1 => BootstrapStatus::Completed,
            _ => return Err("lifecycle persistence unavailable"),
        };
        let profile_provisioning = match record.1.as_str() {
            "not-authorized" => ProfileProvisioningStatus::NotAuthorized,
            "profile-pending" => ProfileProvisioningStatus::ProfilePending,
            "ready" => ProfileProvisioningStatus::Ready,
            _ => return Err("lifecycle persistence unavailable"),
        };
        let result = LifecycleRecord {
            bootstrap,
            profile_provisioning,
            public_participation_authorized: record.2 == 1,
            profile_retry_pending: record.3 == 1,
            backfill_window_days: record.4,
            display_name: record.5,
            touch_grass_id: record.6,
            recovery_disclosure_pending: record.7 == 1,
        };

        let valid = match (result.bootstrap, result.profile_provisioning) {
            (BootstrapStatus::Required, ProfileProvisioningStatus::NotAuthorized) => {
                !result.public_participation_authorized
                    && !result.profile_retry_pending
                    && result.backfill_window_days.is_none()
                    && result.display_name.is_none()
                    && result.touch_grass_id.is_none()
                    && !result.recovery_disclosure_pending
            }
            (BootstrapStatus::Completed, ProfileProvisioningStatus::ProfilePending) => {
                result.public_participation_authorized
                    && result.profile_retry_pending
                    && result.backfill_window_days == Some(PUBLIC_BACKFILL_WINDOW_DAYS)
                    && result.display_name.is_some()
                    && result.touch_grass_id.is_none()
                    && !result.recovery_disclosure_pending
            }
            (BootstrapStatus::Completed, ProfileProvisioningStatus::Ready) => {
                result.public_participation_authorized
                    && !result.profile_retry_pending
                    && matches!(
                        result.backfill_window_days,
                        None | Some(PUBLIC_BACKFILL_WINDOW_DAYS)
                    )
                    && result.display_name.is_some()
                    && result.touch_grass_id.is_some()
            }
            _ => false,
        };
        valid
            .then_some(result)
            .ok_or("lifecycle persistence unavailable")
    }

    fn complete_bootstrap(&self, display_name: &str) -> Result<LifecycleRecord, &'static str> {
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 40 {
            return Err("display name invalid");
        }

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?;
        let transaction = connection
            .transaction()
            .map_err(|_| "lifecycle persistence unavailable")?;
        transaction
            .execute(
                "UPDATE lifecycle_state
                 SET bootstrap_completed = 1,
                     profile_provisioning = 'profile-pending',
                     public_participation_authorized = 1,
                     profile_retry_pending = 1,
                     backfill_window_days = ?1,
                     display_name = ?2
                 WHERE singleton = 1",
                params![PUBLIC_BACKFILL_WINDOW_DAYS, display_name],
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
        transaction
            .commit()
            .map_err(|_| "lifecycle persistence unavailable")?;
        drop(connection);
        self.read()
    }

    fn mark_profile_ready(&self, touch_grass_id: &str) -> Result<(), &'static str> {
        let updated = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .execute(
                "UPDATE lifecycle_state
                 SET profile_provisioning = 'ready',
                     profile_retry_pending = 0,
                     touch_grass_id = ?1,
                     recovery_disclosure_pending = 0
                 WHERE singleton = 1
                   AND profile_provisioning = 'profile-pending'",
                [touch_grass_id],
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
        (updated == 1)
            .then_some(())
            .ok_or("profile lifecycle unavailable")
    }

    fn recover_profile(
        &self,
        display_name: &str,
        touch_grass_id: &str,
    ) -> Result<(), &'static str> {
        let display_name = display_name.trim();
        if display_name.is_empty()
            || display_name.chars().count() > 40
            || !crate::profile::valid_touch_grass_id(touch_grass_id)
        {
            return Err("profile lifecycle unavailable");
        }
        let updated = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .execute(
                "UPDATE lifecycle_state
                 SET bootstrap_completed = 1,
                     profile_provisioning = 'ready',
                     public_participation_authorized = 1,
                     profile_retry_pending = 0,
                     backfill_window_days = NULL,
                     display_name = ?1,
                     touch_grass_id = ?2,
                     recovery_disclosure_pending = 0
                 WHERE singleton = 1",
                params![display_name, touch_grass_id],
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
        (updated == 1)
            .then_some(())
            .ok_or("profile lifecycle unavailable")
    }

    fn update_display_name(&self, display_name: &str) -> Result<(), &'static str> {
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 40 {
            return Err("display name invalid");
        }
        let updated = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .execute(
                "UPDATE lifecycle_state
                 SET display_name = ?1
                 WHERE singleton = 1
                   AND profile_provisioning = 'ready'",
                [display_name],
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
        (updated == 1)
            .then_some(())
            .ok_or("profile lifecycle unavailable")
    }

    fn flush(&self) -> Result<(), &'static str> {
        self.connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .execute_batch("PRAGMA wal_checkpoint(FULL);")
            .map_err(|_| "lifecycle persistence unavailable")
    }

    fn provider_enabled(&self, provider: CodingProvider) -> Result<bool, &'static str> {
        let provider = provider_storage_key(provider);
        self.connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .query_row(
                "SELECT enabled FROM provider_settings WHERE provider = ?1",
                [provider],
                |row| row.get::<_, i64>(0),
            )
            .map(|enabled| enabled == 1)
            .map_err(|_| "lifecycle persistence unavailable")
    }

    fn set_provider_enabled(
        &self,
        provider: CodingProvider,
        enabled: bool,
    ) -> Result<(), &'static str> {
        let updated = self
            .connection
            .lock()
            .map_err(|_| "lifecycle persistence unavailable")?
            .execute(
                "UPDATE provider_settings SET enabled = ?1 WHERE provider = ?2",
                params![i64::from(enabled), provider_storage_key(provider)],
            )
            .map_err(|_| "lifecycle persistence unavailable")?;
        (updated == 1)
            .then_some(())
            .ok_or("lifecycle persistence unavailable")
    }
}

fn provider_storage_key(provider: CodingProvider) -> &'static str {
    match provider {
        CodingProvider::Codex => "codex",
        CodingProvider::Claude => "claude",
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileRequest {
    pub display_name: String,
}

trait ProviderPresenceDetector: Send + Sync {
    fn detect(&self, provider: CodingProvider) -> ProviderPresenceStatus;
}

struct SystemProviderPresenceDetector;

impl ProviderPresenceDetector for SystemProviderPresenceDetector {
    fn detect(&self, provider: CodingProvider) -> ProviderPresenceStatus {
        detect_provider_presence(provider)
    }
}

enum LifecycleStore {
    Persistent(SqliteLifecycleStore),
    Unavailable,
}

struct DesktopLifecycleInner {
    store: LifecycleStore,
    detector: Arc<dyn ProviderPresenceDetector>,
    settings_selection: Mutex<SettingsSelection>,
}

#[derive(Clone)]
pub struct DesktopLifecycle {
    inner: Arc<DesktopLifecycleInner>,
}

impl DesktopLifecycle {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        Self::open_with_detector(path, Arc::new(SystemProviderPresenceDetector))
    }

    fn open_with_detector(
        path: &Path,
        detector: Arc<dyn ProviderPresenceDetector>,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            inner: Arc::new(DesktopLifecycleInner {
                store: LifecycleStore::Persistent(SqliteLifecycleStore::open(path)?),
                detector,
                settings_selection: Mutex::new(SettingsSelection {
                    section: SettingsSection::General,
                    revision: 0,
                }),
            }),
        })
    }

    pub fn unavailable() -> Self {
        Self {
            inner: Arc::new(DesktopLifecycleInner {
                store: LifecycleStore::Unavailable,
                detector: Arc::new(SystemProviderPresenceDetector),
                settings_selection: Mutex::new(SettingsSelection {
                    section: SettingsSection::General,
                    revision: 0,
                }),
            }),
        }
    }

    pub(crate) fn flush(&self) -> Result<(), &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.flush(),
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    fn record(&self) -> Result<LifecycleRecord, &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.read(),
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    fn providers(&self) -> Vec<ProviderPresence> {
        PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| ProviderPresence {
                provider: descriptor.provider,
                display_name: descriptor.display_name.to_owned(),
                status: self.inner.detector.detect(descriptor.provider),
            })
            .collect()
    }

    fn settings_providers(&self) -> Vec<SettingsProvider> {
        PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| SettingsProvider {
                provider: descriptor.provider,
                display_name: descriptor.display_name.to_owned(),
                status: self.inner.detector.detect(descriptor.provider),
                enabled: self.is_provider_enabled(descriptor.provider),
            })
            .collect()
    }

    pub fn should_show_bootstrap(&self) -> bool {
        self.record()
            .map(|record| record.bootstrap == BootstrapStatus::Required)
            .unwrap_or(true)
    }

    pub fn bootstrap_state(&self) -> BootstrapStateV3 {
        let (record, persistence) = self
            .record()
            .map(|record| (record, PersistenceStatus::Available))
            .unwrap_or_else(|_| (LifecycleRecord::required(), PersistenceStatus::Unavailable));
        BootstrapStateV3 {
            contract_version: LIFECYCLE_CONTRACT_VERSION,
            bootstrap: record.bootstrap,
            profile_provisioning: record.profile_provisioning,
            persistence,
            display_name: record.display_name,
            touch_grass_id: record.touch_grass_id,
            providers: self.providers(),
        }
    }

    pub fn complete_bootstrap(&self, display_name: &str) -> Result<BootstrapStateV3, &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => {
                store.complete_bootstrap(display_name)?;
                Ok(self.bootstrap_state())
            }
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    pub(crate) fn profile_request(&self) -> Option<ProfileRequest> {
        let record = self.record().ok()?;
        (record.profile_provisioning == ProfileProvisioningStatus::ProfilePending
            && record.profile_retry_pending)
            .then(|| ProfileRequest {
                display_name: record
                    .display_name
                    .expect("pending Profile has a display name"),
            })
    }

    pub(crate) fn mark_profile_ready(&self, touch_grass_id: &str) -> Result<(), &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.mark_profile_ready(touch_grass_id),
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    pub(crate) fn recover_profile(
        &self,
        display_name: &str,
        touch_grass_id: &str,
    ) -> Result<(), &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => {
                store.recover_profile(display_name, touch_grass_id)
            }
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    pub(crate) fn update_display_name(&self, display_name: &str) -> Result<(), &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.update_display_name(display_name),
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    pub(crate) fn bootstrap_completion_ready(&self) -> bool {
        self.record().is_ok_and(|record| {
            record.bootstrap == BootstrapStatus::Completed
                && record.profile_provisioning == ProfileProvisioningStatus::Ready
        })
    }

    pub(crate) fn ready_touch_grass_id(&self) -> Option<String> {
        let record = self.record().ok()?;
        (record.profile_provisioning == ProfileProvisioningStatus::Ready)
            .then_some(record.touch_grass_id)
            .flatten()
    }

    pub(crate) fn sanitized_profile_outcome(&self) -> SanitizedProfileOutcome {
        let Ok(record) = self.record() else {
            return SanitizedProfileOutcome::NotAuthorized;
        };
        match record.profile_provisioning {
            ProfileProvisioningStatus::NotAuthorized => SanitizedProfileOutcome::NotAuthorized,
            ProfileProvisioningStatus::ProfilePending => SanitizedProfileOutcome::ProfilePending,
            ProfileProvisioningStatus::Ready => SanitizedProfileOutcome::Ready {
                display_name: record
                    .display_name
                    .expect("ready Profile has a display name"),
                touch_grass_id: record
                    .touch_grass_id
                    .expect("ready Profile has a TouchGrass ID"),
            },
        }
    }

    pub fn settings_state(&self, launch_at_login: LaunchAtLoginState) -> SettingsStateV4 {
        let record = self
            .record()
            .unwrap_or_else(|_| LifecycleRecord::required());
        SettingsStateV4 {
            contract_version: SETTINGS_CONTRACT_VERSION,
            section: self.current_settings_section(),
            launch_at_login,
            profile_provisioning: record.profile_provisioning,
            display_name: record.display_name,
            recovery_key_suffix: None,
            touch_grass_id: record.touch_grass_id,
            providers: self.settings_providers(),
        }
    }

    pub fn set_provider_enabled(
        &self,
        provider: CodingProvider,
        enabled: bool,
    ) -> Result<(), &'static str> {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.set_provider_enabled(provider, enabled),
            LifecycleStore::Unavailable => Err("lifecycle persistence unavailable"),
        }
    }

    pub(crate) fn is_provider_enabled(&self, provider: CodingProvider) -> bool {
        match &self.inner.store {
            LifecycleStore::Persistent(store) => store.provider_enabled(provider).unwrap_or(false),
            LifecycleStore::Unavailable => false,
        }
    }

    pub fn request_settings_section(&self, section: SettingsSection) {
        if let Ok(mut current) = self.inner.settings_selection.lock() {
            current.section = section;
            current.revision = current.revision.wrapping_add(1);
        }
    }

    pub fn current_settings_section(&self) -> SettingsSection {
        self.current_settings_selection().section
    }

    pub(crate) fn current_settings_selection(&self) -> SettingsSelection {
        self.inner
            .settings_selection
            .lock()
            .map(|selection| *selection)
            .unwrap_or(SettingsSelection {
                section: SettingsSection::General,
                revision: 0,
            })
    }

    pub(crate) fn authorize_profile_settings(&self) -> Option<SettingsProfileAuthorization> {
        SettingsProfileAuthorization::from_selection(self.current_settings_selection())
    }

    pub(crate) fn is_current_profile_settings(
        &self,
        authorization: SettingsProfileAuthorization,
    ) -> bool {
        let selection = self.current_settings_selection();
        selection.section == SettingsSection::Profile
            && selection.revision == authorization.revision
    }
}

impl ProviderEnablementPolicy for DesktopLifecycle {
    fn is_provider_enabled(&self, provider: CodingProvider) -> bool {
        DesktopLifecycle::is_provider_enabled(self, provider)
    }
}

pub fn bootstrap_state_schema() -> Schema {
    schema_for!(BootstrapStateV3)
}

pub fn settings_state_schema() -> Schema {
    schema_for!(SettingsStateV4)
}

pub fn settings_navigation_schema() -> Schema {
    schema_for!(SettingsNavigationRequest)
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::Value;

    use super::*;

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

    struct TestDatabase(PathBuf);

    impl TestDatabase {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(env::temp_dir().join(format!(
                "touchgrassbar-lifecycle-{}-{timestamp}-{}.sqlite3",
                process::id(),
                NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(self.0.with_extension("sqlite3-shm"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3-wal"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v0"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v0.partial"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v1"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v1.partial"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v2"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v2.partial"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v3"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v3.partial"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v4"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v4.partial"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v5"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3.backup-v5.partial"));
        }
    }

    struct FixtureDetector {
        codex: ProviderPresenceStatus,
        claude: ProviderPresenceStatus,
    }

    impl ProviderPresenceDetector for FixtureDetector {
        fn detect(&self, provider: CodingProvider) -> ProviderPresenceStatus {
            match provider {
                CodingProvider::Codex => self.codex,
                CodingProvider::Claude => self.claude,
            }
        }
    }

    fn detector() -> Arc<dyn ProviderPresenceDetector> {
        Arc::new(FixtureDetector {
            codex: ProviderPresenceStatus::Detected,
            claude: ProviderPresenceStatus::NotDetected,
        })
    }

    #[test]
    fn completing_bootstrap_persists_public_authorization_and_pending_retry() {
        let database = TestDatabase::new();
        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();

        assert!(lifecycle.should_show_bootstrap());
        let completed = lifecycle.complete_bootstrap("  Fabien  ").unwrap();
        assert_eq!(completed.bootstrap, BootstrapStatus::Completed);
        assert_eq!(
            completed.profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
        assert_eq!(completed.display_name.as_deref(), Some("Fabien"));

        let record = lifecycle.record().unwrap();
        assert!(record.public_participation_authorized);
        assert!(record.profile_retry_pending);
        assert_eq!(record.backfill_window_days, Some(30));
    }

    #[test]
    fn profile_pending_bootstrap_stays_closed_after_store_reconstruction() {
        let database = TestDatabase::new();
        {
            let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
            lifecycle.complete_bootstrap("Fabien").unwrap();
        }

        let relaunched = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        assert!(!relaunched.should_show_bootstrap());
        assert_eq!(
            relaunched.bootstrap_state().profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
    }

    #[test]
    fn bootstrap_completion_requires_only_a_ready_profile() {
        let database = TestDatabase::new();
        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();

        lifecycle.complete_bootstrap("Fabien").unwrap();
        assert!(!lifecycle.bootstrap_completion_ready());

        lifecycle.mark_profile_ready("TG-TEST").unwrap();
        assert!(lifecycle.bootstrap_completion_ready());
        assert!(!lifecycle.record().unwrap().recovery_disclosure_pending);
    }

    #[test]
    fn schema_upgrade_creates_a_local_backup_before_migration() {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE lifecycle_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                   profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'identity-pending', 'ready')),
                   public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                   identity_retry_pending INTEGER NOT NULL CHECK (identity_retry_pending IN (0, 1)),
                   backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                   display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40))
                 );
                 INSERT INTO lifecycle_state VALUES (1, 1, 'identity-pending', 1, 1, 30, 'Fabien');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        let record = lifecycle.record().unwrap();
        assert_eq!(
            record.profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
        assert!(record.profile_retry_pending);

        let backup_path = database.0.with_extension("sqlite3.backup-v1");
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let backup_version = backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(backup_version, 1);
        let backup_record = backup
            .query_row(
                "SELECT profile_provisioning, identity_retry_pending FROM lifecycle_state",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(backup_record, ("identity-pending".to_owned(), 1));
        assert!(
            !database
                .0
                .with_extension("sqlite3.backup-v1.partial")
                .exists()
        );

        let draft_database = TestDatabase::new();
        let connection = Connection::open(&draft_database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE lifecycle_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                   profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'identity-pending', 'ready')),
                   public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                   identity_retry_pending INTEGER NOT NULL CHECK (identity_retry_pending IN (0, 1)),
                   backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                   display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                   touch_grass_id TEXT,
                   recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                 );
                 INSERT INTO lifecycle_state VALUES (1, 1, 'identity-pending', 1, 1, 30, 'Fabien', NULL, 0);
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        drop(connection);

        let lifecycle =
            DesktopLifecycle::open_with_detector(&draft_database.0, detector()).unwrap();
        let record = lifecycle.record().unwrap();
        assert_eq!(
            record.profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
        assert!(record.profile_retry_pending);

        let backup = Connection::open_with_flags(
            draft_database.0.with_extension("sqlite3.backup-v2"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let backup_version = backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(backup_version, 2);
        let backup_record = backup
            .query_row(
                "SELECT profile_provisioning, identity_retry_pending FROM lifecycle_state",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(backup_record, ("identity-pending".to_owned(), 1));
    }

    #[test]
    fn schema_upgrade_clears_the_retired_onboarding_disclosure() {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE lifecycle_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                   profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')),
                   public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                   profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
                   backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                   display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                   touch_grass_id TEXT,
                   recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                 );
                 INSERT INTO lifecycle_state VALUES (1, 1, 'ready', 1, 0, 30, 'Fabien', 'TG-TEST', 1);
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        drop(connection);

        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();

        assert!(lifecycle.bootstrap_completion_ready());
        assert!(!lifecycle.record().unwrap().recovery_disclosure_pending);
        assert!(database.0.with_extension("sqlite3.backup-v3").exists());
    }

    #[test]
    fn profile_pending_does_not_gate_local_provider_presence() {
        let database = TestDatabase::new();
        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        lifecycle.complete_bootstrap("Fabien").unwrap();

        assert_eq!(
            lifecycle
                .bootstrap_state()
                .providers
                .into_iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            vec![
                ProviderPresenceStatus::Detected,
                ProviderPresenceStatus::NotDetected
            ]
        );
    }

    #[test]
    fn provider_settings_default_to_enabled_and_persist_each_choice() {
        let database = TestDatabase::new();
        {
            let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
            let settings = lifecycle.settings_state(LaunchAtLoginState::Unavailable);

            assert_eq!(settings.contract_version, SETTINGS_CONTRACT_VERSION);
            assert!(settings.providers.iter().all(|provider| provider.enabled));
            assert_eq!(
                lifecycle.bootstrap_state().contract_version,
                LIFECYCLE_CONTRACT_VERSION
            );
            lifecycle
                .set_provider_enabled(CodingProvider::Codex, false)
                .unwrap();
            lifecycle
                .set_provider_enabled(CodingProvider::Claude, false)
                .unwrap();
        }

        let relaunched = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        assert!(!relaunched.is_provider_enabled(CodingProvider::Codex));
        assert!(!relaunched.is_provider_enabled(CodingProvider::Claude));
        let claude = relaunched
            .settings_state(LaunchAtLoginState::Unavailable)
            .providers
            .into_iter()
            .find(|provider| provider.provider == CodingProvider::Claude)
            .unwrap();
        assert!(!claude.enabled);
    }

    #[test]
    fn provider_settings_migration_backs_up_v4_and_enables_both_providers() {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE lifecycle_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
                   profile_provisioning TEXT NOT NULL CHECK (profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')),
                   public_participation_authorized INTEGER NOT NULL CHECK (public_participation_authorized IN (0, 1)),
                   profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
                   backfill_window_days INTEGER CHECK (backfill_window_days = 30),
                   display_name TEXT CHECK (display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)),
                   touch_grass_id TEXT,
                   recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (recovery_disclosure_pending IN (0, 1))
                 );
                 INSERT INTO lifecycle_state VALUES (1, 0, 'not-authorized', 0, 0, NULL, NULL, NULL, 0);
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        drop(connection);

        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();

        assert!(lifecycle.is_provider_enabled(CodingProvider::Codex));
        assert!(lifecycle.is_provider_enabled(CodingProvider::Claude));
        let backup_path = database.0.with_extension("sqlite3.backup-v4");
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let version = backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(version, 4);
        let provider_table_exists = backup
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type = 'table' AND name = 'provider_settings'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap();
        assert!(!provider_table_exists);
    }

    #[test]
    fn unreadable_persisted_provider_choice_fails_closed() {
        let database = TestDatabase::new();
        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        lifecycle
            .set_provider_enabled(CodingProvider::Claude, false)
            .unwrap();
        Connection::open(&database.0)
            .unwrap()
            .execute(
                "DELETE FROM provider_settings WHERE provider = 'claude'",
                [],
            )
            .unwrap();

        assert!(!lifecycle.is_provider_enabled(CodingProvider::Claude));
        let unavailable = DesktopLifecycle::unavailable();
        assert!(!unavailable.is_provider_enabled(CodingProvider::Codex));
        assert!(!unavailable.is_provider_enabled(CodingProvider::Claude));
    }

    #[test]
    fn invalid_or_unavailable_persistence_never_invents_completion() {
        let lifecycle = DesktopLifecycle::unavailable();
        let state = lifecycle.bootstrap_state();

        assert_eq!(state.bootstrap, BootstrapStatus::Required);
        assert_eq!(state.persistence, PersistenceStatus::Unavailable);
        assert!(lifecycle.complete_bootstrap("Fabien").is_err());
        assert!(lifecycle.complete_bootstrap("").is_err());
    }

    #[test]
    fn sanitized_lifecycle_views_never_serialize_probe_paths_or_retry_internals() {
        let database = TestDatabase::new();
        let lifecycle = DesktopLifecycle::open_with_detector(&database.0, detector()).unwrap();
        lifecycle.complete_bootstrap("Fabien").unwrap();
        let value = serde_json::to_value(lifecycle.bootstrap_state()).unwrap();
        let serialized = value.to_string().to_lowercase();

        for prohibited in [
            "path",
            "credential",
            "cookie",
            "raw",
            "session",
            "retry",
            "backfillwindowdays",
        ] {
            assert!(!serialized.contains(prohibited), "serialized {prohibited}");
        }
        assert!(matches!(value, Value::Object(_)));
    }
}
