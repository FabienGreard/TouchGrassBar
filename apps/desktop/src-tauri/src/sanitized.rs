use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rusqlite::{Connection, OptionalExtension, params};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::daily_usage_aggregate::combine_usage_periods;
use crate::lifecycle::{
    LIFECYCLE_CONTRACT_VERSION, SETTINGS_CONTRACT_VERSION, SETTINGS_NAVIGATION_EVENT,
    SETTINGS_RECOVERY_CLEAR_EVENT, bootstrap_state_schema, settings_navigation_schema,
    settings_state_schema,
};
pub use crate::providers::{CodingProvider, ProviderPresenceStatus};
use crate::providers::{
    PROVIDER_REGISTRY, ProviderEnablementPolicy, all_providers_enabled_policy,
    detect_provider_presence, production_observation_coordinator, provider_descriptor,
};
use crate::updater::{UPDATE_CONTRACT_VERSION, UPDATE_STATE_CHANGED_EVENT, update_state_schema};

pub const CONTRACT_VERSION: u8 = 3;
pub const PANEL_ADD_TOKENMAXXER_EVENT: &str = "panel-add-tokenmaxxer-requested";
pub const REVISION_NOTICE_EVENT: &str = "sanitized-desktop-state-revision";
const READ_MODEL_SCHEMA_VERSION: i64 = 4;
const READ_MODEL_SCHEMA_MODULE: &str = "sanitized-desktop-state";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REFRESH_BACKOFF_BASE: Duration = Duration::from_secs(5);
const REFRESH_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const REFRESH_ATTEMPT_TIMEOUT: Duration = REFRESH_INTERVAL;
const NETWORK_RECOVERY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const LOCAL_USAGE_CATCH_UP_DEFAULT_ACTIVE: Duration = Duration::from_secs(2);
const LOCAL_USAGE_CATCH_UP_ERROR_DELAY: Duration = Duration::from_secs(60);

#[cfg(debug_assertions)]
fn debug_local_usage_event(event: &str) {
    eprintln!("[TouchGrassBar][codex-usage] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_local_usage_event(_event: &str) {}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDesktopStateV3 {
    pub contract_version: u8,
    pub generated_at: String,
    pub revision: String,
    #[schemars(length(max = 16))]
    pub providers: Vec<ProviderPresentation>,
    pub combined_usage: UsagePeriods,
    pub sync: SyncState,
    pub profile: SanitizedProfileOutcome,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPresentation {
    pub provider: CodingProvider,
    #[schemars(length(min = 1, max = 40))]
    pub display_name: String,
    pub presence: ProviderPresenceStatus,
    pub quota: ProviderSnapshot,
    pub usage: UsagePeriods,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacySanitizedDesktopState {
    contract_version: u8,
    generated_at: String,
    revision: String,
    providers: Vec<ProviderSnapshot>,
    usage: LegacyUsageByProvider,
    sync: SyncState,
    profile: SanitizedProfileOutcome,
}

#[derive(Deserialize)]
struct LegacyUsageByProvider {
    codex: UsagePeriods,
    claude: UsagePeriods,
}

impl LegacyUsageByProvider {
    fn get(&self, provider: CodingProvider) -> &UsagePeriods {
        match provider {
            CodingProvider::Codex => &self.codex,
            CodingProvider::Claude => &self.claude,
        }
    }
}

impl LegacySanitizedDesktopState {
    fn into_current(
        self,
        revision: String,
        schema_version: i64,
    ) -> Result<SanitizedDesktopStateV3, &'static str> {
        if i64::from(self.contract_version) != schema_version || self.revision != revision {
            return Err("native state persistence unavailable");
        }
        let Self {
            contract_version: _,
            generated_at,
            revision: _,
            providers: legacy_quotas,
            usage,
            sync,
            profile,
        } = self;
        let providers = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| {
                let provider = descriptor.provider;
                let quota = legacy_quotas
                    .iter()
                    .find(|snapshot| snapshot.provider() == provider)
                    .cloned()
                    .unwrap_or(ProviderSnapshot::Unavailable {
                        provider,
                        quota_lanes: [],
                    });
                ProviderPresentation {
                    provider,
                    display_name: descriptor.display_name.to_owned(),
                    presence: detect_provider_presence(provider),
                    quota,
                    usage: usage.get(provider).clone(),
                }
            })
            .collect();
        let mut current = SanitizedDesktopStateV3 {
            contract_version: CONTRACT_VERSION,
            generated_at,
            revision,
            providers,
            combined_usage: unavailable_periods(),
            sync,
            profile,
        };
        current.refresh_combined_usage();
        Ok(current)
    }
}

impl SanitizedDesktopStateV3 {
    pub(crate) fn provider(&self, provider: CodingProvider) -> Option<&ProviderPresentation> {
        self.providers
            .iter()
            .find(|presentation| presentation.provider == provider)
    }

    pub(crate) fn provider_mut(
        &mut self,
        provider: CodingProvider,
    ) -> Option<&mut ProviderPresentation> {
        self.providers
            .iter_mut()
            .find(|presentation| presentation.provider == provider)
    }

    pub(crate) fn refresh_combined_usage(&mut self) {
        let periods = self
            .providers
            .iter()
            .filter(|presentation| presentation.is_visible())
            .map(|presentation| &presentation.usage)
            .collect::<Vec<_>>();
        self.combined_usage = combine_usage_periods(&periods);
    }

    fn apply_provider_enablement(
        &mut self,
        enablement: &dyn ProviderEnablementPolicy,
    ) -> Vec<CodingProvider> {
        let mut disabled_providers = Vec::new();
        self.providers = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| {
                let mut presentation = self
                    .provider(descriptor.provider)
                    .cloned()
                    .unwrap_or_else(|| ProviderPresentation::unavailable(descriptor.provider));
                if !enablement.is_provider_enabled(descriptor.provider) {
                    disabled_providers.push(descriptor.provider);
                    presentation.quota = ProviderSnapshot::Unavailable {
                        provider: descriptor.provider,
                        quota_lanes: [],
                    };
                    presentation.usage = unavailable_periods();
                }
                presentation
            })
            .collect();
        self.refresh_combined_usage();
        disabled_providers
    }
}

impl ProviderPresentation {
    pub(crate) fn unavailable(provider: CodingProvider) -> Self {
        let descriptor = provider_descriptor(provider);
        Self {
            provider,
            display_name: descriptor.display_name.to_owned(),
            presence: ProviderPresenceStatus::Unavailable,
            quota: ProviderSnapshot::Unavailable {
                provider,
                quota_lanes: [],
            },
            usage: unavailable_periods(),
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.presence == ProviderPresenceStatus::Detected
            || !matches!(self.quota, ProviderSnapshot::Unavailable { .. })
            || !matches!(self.usage.today, UsageTotal::Unavailable)
            || !matches!(self.usage.seven_days, UsageTotal::Unavailable)
            || !matches!(self.usage.thirty_days, UsageTotal::Unavailable)
    }

    fn is_reconnecting(&self) -> bool {
        matches!(self.quota, ProviderSnapshot::Unavailable { .. })
            && self.usage.scan_status == UsageScanStatus::Indexing
    }

    pub(crate) fn finish_reconnecting(&mut self) {
        if self.is_reconnecting() {
            let period_statuses = [
                self.usage.today_scan_status,
                self.usage.seven_day_scan_status,
                self.usage.thirty_day_scan_status,
            ];
            self.usage.scan_status = if period_statuses.contains(&UsageScanStatus::Indexing) {
                UsageScanStatus::Indexing
            } else if period_statuses.contains(&UsageScanStatus::Complete) {
                UsageScanStatus::Complete
            } else {
                UsageScanStatus::Unavailable
            };
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    tag = "status"
)]
pub enum SanitizedProfileOutcome {
    NotAuthorized,
    ProfilePending,
    Ready {
        display_name: String,
        touch_grass_id: String,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "availability"
)]
pub enum ProviderSnapshot {
    Unavailable {
        provider: CodingProvider,
        quota_lanes: [QuotaLane; 0],
    },
    Current {
        provider: CodingProvider,
        observed_at: String,
        #[schemars(length(min = 1))]
        quota_lanes: Vec<QuotaLane>,
    },
    Stale {
        provider: CodingProvider,
        observed_at: String,
        #[schemars(length(min = 1))]
        quota_lanes: Vec<QuotaLane>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaLane {
    #[schemars(length(min = 1))]
    pub label: String,
    #[schemars(length(min = 1))]
    pub unit: String,
    pub allowance: Option<f64>,
    pub remaining: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriods {
    pub scan_status: UsageScanStatus,
    #[serde(default)]
    pub today_scan_status: UsageScanStatus,
    #[serde(default)]
    pub seven_day_scan_status: UsageScanStatus,
    #[serde(default)]
    pub thirty_day_scan_status: UsageScanStatus,
    pub today: UsageTotal,
    pub seven_days: UsageTotal,
    pub thirty_days: UsageTotal,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "availability"
)]
pub enum UsageTotal {
    Unavailable,
    Current {
        evidence_basis: UsageEvidenceBasis,
        coverage: UsageCoverage,
        observed_at: String,
        observed_tokens: u64,
        api_equivalent_cost_usd: Option<f64>,
        #[serde(default)]
        trend_percent: Option<f64>,
        #[serde(default)]
        trend_previous_tokens: Option<u64>,
        #[serde(default)]
        #[schemars(length(min = 1, max = 256))]
        api_equivalent_cost_basis: Option<String>,
        #[serde(default)]
        api_equivalent_cost_quality: Option<ApiEquivalentCostQuality>,
        #[serde(default)]
        #[schemars(range(min = 0.0, max = 100.0))]
        api_equivalent_cost_coverage_percent: Option<f64>,
    },
    Stale {
        evidence_basis: UsageEvidenceBasis,
        coverage: UsageCoverage,
        observed_at: String,
        observed_tokens: u64,
        api_equivalent_cost_usd: Option<f64>,
        #[serde(default)]
        trend_percent: Option<f64>,
        #[serde(default)]
        trend_previous_tokens: Option<u64>,
        #[serde(default)]
        #[schemars(length(min = 1, max = 256))]
        api_equivalent_cost_basis: Option<String>,
        #[serde(default)]
        api_equivalent_cost_quality: Option<ApiEquivalentCostQuality>,
        #[serde(default)]
        #[schemars(range(min = 0.0, max = 100.0))]
        api_equivalent_cost_coverage_percent: Option<f64>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageEvidenceBasis {
    ProviderReported,
    LocallyDerived,
    Mixed,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageCoverage {
    Complete,
    Partial,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiEquivalentCostQuality {
    Reconciled,
    Modeled,
    LocalOnly,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageScanStatus {
    Complete,
    Indexing,
    #[default]
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub status: SyncStatus,
    pub last_successful_at: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Synced,
    Pending,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct RevisionNotice {
    pub revision: String,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReceipt {
    pub accepted: bool,
}

#[derive(Clone)]
pub(crate) struct RefreshAttempt {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    sources: RefreshSources,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RefreshFailure {
    Cancelled,
    DeadlineExceeded,
    SourceUnavailable,
}

impl RefreshAttempt {
    fn new(cancelled: Arc<AtomicBool>, sources: RefreshSources) -> Self {
        Self {
            cancelled,
            deadline: Instant::now() + REFRESH_ATTEMPT_TIMEOUT,
            sources,
        }
    }

    pub(crate) fn is_manual(&self) -> bool {
        self.sources.contains(RefreshSource::Manual)
    }

    pub(crate) fn is_local_usage_only(&self) -> bool {
        self.sources.is_only(RefreshSource::LocalUsageCatchUp)
    }

    pub(crate) fn includes_local_usage_catch_up(&self) -> bool {
        self.sources.contains(RefreshSource::LocalUsageCatchUp)
    }

    pub(crate) fn should_skip_claude_quota_probe(&self) -> bool {
        self.sources.contains_only(&[
            RefreshSource::ProviderNotification,
            RefreshSource::LocalUsageCatchUp,
        ])
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn remaining(&self) -> Result<Duration, RefreshFailure> {
        if self.is_cancelled() {
            return Err(RefreshFailure::Cancelled);
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(RefreshFailure::DeadlineExceeded)
        } else {
            Ok(remaining)
        }
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::new(
            Arc::new(AtomicBool::new(false)),
            RefreshSources(RefreshSource::Manual.bit()),
        )
    }

    #[cfg(test)]
    pub(crate) fn test_provider_notification() -> Self {
        Self::new(
            Arc::new(AtomicBool::new(false)),
            RefreshSources(RefreshSource::ProviderNotification.bit()),
        )
    }

    #[cfg(test)]
    pub(crate) fn test_provider_notification_with_local_usage() -> Self {
        Self::new(
            Arc::new(AtomicBool::new(false)),
            RefreshSources(
                RefreshSource::ProviderNotification.bit() | RefreshSource::LocalUsageCatchUp.bit(),
            ),
        )
    }
}

pub(crate) trait SnapshotRefreshAdapter: Send + Sync {
    fn install_refresh_trigger(&self, _trigger: RefreshTrigger) {}

    /// Stops resources that can block an active refresh.
    ///
    /// The refresh coordinator calls this after it cancels new work and before
    /// it joins the worker thread.
    fn shutdown(&self) {}

    /// Production adapters must bound each blocking operation by
    /// `attempt.remaining()` and stop when cancellation is observed. This
    /// keeps application shutdown bounded.
    fn refresh(
        &self,
        cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
    ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure>;
}

#[cfg(test)]
struct CachedProjectionRefreshAdapter;

#[cfg(test)]
impl SnapshotRefreshAdapter for CachedProjectionRefreshAdapter {
    fn refresh(
        &self,
        _cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
    ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
        attempt.remaining()?;
        // Provider observation is not wired yet. An unchanged cached projection
        // does not create a false revision or notice.
        Ok(None)
    }
}

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub(crate) type RefreshTrigger = Arc<dyn Fn() + Send + Sync>;

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

fn production_refresh_adapter(
    clock: Arc<dyn Clock>,
    database_path: Option<PathBuf>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
) -> Arc<dyn SnapshotRefreshAdapter> {
    Arc::new(production_observation_coordinator(
        clock,
        database_path,
        enablement,
    ))
}

struct SqliteReadModelStore {
    connection: Connection,
}

impl SqliteReadModelStore {
    fn open(
        path: &Path,
        initial: &SanitizedDesktopStateV3,
    ) -> Result<(Self, SanitizedDesktopStateV3), &'static str> {
        let Some(parent) = path.parent() else {
            return Err("native state persistence unavailable");
        };
        fs::create_dir_all(parent).map_err(|_| "native state persistence unavailable")?;
        let mut connection =
            Connection::open(path).map_err(|_| "native state persistence unavailable")?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|_| "native state persistence unavailable")?;
        Self::migrate(&mut connection, path, initial)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
            .map_err(|_| "native state persistence unavailable")?;
        let state = Self::read_from(&connection)?;
        Ok((Self { connection }, state))
    }

    fn migrate(
        connection: &mut Connection,
        path: &Path,
        initial: &SanitizedDesktopStateV3,
    ) -> Result<(), &'static str> {
        let version = read_model_schema_version(connection)?;

        if version > READ_MODEL_SCHEMA_VERSION {
            return Err("native state persistence unavailable");
        }
        if version == READ_MODEL_SCHEMA_VERSION {
            return Ok(());
        }

        backup_read_model_before_migration(connection, path, version)?;

        let (snapshot, revision) = if (1..=3).contains(&version) {
            let (revision, snapshot_json) = connection
                .query_row(
                    "SELECT revision, snapshot_json
                     FROM sanitized_desktop_state
                     WHERE singleton = 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|_| "native state persistence unavailable")?;
            let mut snapshot_value: Value = serde_json::from_str(&snapshot_json)
                .map_err(|_| "native state persistence unavailable")?;
            let snapshot_object = snapshot_value
                .as_object_mut()
                .ok_or("native state persistence unavailable")?;
            if version == 1 {
                snapshot_object.insert("profile".to_owned(), json!({ "status": "not-authorized" }));
            }
            if version <= 2 {
                if let Some(usage) = snapshot_value
                    .get_mut("usage")
                    .and_then(Value::as_object_mut)
                {
                    for provider in ["codex", "claude"] {
                        if let Some(periods) =
                            usage.get_mut(provider).and_then(Value::as_object_mut)
                        {
                            periods.insert("scanStatus".to_owned(), json!("unavailable"));
                        }
                    }
                }
            }
            let snapshot: LegacySanitizedDesktopState = serde_json::from_value(snapshot_value)
                .map_err(|_| "native state persistence unavailable")?;
            let snapshot = snapshot.into_current(revision.clone(), version)?;
            validate_snapshot(&snapshot)?;
            (snapshot, revision)
        } else {
            (initial.clone(), initial.revision.clone())
        };
        let snapshot_json =
            serde_json::to_string(&snapshot).map_err(|_| "native state persistence unavailable")?;
        let transaction = connection
            .transaction()
            .map_err(|_| "native state persistence unavailable")?;
        if version > 0 {
            transaction
                .execute_batch(
                    "ALTER TABLE sanitized_desktop_state
                       RENAME TO sanitized_desktop_state_previous;",
                )
                .map_err(|_| "native state persistence unavailable")?;
        }
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 4),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 3),
                   revision TEXT NOT NULL CHECK (
                     length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
                   ),
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .map_err(|_| "native state persistence unavailable")?;
        transaction
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, ?1, ?2, ?3, ?4)",
                params![
                    READ_MODEL_SCHEMA_VERSION,
                    CONTRACT_VERSION,
                    revision,
                    snapshot_json
                ],
            )
            .map_err(|_| "native state persistence unavailable")?;
        if version > 0 {
            transaction
                .execute_batch("DROP TABLE sanitized_desktop_state_previous;")
                .map_err(|_| "native state persistence unavailable")?;
        }
        transaction
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version)
                 VALUES (?1, ?2)
                 ON CONFLICT(module) DO UPDATE SET version = excluded.version",
                params![READ_MODEL_SCHEMA_MODULE, READ_MODEL_SCHEMA_VERSION],
            )
            .map_err(|_| "native state persistence unavailable")?;
        transaction
            .commit()
            .map_err(|_| "native state persistence unavailable")
    }

    fn read_from(connection: &Connection) -> Result<SanitizedDesktopStateV3, &'static str> {
        let (schema_version, contract_version, revision, snapshot_json) = connection
            .query_row(
                "SELECT schema_version, contract_version, revision, snapshot_json
                 FROM sanitized_desktop_state
                 WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, u8>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(|_| "native state persistence unavailable")?;
        if schema_version != READ_MODEL_SCHEMA_VERSION || contract_version != CONTRACT_VERSION {
            return Err("native state persistence unavailable");
        }
        let snapshot: SanitizedDesktopStateV3 = serde_json::from_str(&snapshot_json)
            .map_err(|_| "native state persistence unavailable")?;
        validate_snapshot(&snapshot)?;
        if snapshot.revision != revision {
            return Err("native state persistence unavailable");
        }
        Ok(snapshot)
    }

    fn commit(&mut self, state: &SanitizedDesktopStateV3) -> Result<(), &'static str> {
        validate_snapshot(state)?;
        let snapshot_json =
            serde_json::to_string(state).map_err(|_| "native state persistence unavailable")?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| "native state persistence unavailable")?;
        let updated = transaction
            .execute(
                "UPDATE sanitized_desktop_state
                 SET contract_version = ?1, revision = ?2, snapshot_json = ?3
                 WHERE singleton = 1 AND schema_version = ?4",
                params![
                    state.contract_version,
                    state.revision,
                    snapshot_json,
                    READ_MODEL_SCHEMA_VERSION
                ],
            )
            .map_err(|_| "native state persistence unavailable")?;
        if updated != 1 {
            return Err("native state persistence unavailable");
        }
        transaction
            .commit()
            .map_err(|_| "native state persistence unavailable")
    }

    fn flush(&self) -> Result<(), &'static str> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(FULL);")
            .map_err(|_| "native state persistence unavailable")
    }
}

enum ReadModelStore {
    Persistent(SqliteReadModelStore),
    Memory,
}

struct SnapshotCommitOutcome {
    notice: Option<RevisionNotice>,
    persistence_failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshSource {
    Launch,
    StalePanelOpen,
    Manual,
    Wake,
    NetworkRecovery,
    Schedule,
    ProviderNotification,
    LocalUsageCatchUp,
}

impl RefreshSource {
    fn bit(self) -> u8 {
        match self {
            Self::Launch => 1 << 0,
            Self::StalePanelOpen => 1 << 1,
            Self::Manual => 1 << 2,
            Self::Wake => 1 << 3,
            Self::NetworkRecovery => 1 << 4,
            Self::Schedule => 1 << 5,
            Self::ProviderNotification => 1 << 6,
            Self::LocalUsageCatchUp => 1 << 7,
        }
    }
}

#[derive(Clone, Copy)]
struct RefreshSources(u8);

impl RefreshSources {
    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn contains(self, source: RefreshSource) -> bool {
        self.0 & source.bit() != 0
    }

    fn is_only(self, source: RefreshSource) -> bool {
        self.0 == source.bit()
    }

    fn contains_only(self, allowed: &[RefreshSource]) -> bool {
        let allowed = allowed.iter().fold(0, |mask, source| mask | source.bit());
        self.0 != 0 && self.0 & !allowed == 0
    }

    fn contains_immediate_request(self) -> bool {
        [
            RefreshSource::Launch,
            RefreshSource::Manual,
            RefreshSource::Wake,
            RefreshSource::NetworkRecovery,
            RefreshSource::ProviderNotification,
            RefreshSource::LocalUsageCatchUp,
        ]
        .into_iter()
        .any(|source| self.contains(source))
    }
}

struct CachedProjection {
    state: Mutex<SanitizedDesktopStateV3>,
}

#[derive(Default)]
struct RevisionSubscribersState {
    closed: bool,
    senders: Vec<Sender<RevisionNotice>>,
}

struct RevisionSubscribers {
    state: Mutex<RevisionSubscribersState>,
}

impl CachedProjection {
    fn new(state: SanitizedDesktopStateV3) -> Self {
        Self {
            state: Mutex::new(state),
        }
    }

    fn snapshot(&self) -> Result<SanitizedDesktopStateV3, &'static str> {
        self.state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "native state unavailable")
    }

    fn commit_refreshed_snapshot(
        &self,
        store: &mut ReadModelStore,
        mut refreshed: SanitizedDesktopStateV3,
        enablement: &dyn ProviderEnablementPolicy,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let cached = self.snapshot()?;
        refreshed.profile.clone_from(&cached.profile);
        refreshed.providers = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| {
                let mut presentation = if enablement.is_provider_enabled(descriptor.provider) {
                    refreshed
                        .provider(descriptor.provider)
                        .cloned()
                        .or_else(|| {
                            cached
                                .provider(descriptor.provider)
                                .filter(|presentation| presentation.is_reconnecting())
                                .cloned()
                        })
                } else {
                    cached.provider(descriptor.provider).cloned()
                }
                .unwrap_or_else(|| ProviderPresentation::unavailable(descriptor.provider));
                presentation.display_name = descriptor.display_name.to_owned();
                presentation
            })
            .collect();
        refreshed.refresh_combined_usage();
        self.commit_snapshot(store, refreshed, cached, now)
    }

    fn commit_profile_outcome(
        &self,
        store: &mut ReadModelStore,
        profile: SanitizedProfileOutcome,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let cached = self.snapshot()?;
        if cached.profile == profile {
            return Ok(SnapshotCommitOutcome {
                notice: None,
                persistence_failed: false,
            });
        }
        let mut refreshed = cached.clone();
        refreshed.profile = profile;
        self.commit_snapshot(store, refreshed, cached, now)
    }

    fn commit_provider_enablement(
        &self,
        store: &mut ReadModelStore,
        changed_provider: Option<(CodingProvider, bool)>,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let cached = self.snapshot()?;
        let mut refreshed = cached.clone();
        if let Some((provider, true)) = changed_provider
            && let Some(presentation) = refreshed.provider_mut(provider)
        {
            presentation.quota = ProviderSnapshot::Unavailable {
                provider,
                quota_lanes: [],
            };
            presentation.usage.scan_status = UsageScanStatus::Indexing;
        }
        refreshed.refresh_combined_usage();
        self.commit_snapshot_with_force(store, refreshed, cached, now, changed_provider.is_some())
    }

    fn commit_snapshot(
        &self,
        store: &mut ReadModelStore,
        refreshed: SanitizedDesktopStateV3,
        cached: SanitizedDesktopStateV3,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        self.commit_snapshot_with_force(store, refreshed, cached, now, false)
    }

    fn commit_snapshot_with_force(
        &self,
        store: &mut ReadModelStore,
        mut refreshed: SanitizedDesktopStateV3,
        cached: SanitizedDesktopStateV3,
        now: OffsetDateTime,
        force_revision: bool,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        refreshed.contract_version = CONTRACT_VERSION;
        refreshed.generated_at.clone_from(&cached.generated_at);
        refreshed.revision.clone_from(&cached.revision);
        if matches!(&*store, ReadModelStore::Memory) {
            refreshed.sync.status = SyncStatus::Unavailable;
        }
        validate_snapshot(&refreshed)?;
        if refreshed == cached && !force_revision {
            return Ok(SnapshotCommitOutcome {
                notice: None,
                persistence_failed: false,
            });
        }
        let revision = cached
            .revision
            .parse::<u64>()
            .ok()
            .and_then(|revision| revision.checked_add(1))
            .ok_or("native revision unavailable")?;
        refreshed.generated_at = format_time(now);
        refreshed.revision = revision.to_string();
        validate_snapshot(&refreshed)?;

        // Persist first when SQLite is available. Panel reads can continue to
        // clone the previous complete snapshot during this transaction. If the
        // write fails, keep operating from memory and expose that synchronization
        // is unavailable instead of leaving expired values marked as current.
        let persistence_failed = match store {
            ReadModelStore::Persistent(persistent) => {
                if persistent.commit(&refreshed).is_err() {
                    *store = ReadModelStore::Memory;
                    true
                } else {
                    false
                }
            }
            ReadModelStore::Memory => {
                refreshed.sync.status = SyncStatus::Unavailable;
                false
            }
        };
        if persistence_failed {
            refreshed.sync.status = SyncStatus::Unavailable;
        }
        validate_snapshot(&refreshed)?;
        *self.state.lock().map_err(|_| "native state unavailable")? = refreshed;
        Ok(SnapshotCommitOutcome {
            notice: Some(RevisionNotice {
                revision: revision.to_string(),
            }),
            persistence_failed,
        })
    }
}

impl RevisionSubscribers {
    fn new() -> Self {
        Self {
            state: Mutex::new(RevisionSubscribersState::default()),
        }
    }

    fn subscribe(&self) -> Result<Receiver<RevisionNotice>, &'static str> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "revision notices unavailable")?;
        if state.closed {
            return Err("revision notices unavailable");
        }
        let (sender, receiver) = mpsc::channel();
        state.senders.push(sender);
        Ok(receiver)
    }

    fn publish(&self, notice: RevisionNotice) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.closed {
            return;
        }
        state
            .senders
            .retain(|sender| sender.send(notice.clone()).is_ok());
    }

    fn close(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.closed = true;
        state.senders.clear();
    }
}

struct RefreshInbox {
    admission: Mutex<()>,
    pending_sources: AtomicU8,
    provider_settings_pending: AtomicBool,
    in_flight: AtomicBool,
    paused: AtomicBool,
    stopping: AtomicBool,
    wake: SyncSender<()>,
}

impl RefreshInbox {
    fn request(&self, source: RefreshSource) -> Result<RefreshReceipt, &'static str> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("refresh coordinator unavailable");
        }
        self.record(source);
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {
                return Err("refresh coordinator unavailable");
            }
        }
        if self.stopping.load(Ordering::Acquire) {
            return Err("refresh coordinator unavailable");
        }
        Ok(RefreshReceipt { accepted: true })
    }

    fn record(&self, source: RefreshSource) {
        self.pending_sources
            .fetch_or(source.bit(), Ordering::AcqRel);
    }

    fn take_sources(&self) -> RefreshSources {
        RefreshSources(self.pending_sources.swap(0, Ordering::AcqRel))
    }

    fn try_start_refresh(&self) -> bool {
        let _admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.paused.load(Ordering::Acquire) {
            return false;
        }
        self.in_flight.store(true, Ordering::Release);
        true
    }
}

struct RefreshCoordinator {
    inbox: Arc<RefreshInbox>,
    cancelled: Arc<AtomicBool>,
    refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    worker: Mutex<Option<JoinHandle<()>>>,
    subscribers: Arc<RevisionSubscribers>,
}

pub(crate) struct UpdatePauseGuard<'a> {
    coordinator: &'a RefreshCoordinator,
    resume_on_drop: bool,
}

impl UpdatePauseGuard<'_> {
    pub(crate) fn keep_paused(mut self) {
        self.resume_on_drop = false;
    }
}

impl Drop for UpdatePauseGuard<'_> {
    fn drop(&mut self) {
        if self.resume_on_drop {
            self.coordinator
                .inbox
                .paused
                .store(false, Ordering::Release);
            let _ = self.coordinator.inbox.wake.try_send(());
        }
    }
}

impl RefreshCoordinator {
    fn start(
        projection: Arc<CachedProjection>,
        store: Arc<Mutex<ReadModelStore>>,
        subscribers: Arc<RevisionSubscribers>,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let (wake, wake_receiver) = mpsc::sync_channel(1);
        let inbox = Arc::new(RefreshInbox {
            admission: Mutex::new(()),
            pending_sources: AtomicU8::new(0),
            provider_settings_pending: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            wake,
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger_inbox = Arc::clone(&inbox);
        refresh_adapter.install_refresh_trigger(Arc::new(move || {
            trigger_inbox.record(RefreshSource::ProviderNotification);
            let _ = trigger_inbox.wake.try_send(());
        }));
        let now = clock.now();
        let next_scheduled_at = projection
            .snapshot()
            .map(|state| next_refresh_at(&state, now))
            .unwrap_or(now + to_time_duration(REFRESH_INTERVAL));
        let worker = CoordinatorWorker {
            projection,
            store,
            subscribers: Arc::clone(&subscribers),
            clock,
            refresh_adapter: Arc::clone(&refresh_adapter),
            inbox: Arc::clone(&inbox),
            cancelled: Arc::clone(&cancelled),
            enablement,
            consecutive_failures: 0,
            retry_not_before: None,
            next_scheduled_at,
            next_network_poll_at: Instant::now() + NETWORK_RECOVERY_POLL_INTERVAL,
            next_local_usage_catch_up_at: Instant::now(),
            last_network_reachability: None,
        };
        let worker_inbox = Arc::clone(&inbox);
        let worker = thread::Builder::new()
            .name("sanitized-state-coordinator".to_owned())
            .spawn(move || {
                let _ = catch_unwind(AssertUnwindSafe(|| worker.run(wake_receiver)));
                worker_inbox.in_flight.store(false, Ordering::Release);
                worker_inbox.stopping.store(true, Ordering::Release);
            })
            .ok();
        if worker.is_none() {
            inbox.stopping.store(true, Ordering::Release);
            cancelled.store(true, Ordering::Release);
        }
        Self {
            inbox,
            cancelled,
            refresh_adapter,
            worker: Mutex::new(worker),
            subscribers,
        }
    }

    fn request(&self, source: RefreshSource) -> Result<RefreshReceipt, &'static str> {
        self.inbox.request(source)
    }

    fn request_provider_refresh(&self) -> Result<RefreshReceipt, &'static str> {
        self.inbox
            .provider_settings_pending
            .store(true, Ordering::Release);
        self.inbox.request(RefreshSource::Manual)
    }

    fn pause_for_update(&self) -> UpdatePauseGuard<'_> {
        let _admission = self
            .inbox
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.inbox.paused.store(true, Ordering::Release);
        drop(_admission);
        let _ = self.inbox.wake.try_send(());
        while self.inbox.in_flight.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(10));
        }
        UpdatePauseGuard {
            coordinator: self,
            resume_on_drop: true,
        }
    }

    fn shutdown(&self) {
        self.inbox.stopping.store(true, Ordering::Release);
        self.cancelled.store(true, Ordering::Release);
        let _ = self.inbox.wake.try_send(());
        self.refresh_adapter.shutdown();
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(worker) = worker.take() {
            let _ = worker.join();
        }
        drop(worker);
        self.subscribers.close();
    }
}

impl Drop for RefreshCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum RefreshRunResult {
    Completed {
        failed: bool,
        notice: Option<RevisionNotice>,
    },
    Cancelled,
}

struct CoordinatorWorker {
    projection: Arc<CachedProjection>,
    store: Arc<Mutex<ReadModelStore>>,
    subscribers: Arc<RevisionSubscribers>,
    clock: Arc<dyn Clock>,
    refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    inbox: Arc<RefreshInbox>,
    cancelled: Arc<AtomicBool>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
    consecutive_failures: u32,
    retry_not_before: Option<OffsetDateTime>,
    next_scheduled_at: OffsetDateTime,
    next_network_poll_at: Instant,
    next_local_usage_catch_up_at: Instant,
    last_network_reachability: Option<bool>,
}

impl CoordinatorWorker {
    fn run(mut self, wake_receiver: Receiver<()>) {
        self.last_network_reachability = crate::network::is_reachable();
        while !self.inbox.stopping.load(Ordering::Acquire) {
            if self.inbox.paused.load(Ordering::Acquire) {
                match wake_receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            let schedule_wait = wait_until(self.clock.now(), self.next_scheduled_at);
            let network_wait = self
                .next_network_poll_at
                .saturating_duration_since(Instant::now());
            let local_usage_wait = self.local_usage_catch_up_wait().unwrap_or(Duration::MAX);
            match wake_receiver.recv_timeout(schedule_wait.min(network_wait).min(local_usage_wait))
            {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.inbox.stopping.load(Ordering::Acquire) {
                break;
            }

            self.poll_network_if_due();
            let now = self.clock.now();
            if now >= self.next_scheduled_at {
                self.inbox.record(RefreshSource::Schedule);
            }
            if self.local_usage_catch_up_is_due() {
                self.inbox.record(RefreshSource::LocalUsageCatchUp);
            }
            let sources = self.inbox.take_sources();
            if sources.is_empty() || !self.refresh_is_due(sources, now) {
                continue;
            }

            if !self.inbox.try_start_refresh() {
                self.inbox
                    .pending_sources
                    .fetch_or(sources.0, Ordering::AcqRel);
                continue;
            }
            let refresh_started = Instant::now();
            self.inbox
                .provider_settings_pending
                .store(false, Ordering::Release);
            let result = self.refresh_once(sources);
            // User requests that race with admission join this active attempt.
            // A provider notification can arrive after the full read, so keep
            // that source pending for a follow-up merge.
            while wake_receiver.try_recv().is_ok() {}
            // Drain first, then take joined sources. A request that arrives
            // after this take leaves its wake signal queued for the next loop.
            let joined_sources = self.inbox.take_sources();
            let manual_follow_up = joined_sources.contains(RefreshSource::Manual)
                && !sources.contains(RefreshSource::Manual);
            let provider_follow_up = joined_sources.contains(RefreshSource::ProviderNotification);
            let provider_settings_follow_up = self
                .inbox
                .provider_settings_pending
                .swap(false, Ordering::AcqRel);
            if provider_follow_up || manual_follow_up || provider_settings_follow_up {
                if manual_follow_up || provider_settings_follow_up {
                    self.inbox.record(RefreshSource::Manual);
                }
                if provider_follow_up {
                    self.inbox.record(RefreshSource::ProviderNotification);
                }
                let _ = self.inbox.wake.try_send(());
            }
            let notice = match result {
                RefreshRunResult::Completed { failed, notice } => {
                    if sources.contains(RefreshSource::LocalUsageCatchUp) {
                        self.record_local_usage_catch_up_result(failed, refresh_started.elapsed());
                    }
                    if !sources.is_only(RefreshSource::LocalUsageCatchUp) {
                        self.record_refresh_result(failed, self.clock.now());
                    }
                    notice
                }
                RefreshRunResult::Cancelled => None,
            };
            self.inbox.in_flight.store(false, Ordering::Release);
            if let Some(notice) = notice {
                self.subscribers.publish(notice);
            }
        }
    }

    fn local_usage_catch_up_wait(&self) -> Option<Duration> {
        self.projection
            .snapshot()
            .ok()
            .filter(|state| {
                state
                    .providers
                    .iter()
                    .any(|provider| provider.usage.scan_status == UsageScanStatus::Indexing)
            })
            .map(|_| {
                self.next_local_usage_catch_up_at
                    .saturating_duration_since(Instant::now())
            })
    }

    fn local_usage_catch_up_is_due(&self) -> bool {
        self.local_usage_catch_up_wait()
            .is_some_and(|wait| wait.is_zero())
    }

    fn record_local_usage_catch_up_result(&mut self, failed: bool, active_duration: Duration) {
        let delay = if failed {
            LOCAL_USAGE_CATCH_UP_ERROR_DELAY
        } else {
            active_duration
                .max(LOCAL_USAGE_CATCH_UP_DEFAULT_ACTIVE)
                .saturating_mul(4)
        };
        self.next_local_usage_catch_up_at = Instant::now() + delay;
        debug_local_usage_event(&format!(
            "catch_up_scheduled delay_ms={} active_ms={} failed={failed}",
            delay.as_millis(),
            active_duration.as_millis()
        ));
    }

    fn poll_network_if_due(&mut self) {
        if Instant::now() < self.next_network_poll_at {
            return;
        }
        let current = crate::network::is_reachable();
        if self.last_network_reachability == Some(false) && current == Some(true) {
            self.inbox.record(RefreshSource::NetworkRecovery);
        }
        if current.is_some() {
            self.last_network_reachability = current;
        }
        self.next_network_poll_at = Instant::now() + NETWORK_RECOVERY_POLL_INTERVAL;
    }

    fn refresh_is_due(&mut self, sources: RefreshSources, now: OffsetDateTime) -> bool {
        if sources.is_only(RefreshSource::LocalUsageCatchUp) {
            return true;
        }
        if let Some(retry_not_before) = self.retry_not_before
            && now < retry_not_before
        {
            self.next_scheduled_at = retry_not_before;
            return false;
        }
        if sources.contains_immediate_request() {
            return true;
        }
        if sources.contains(RefreshSource::StalePanelOpen)
            && self
                .projection
                .snapshot()
                .is_ok_and(|state| snapshot_needs_refresh(&state, now))
        {
            return true;
        }
        sources.contains(RefreshSource::Schedule) && now >= self.next_scheduled_at
    }

    fn refresh_once(&mut self, sources: RefreshSources) -> RefreshRunResult {
        let mut cached = match self.projection.snapshot() {
            Ok(cached) => cached,
            Err(_) => {
                return RefreshRunResult::Completed {
                    failed: true,
                    notice: None,
                };
            }
        };
        let mut pre_refresh_failed = false;
        if let Some(transitioned) = transition_snapshot_at(&cached, self.clock.now()) {
            let transition = self
                .store
                .lock()
                .map_err(|_| "native state unavailable")
                .and_then(|mut store| {
                    self.projection.commit_refreshed_snapshot(
                        &mut store,
                        transitioned,
                        self.enablement.as_ref(),
                        self.clock.now(),
                    )
                });
            match transition {
                Ok(outcome) => {
                    pre_refresh_failed = outcome.persistence_failed;
                    if let Some(notice) = outcome.notice {
                        self.subscribers.publish(notice);
                    }
                    match self.projection.snapshot() {
                        Ok(transitioned) => cached = transitioned,
                        Err(_) => pre_refresh_failed = true,
                    }
                }
                Err(_) => pre_refresh_failed = true,
            }
        }
        let attempt = RefreshAttempt::new(Arc::clone(&self.cancelled), sources);
        let observation = catch_unwind(AssertUnwindSafe(|| {
            self.refresh_adapter.refresh(cached.clone(), &attempt)
        }));
        if attempt.is_cancelled() {
            return RefreshRunResult::Cancelled;
        }
        let completed_at = self.clock.now();

        let (candidate, source_failed) = match observation {
            Ok(Ok(Some(refreshed))) if attempt.remaining().is_ok() => (
                Some(transition_snapshot_at(&refreshed, completed_at).unwrap_or(refreshed)),
                false,
            ),
            Ok(Ok(None)) if attempt.remaining().is_ok() => {
                (transition_snapshot_at(&cached, completed_at), false)
            }
            Ok(Err(RefreshFailure::Cancelled)) => return RefreshRunResult::Cancelled,
            Ok(Err(_)) | Ok(Ok(_)) | Err(_) => {
                (transition_snapshot_at(&cached, completed_at), true)
            }
        };
        if attempt.is_cancelled() {
            return RefreshRunResult::Cancelled;
        }

        let commit_result = candidate
            .map(|candidate| {
                let mut store = self.store.lock().map_err(|_| "native state unavailable")?;
                self.projection.commit_refreshed_snapshot(
                    &mut store,
                    candidate,
                    self.enablement.as_ref(),
                    completed_at,
                )
            })
            .transpose();
        let failed = pre_refresh_failed
            || source_failed
            || commit_result.is_err()
            || commit_result
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|outcome| outcome.persistence_failed);
        let notice = commit_result
            .ok()
            .flatten()
            .and_then(|outcome| outcome.notice);
        RefreshRunResult::Completed { failed, notice }
    }

    fn record_refresh_result(&mut self, failed: bool, now: OffsetDateTime) {
        self.next_scheduled_at = self
            .projection
            .snapshot()
            .map(|state| next_refresh_at(&state, now))
            .unwrap_or(now + to_time_duration(REFRESH_INTERVAL));
        if failed {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let retry_not_before =
                now + to_time_duration(refresh_backoff(self.consecutive_failures));
            self.retry_not_before = Some(retry_not_before);
            self.next_scheduled_at = self.next_scheduled_at.min(retry_not_before);
        } else {
            self.consecutive_failures = 0;
            self.retry_not_before = None;
        }
    }
}

struct NativeCoreInner {
    projection: Arc<CachedProjection>,
    store: Arc<Mutex<ReadModelStore>>,
    subscribers: Arc<RevisionSubscribers>,
    coordinator: RefreshCoordinator,
    clock: Arc<dyn Clock>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
}

#[derive(Clone)]
pub struct NativeCore {
    inner: Arc<NativeCoreInner>,
}

impl NativeCore {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        Self::open_with_provider_enablement(path, all_providers_enabled_policy())
    }

    pub(crate) fn open_with_provider_enablement(
        path: &Path,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Result<Self, &'static str> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let refresh_adapter = production_refresh_adapter(
            Arc::clone(&clock),
            Some(path.to_path_buf()),
            Arc::clone(&enablement),
        );
        Self::open_with_enablement(path, clock, refresh_adapter, enablement)
    }

    #[cfg(test)]
    fn open_with(
        path: &Path,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    ) -> Result<Self, &'static str> {
        Self::open_with_enablement(path, clock, refresh_adapter, all_providers_enabled_policy())
    }

    fn open_with_enablement(
        path: &Path,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Result<Self, &'static str> {
        let core =
            Self::open_without_launch_with_enablement(path, clock, refresh_adapter, enablement)?;
        // A failed coordinator must not discard a valid restored snapshot.
        let _ = core.request_refresh(RefreshSource::Launch);
        Ok(core)
    }

    #[cfg(test)]
    fn open_without_launch(
        path: &Path,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    ) -> Result<Self, &'static str> {
        Self::open_without_launch_with_enablement(
            path,
            clock,
            refresh_adapter,
            all_providers_enabled_policy(),
        )
    }

    fn open_without_launch_with_enablement(
        path: &Path,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Result<Self, &'static str> {
        let now = clock.now();
        let mut initial = unavailable_state_at(1, now);
        drop(initial.apply_provider_enablement(enablement.as_ref()));
        let (store, state) = SqliteReadModelStore::open(path, &initial)?;
        let mut store = ReadModelStore::Persistent(store);
        let projection = Arc::new(CachedProjection::new(state));
        if let Some(transitioned) = transition_snapshot_at(&projection.snapshot()?, now) {
            projection.commit_refreshed_snapshot(
                &mut store,
                transitioned,
                enablement.as_ref(),
                now,
            )?;
        }
        projection.commit_provider_enablement(&mut store, None, now)?;
        Ok(Self::from_components(
            projection,
            store,
            clock,
            refresh_adapter,
            enablement,
        ))
    }

    pub fn unavailable() -> Self {
        Self::unavailable_with_provider_enablement(all_providers_enabled_policy())
    }

    pub(crate) fn unavailable_with_provider_enablement(
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let refresh_adapter =
            production_refresh_adapter(Arc::clone(&clock), None, Arc::clone(&enablement));
        let mut state = unavailable_state_at(1, clock.now());
        drop(state.apply_provider_enablement(enablement.as_ref()));
        let core = Self::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            refresh_adapter,
            enablement,
        );
        let _ = core.request_refresh(RefreshSource::Launch);
        core
    }

    #[cfg(test)]
    pub(crate) fn test_unavailable() -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self::with_components(
            unavailable_state_at(1, clock.now()),
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            all_providers_enabled_policy(),
        )
    }

    #[cfg(test)]
    fn with_refresh_adapter(refresh_adapter: Arc<dyn SnapshotRefreshAdapter>) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self::with_components(
            unavailable_state_at(1, clock.now()),
            ReadModelStore::Memory,
            clock,
            refresh_adapter,
            all_providers_enabled_policy(),
        )
    }

    fn with_components(
        state: SanitizedDesktopStateV3,
        store: ReadModelStore,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        Self::from_components(
            Arc::new(CachedProjection::new(state)),
            store,
            clock,
            refresh_adapter,
            enablement,
        )
    }

    fn from_components(
        projection: Arc<CachedProjection>,
        store: ReadModelStore,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let subscribers = Arc::new(RevisionSubscribers::new());
        let store = Arc::new(Mutex::new(store));
        let coordinator = RefreshCoordinator::start(
            Arc::clone(&projection),
            Arc::clone(&store),
            Arc::clone(&subscribers),
            Arc::clone(&clock),
            refresh_adapter,
            Arc::clone(&enablement),
        );
        Self {
            inner: Arc::new(NativeCoreInner {
                projection,
                store,
                subscribers,
                coordinator,
                clock,
                enablement,
            }),
        }
    }

    /// Returns the complete panel projection with provider visibility already
    /// applied by the native policy.
    pub fn panel_state(&self) -> Result<SanitizedDesktopStateV3, &'static str> {
        let mut snapshot = self.inner.projection.snapshot()?;
        drop(snapshot.apply_provider_enablement(self.inner.enablement.as_ref()));
        snapshot.refresh_combined_usage();
        Ok(snapshot)
    }

    pub(crate) fn provider_enablement_changed(
        &self,
        provider: CodingProvider,
        enabled: bool,
    ) -> Result<(), &'static str> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let outcome = self.inner.projection.commit_provider_enablement(
            &mut store,
            Some((provider, enabled)),
            self.inner.clock.now(),
        )?;
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    pub fn set_profile_outcome(
        &self,
        profile: SanitizedProfileOutcome,
    ) -> Result<(), &'static str> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let outcome = self.inner.projection.commit_profile_outcome(
            &mut store,
            profile,
            self.inner.clock.now(),
        )?;
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    pub fn revision_notices(&self) -> Result<Receiver<RevisionNotice>, &'static str> {
        self.inner.subscribers.subscribe()
    }

    pub fn request_refresh(&self, source: RefreshSource) -> Result<RefreshReceipt, &'static str> {
        self.inner.coordinator.request(source)
    }

    pub(crate) fn request_provider_refresh(&self) -> Result<RefreshReceipt, &'static str> {
        self.inner.coordinator.request_provider_refresh()
    }

    pub(crate) fn shutdown(&self) {
        self.inner.coordinator.shutdown();
    }

    pub(crate) fn pause_for_update(&self) -> UpdatePauseGuard<'_> {
        self.inner.coordinator.pause_for_update()
    }

    pub(crate) fn flush(&self) -> Result<(), &'static str> {
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state persistence unavailable")?;
        match &*store {
            ReadModelStore::Persistent(store) => store.flush(),
            ReadModelStore::Memory => Ok(()),
        }
    }
}

fn read_model_backup_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!("sqlite3.read-model-v{source_version}.backup"))
}

fn read_model_backup_partial_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!(
        "sqlite3.read-model-v{source_version}.backup.partial"
    ))
}

fn read_model_schema_version(connection: &Connection) -> Result<i64, &'static str> {
    let schema_table_exists = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type = 'table' AND name = 'touchgrassbar_schema_versions'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| "native state persistence unavailable")?;
    if !schema_table_exists {
        return Ok(0);
    }
    connection
        .query_row(
            "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1",
            [READ_MODEL_SCHEMA_MODULE],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|version| version.unwrap_or(0))
        .map_err(|_| "native state persistence unavailable")
}

fn read_model_backup_is_valid(
    connection: &Connection,
    source_version: i64,
) -> Result<bool, &'static str> {
    if read_model_schema_version(connection)? != source_version {
        return Ok(false);
    }
    if source_version == 0 {
        return Ok(true);
    }
    let stored_versions = connection
        .query_row(
            "SELECT schema_version, contract_version
             FROM sanitized_desktop_state
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|_| "native state persistence unavailable")?;
    let expected_contract_version = match source_version {
        1 => 1,
        2 => 2,
        3 | 4 => i64::from(CONTRACT_VERSION),
        _ => return Ok(false),
    };
    Ok(stored_versions == (source_version, expected_contract_version))
}

fn backup_read_model_before_migration(
    connection: &Connection,
    path: &Path,
    source_version: i64,
) -> Result<(), &'static str> {
    let backup_path = read_model_backup_path(path, source_version);
    if backup_path.exists() {
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "native state persistence unavailable")?;
        return read_model_backup_is_valid(&backup, source_version)?
            .then_some(())
            .ok_or("native state persistence unavailable");
    }

    let partial_path = read_model_backup_partial_path(path, source_version);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| "native state persistence unavailable")?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &partial_path, None)
        .map_err(|_| "native state persistence unavailable")?;
    fs::File::open(&partial_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| "native state persistence unavailable")?;
    let backup =
        Connection::open_with_flags(&partial_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "native state persistence unavailable")?;
    if !read_model_backup_is_valid(&backup, source_version)? {
        return Err("native state persistence unavailable");
    }
    drop(backup);
    fs::rename(partial_path, backup_path).map_err(|_| "native state persistence unavailable")
}

fn wait_until(now: OffsetDateTime, deadline: OffsetDateTime) -> Duration {
    let remaining = deadline - now;
    if remaining.is_negative() || remaining.is_zero() {
        Duration::ZERO
    } else {
        Duration::from_millis(u64::try_from(remaining.whole_milliseconds()).unwrap_or(u64::MAX))
    }
}

fn to_time_duration(duration: Duration) -> TimeDuration {
    TimeDuration::seconds(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
}

fn refresh_backoff(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(6);
    let seconds = REFRESH_BACKOFF_BASE
        .as_secs()
        .saturating_mul(1_u64 << exponent)
        .min(REFRESH_BACKOFF_MAX.as_secs());
    Duration::from_secs(seconds)
}

fn timestamp_is_due(timestamp: &str, now: OffsetDateTime) -> bool {
    OffsetDateTime::parse(timestamp, &Rfc3339)
        .map(|observed_at| now - observed_at >= to_time_duration(REFRESH_INTERVAL))
        .unwrap_or(true)
}

fn freshness_deadline_after(timestamp: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339)
        .ok()
        .map(|observed_at| observed_at + to_time_duration(REFRESH_INTERVAL))
        .filter(|deadline| *deadline > now)
}

impl QuotaLane {
    fn is_valid_at(&self, now: OffsetDateTime) -> bool {
        self.reset_at.as_deref().is_none_or(|reset_at| {
            OffsetDateTime::parse(reset_at, &Rfc3339).is_ok_and(|reset_at| now < reset_at)
        })
    }

    fn next_reset_after(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.reset_at
            .as_deref()
            .and_then(|reset_at| OffsetDateTime::parse(reset_at, &Rfc3339).ok())
            .filter(|reset_at| *reset_at > now)
    }
}

impl ProviderSnapshot {
    pub(crate) fn provider(&self) -> CodingProvider {
        match self {
            Self::Unavailable { provider, .. }
            | Self::Current { provider, .. }
            | Self::Stale { provider, .. } => *provider,
        }
    }

    fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        match self {
            Self::Unavailable { .. } => false,
            Self::Stale { .. } => true,
            Self::Current {
                observed_at,
                quota_lanes,
                ..
            } => {
                timestamp_is_due(observed_at, now)
                    || quota_lanes.iter().any(|lane| !lane.is_valid_at(now))
            }
        }
    }

    fn transition_at(&self, now: OffsetDateTime) -> (Self, bool) {
        let (provider, observed_at, quota_lanes, current) = match self {
            Self::Unavailable { .. } => return (self.clone(), false),
            Self::Current {
                provider,
                observed_at,
                quota_lanes,
            } => (*provider, observed_at, quota_lanes, true),
            Self::Stale {
                provider,
                observed_at,
                quota_lanes,
            } => (*provider, observed_at, quota_lanes, false),
        };
        let valid_lanes = quota_lanes
            .iter()
            .filter(|lane| lane.is_valid_at(now))
            .cloned()
            .collect::<Vec<_>>();
        if valid_lanes.is_empty() {
            return (
                Self::Unavailable {
                    provider,
                    quota_lanes: [],
                },
                true,
            );
        }
        let becomes_stale = current && timestamp_is_due(observed_at, now);
        let changed = becomes_stale || valid_lanes.len() != quota_lanes.len();
        if current && !becomes_stale {
            (
                Self::Current {
                    provider,
                    observed_at: observed_at.clone(),
                    quota_lanes: valid_lanes,
                },
                changed,
            )
        } else {
            (
                Self::Stale {
                    provider,
                    observed_at: observed_at.clone(),
                    quota_lanes: valid_lanes,
                },
                changed,
            )
        }
    }

    fn next_transition_after(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        match self {
            Self::Unavailable { .. } => None,
            Self::Current {
                observed_at,
                quota_lanes,
                ..
            } => freshness_deadline_after(observed_at, now)
                .into_iter()
                .chain(
                    quota_lanes
                        .iter()
                        .filter_map(|lane| lane.next_reset_after(now)),
                )
                .min(),
            Self::Stale { quota_lanes, .. } => quota_lanes
                .iter()
                .filter_map(|lane| lane.next_reset_after(now))
                .min(),
        }
    }
}

impl UsageTotal {
    fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        match self {
            Self::Unavailable => false,
            Self::Stale { .. } => true,
            Self::Current { observed_at, .. } => timestamp_is_due(observed_at, now),
        }
    }

    fn transition_at(&self, now: OffsetDateTime) -> (Self, bool) {
        match self {
            Self::Current {
                evidence_basis,
                coverage,
                observed_at,
                observed_tokens,
                api_equivalent_cost_usd,
                trend_percent,
                trend_previous_tokens,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
            } if timestamp_is_due(observed_at, now) => (
                Self::Stale {
                    evidence_basis: *evidence_basis,
                    coverage: *coverage,
                    observed_at: observed_at.clone(),
                    observed_tokens: *observed_tokens,
                    api_equivalent_cost_usd: *api_equivalent_cost_usd,
                    trend_percent: *trend_percent,
                    trend_previous_tokens: *trend_previous_tokens,
                    api_equivalent_cost_basis: api_equivalent_cost_basis.clone(),
                    api_equivalent_cost_quality: *api_equivalent_cost_quality,
                    api_equivalent_cost_coverage_percent: *api_equivalent_cost_coverage_percent,
                },
                true,
            ),
            _ => (self.clone(), false),
        }
    }

    fn next_transition_after(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        match self {
            Self::Current { observed_at, .. } => freshness_deadline_after(observed_at, now),
            Self::Unavailable | Self::Stale { .. } => None,
        }
    }
}

impl ProviderPresentation {
    fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        self.quota.needs_refresh(now)
            || [
                &self.usage.today,
                &self.usage.seven_days,
                &self.usage.thirty_days,
            ]
            .into_iter()
            .any(|usage| usage.needs_refresh(now))
    }

    fn transition_at(&self, now: OffsetDateTime) -> (Self, bool) {
        let (quota, quota_changed) = self.quota.transition_at(now);
        let (usage, usage_changed) = transition_periods_at(&self.usage, now);
        let mut transitioned = self.clone();
        transitioned.quota = quota;
        transitioned.usage = usage;
        (transitioned, quota_changed || usage_changed)
    }

    fn next_transition_after(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.quota
            .next_transition_after(now)
            .into_iter()
            .chain(
                [
                    &self.usage.today,
                    &self.usage.seven_days,
                    &self.usage.thirty_days,
                ]
                .into_iter()
                .filter_map(|usage| usage.next_transition_after(now)),
            )
            .min()
    }
}

fn snapshot_needs_refresh(snapshot: &SanitizedDesktopStateV3, now: OffsetDateTime) -> bool {
    snapshot
        .providers
        .iter()
        .any(|provider| provider.needs_refresh(now))
}

fn transition_periods_at(periods: &UsagePeriods, now: OffsetDateTime) -> (UsagePeriods, bool) {
    let (today, today_changed) = periods.today.transition_at(now);
    let (seven_days, seven_days_changed) = periods.seven_days.transition_at(now);
    let (thirty_days, thirty_days_changed) = periods.thirty_days.transition_at(now);
    (
        UsagePeriods {
            scan_status: periods.scan_status,
            today_scan_status: periods.today_scan_status,
            seven_day_scan_status: periods.seven_day_scan_status,
            thirty_day_scan_status: periods.thirty_day_scan_status,
            today,
            seven_days,
            thirty_days,
        },
        today_changed || seven_days_changed || thirty_days_changed,
    )
}

fn transition_snapshot_at(
    snapshot: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
) -> Option<SanitizedDesktopStateV3> {
    let mut changed = false;
    let providers = snapshot
        .providers
        .iter()
        .map(|provider| {
            let (provider, provider_changed) = provider.transition_at(now);
            changed |= provider_changed;
            provider
        })
        .collect::<Vec<_>>();
    changed.then(|| {
        let mut transitioned = snapshot.clone();
        transitioned.providers = providers;
        transitioned.refresh_combined_usage();
        transitioned
    })
}

fn next_refresh_at(snapshot: &SanitizedDesktopStateV3, now: OffsetDateTime) -> OffsetDateTime {
    let provider_deadline = snapshot
        .providers
        .iter()
        .filter_map(|provider| provider.next_transition_after(now))
        .min();
    provider_deadline
        .unwrap_or(now + to_time_duration(REFRESH_INTERVAL))
        .min(now + to_time_duration(REFRESH_INTERVAL))
}

fn validate_snapshot(snapshot: &SanitizedDesktopStateV3) -> Result<(), &'static str> {
    if snapshot.contract_version != CONTRACT_VERSION
        || snapshot.revision.parse::<u64>().is_err()
        || OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339).is_err()
    {
        return Err("native state unavailable");
    }
    let mut registry_index = 0;
    for presentation in &snapshot.providers {
        while registry_index < PROVIDER_REGISTRY.len()
            && PROVIDER_REGISTRY[registry_index].provider != presentation.provider
        {
            registry_index += 1;
        }
        if registry_index == PROVIDER_REGISTRY.len() {
            return Err("native state unavailable");
        }
        registry_index += 1;
    }
    let quota_provider = |snapshot: &ProviderSnapshot| match snapshot {
        ProviderSnapshot::Unavailable { provider, .. }
        | ProviderSnapshot::Current { provider, .. }
        | ProviderSnapshot::Stale { provider, .. } => *provider,
    };
    let mut seen = std::collections::BTreeSet::new();
    for presentation in &snapshot.providers {
        if !seen.insert(presentation.provider)
            || quota_provider(&presentation.quota) != presentation.provider
            || presentation.display_name != provider_descriptor(presentation.provider).display_name
        {
            return Err("native state unavailable");
        }
        for usage in [
            &presentation.usage.today,
            &presentation.usage.seven_days,
            &presentation.usage.thirty_days,
        ] {
            validate_usage_total(usage)?;
        }
    }
    for usage in [
        &snapshot.combined_usage.today,
        &snapshot.combined_usage.seven_days,
        &snapshot.combined_usage.thirty_days,
    ] {
        validate_usage_total(usage)?;
    }
    let expected_combined = combine_usage_periods(
        &snapshot
            .providers
            .iter()
            .filter(|presentation| presentation.is_visible())
            .map(|presentation| &presentation.usage)
            .collect::<Vec<_>>(),
    );
    if snapshot.combined_usage != expected_combined {
        return Err("native state unavailable");
    }
    Ok(())
}

fn validate_usage_total(usage: &UsageTotal) -> Result<(), &'static str> {
    let (UsageTotal::Current {
        observed_at,
        api_equivalent_cost_usd,
        trend_percent,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
        ..
    }
    | UsageTotal::Stale {
        observed_at,
        api_equivalent_cost_usd,
        trend_percent,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
        ..
    }) = usage
    else {
        return Ok(());
    };
    if OffsetDateTime::parse(observed_at, &Rfc3339).is_err()
        || trend_percent.is_some_and(|value| !value.is_finite())
    {
        return Err("native state unavailable");
    }
    match (
        api_equivalent_cost_usd,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    ) {
        (None, None, None, None) => Ok(()),
        (Some(cost), Some(_), Some(ApiEquivalentCostQuality::Modeled), Some(coverage))
            if cost.is_finite()
                && *cost >= 0.0
                && coverage.is_finite()
                && (0.0..=100.0).contains(coverage) =>
        {
            Ok(())
        }
        (Some(cost), Some(_), Some(quality), None)
            if cost.is_finite()
                && *cost >= 0.0
                && matches!(
                    quality,
                    ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly
                ) =>
        {
            Ok(())
        }
        _ => Err("native state unavailable"),
    }
}

fn unavailable_periods() -> UsagePeriods {
    UsagePeriods {
        scan_status: UsageScanStatus::Unavailable,
        today_scan_status: UsageScanStatus::Unavailable,
        seven_day_scan_status: UsageScanStatus::Unavailable,
        thirty_day_scan_status: UsageScanStatus::Unavailable,
        today: UsageTotal::Unavailable,
        seven_days: UsageTotal::Unavailable,
        thirty_days: UsageTotal::Unavailable,
    }
}

fn format_time(now: OffsetDateTime) -> String {
    now.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

pub fn unavailable_state(revision: u64) -> SanitizedDesktopStateV3 {
    unavailable_state_at(revision, OffsetDateTime::now_utc())
}

fn unavailable_state_at(revision: u64, now: OffsetDateTime) -> SanitizedDesktopStateV3 {
    let providers = PROVIDER_REGISTRY
        .iter()
        .map(|descriptor| {
            let mut presentation = ProviderPresentation::unavailable(descriptor.provider);
            presentation.presence = detect_provider_presence(descriptor.provider);
            presentation
        })
        .collect::<Vec<_>>();
    let mut state = SanitizedDesktopStateV3 {
        contract_version: CONTRACT_VERSION,
        generated_at: format_time(now),
        revision: revision.max(1).to_string(),
        providers,
        combined_usage: unavailable_periods(),
        sync: SyncState {
            status: SyncStatus::Unavailable,
            last_successful_at: None,
        },
        profile: SanitizedProfileOutcome::NotAuthorized,
    };
    state.refresh_combined_usage();
    state
}

pub fn native_contract_schema() -> Schema {
    schema_for!(SanitizedDesktopStateV3)
}

pub fn native_contract_export() -> Value {
    json!({
        "bootstrapContractVersion": LIFECYCLE_CONTRACT_VERSION,
        "bootstrapStateSchema": bootstrap_state_schema(),
        "contractVersion": CONTRACT_VERSION,
        "panelAddTokenmaxxerEvent": PANEL_ADD_TOKENMAXXER_EVENT,
        "refreshReceiptSchema": schema_for!(RefreshReceipt),
        "revisionNoticeEvent": REVISION_NOTICE_EVENT,
        "revisionNoticeSchema": schema_for!(RevisionNotice),
        "settingsContractVersion": SETTINGS_CONTRACT_VERSION,
        "settingsNavigationEvent": SETTINGS_NAVIGATION_EVENT,
        "settingsNavigationSchema": settings_navigation_schema(),
        "settingsRecoveryClearEvent": SETTINGS_RECOVERY_CLEAR_EVENT,
        "settingsStateSchema": settings_state_schema(),
        "stateSchema": native_contract_schema(),
        "updateContractVersion": UPDATE_CONTRACT_VERSION,
        "updateStateChangedEvent": UPDATE_STATE_CHANGED_EVENT,
        "updateStateSchema": update_state_schema(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        env, fs,
        path::PathBuf,
        process,
        sync::{
            Barrier, Condvar,
            atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::{Value, json};

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
                "touchgrassbar-sanitized-{}-{timestamp}-{}.sqlite3",
                process::id(),
                NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            for path in [
                self.0.clone(),
                self.0.with_extension("sqlite3-shm"),
                self.0.with_extension("sqlite3-wal"),
                read_model_backup_path(&self.0, 0),
                read_model_backup_partial_path(&self.0, 0),
                read_model_backup_path(&self.0, 1),
                read_model_backup_partial_path(&self.0, 1),
                read_model_backup_path(&self.0, 2),
                read_model_backup_partial_path(&self.0, 2),
                read_model_backup_path(&self.0, 3),
                read_model_backup_partial_path(&self.0, 3),
                read_model_backup_path(&self.0, 4),
                read_model_backup_partial_path(&self.0, 4),
            ] {
                let _ = fs::remove_file(path);
            }
        }
    }

    struct FixtureClock {
        unix_seconds: AtomicI64,
    }

    impl FixtureClock {
        fn new(now: OffsetDateTime) -> Self {
            Self {
                unix_seconds: AtomicI64::new(now.unix_timestamp()),
            }
        }

        fn advance(&self, duration: Duration) {
            self.unix_seconds
                .fetch_add(i64::try_from(duration.as_secs()).unwrap(), Ordering::SeqCst);
        }
    }

    impl Clock for FixtureClock {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::from_unix_timestamp(self.unix_seconds.load(Ordering::SeqCst)).unwrap()
        }
    }

    struct ScriptedRefreshSource {
        responses: Mutex<VecDeque<Result<Option<SanitizedDesktopStateV3>, RefreshFailure>>>,
        runs: AtomicUsize,
        local_runs: AtomicUsize,
        completed: AtomicUsize,
        refresh_gate: Option<(usize, Arc<RefreshGate>)>,
        refresh_trigger: Mutex<Option<RefreshTrigger>>,
        clock: Option<Arc<FixtureClock>>,
        elapsed: Mutex<VecDeque<Duration>>,
    }

    struct RefreshGate {
        started: Barrier,
        release: Barrier,
    }

    impl RefreshGate {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                release: Barrier::new(2),
            }
        }
    }

    impl ScriptedRefreshSource {
        fn new(
            responses: impl IntoIterator<Item = Result<Option<SanitizedDesktopStateV3>, RefreshFailure>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                runs: AtomicUsize::new(0),
                local_runs: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                refresh_gate: None,
                refresh_trigger: Mutex::new(None),
                clock: None,
                elapsed: Mutex::new(VecDeque::new()),
            }
        }

        fn with_first_refresh_gate(mut self, gate: Arc<RefreshGate>) -> Self {
            self.refresh_gate = Some((0, gate));
            self
        }

        fn with_refresh_gate(mut self, run: usize, gate: Arc<RefreshGate>) -> Self {
            self.refresh_gate = Some((run, gate));
            self
        }

        fn with_elapsed(
            mut self,
            clock: Arc<FixtureClock>,
            elapsed: impl IntoIterator<Item = Duration>,
        ) -> Self {
            self.clock = Some(clock);
            self.elapsed = Mutex::new(elapsed.into_iter().collect());
            self
        }

        fn notify(&self) {
            self.refresh_trigger
                .lock()
                .unwrap()
                .as_ref()
                .expect("refresh trigger installed")();
        }
    }

    impl SnapshotRefreshAdapter for ScriptedRefreshSource {
        fn install_refresh_trigger(&self, trigger: RefreshTrigger) {
            *self.refresh_trigger.lock().unwrap() = Some(trigger);
        }

        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            attempt.remaining()?;
            if attempt.is_local_usage_only() {
                self.local_runs.fetch_add(1, Ordering::SeqCst);
            }
            let run = self.runs.fetch_add(1, Ordering::SeqCst);
            if let Some((blocked_run, gate)) = &self.refresh_gate
                && run == *blocked_run
            {
                gate.started.wait();
                gate.release.wait();
                attempt.remaining()?;
            }
            if let Some(elapsed) = self.elapsed.lock().unwrap().pop_front()
                && let Some(clock) = &self.clock
            {
                clock.advance(elapsed);
            }
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(None));
            self.completed.fetch_add(1, Ordering::SeqCst);
            response
        }
    }

    fn wait_for_completed_runs(source: &ScriptedRefreshSource, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while source.completed.load(Ordering::SeqCst) < expected {
            assert!(Instant::now() < deadline, "refresh did not complete");
            thread::yield_now();
        }
    }

    fn wait_for_idle(core: &NativeCore) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while core
            .inner
            .coordinator
            .inbox
            .in_flight
            .load(Ordering::Acquire)
        {
            assert!(
                Instant::now() < deadline,
                "refresh coordinator did not become idle"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn pause_for_update_closes_refresh_admission() {
        let (wake, _receiver) = std::sync::mpsc::sync_channel(1);
        let inbox = RefreshInbox {
            admission: Mutex::new(()),
            pending_sources: AtomicU8::new(0),
            provider_settings_pending: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            paused: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            wake,
        };

        assert!(!inbox.try_start_refresh());
        assert!(!inbox.in_flight.load(Ordering::Acquire));
    }

    fn observed_state(
        observed_at: OffsetDateTime,
        observed_tokens: u64,
    ) -> SanitizedDesktopStateV3 {
        let observed_at = format_time(observed_at);
        let codex_usage = UsagePeriods {
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens,
                api_equivalent_cost_usd: None,
                trend_percent: None,
                trend_previous_tokens: None,
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let mut state = SanitizedDesktopStateV3 {
            contract_version: CONTRACT_VERSION,
            generated_at: observed_at.clone(),
            revision: "1".to_owned(),
            providers: vec![
                ProviderPresentation {
                    provider: CodingProvider::Codex,
                    display_name: "Codex".to_owned(),
                    presence: ProviderPresenceStatus::Detected,
                    quota: ProviderSnapshot::Current {
                        provider: CodingProvider::Codex,
                        observed_at: observed_at.clone(),
                        quota_lanes: vec![QuotaLane {
                            label: "Weekly limit".to_owned(),
                            unit: "percent".to_owned(),
                            allowance: Some(100.0),
                            remaining: Some(74.0),
                            reset_at: None,
                        }],
                    },
                    usage: codex_usage,
                },
                ProviderPresentation {
                    provider: CodingProvider::Claude,
                    display_name: "Claude".to_owned(),
                    presence: ProviderPresenceStatus::NotDetected,
                    quota: ProviderSnapshot::Unavailable {
                        provider: CodingProvider::Claude,
                        quota_lanes: [],
                    },
                    usage: unavailable_periods(),
                },
            ],
            combined_usage: unavailable_periods(),
            sync: SyncState {
                status: SyncStatus::Unavailable,
                last_successful_at: None,
            },
            profile: SanitizedProfileOutcome::NotAuthorized,
        };
        state.refresh_combined_usage();
        state
    }

    fn legacy_observed_state_value(
        contract_version: u8,
        observed_at: OffsetDateTime,
        observed_tokens: u64,
    ) -> Value {
        let state = observed_state(observed_at, observed_tokens);
        let codex_usage = &state.provider(CodingProvider::Codex).unwrap().usage;
        let claude_usage = &state.provider(CodingProvider::Claude).unwrap().usage;
        let mut value = json!({
            "contractVersion": contract_version,
            "generatedAt": state.generated_at,
            "revision": state.revision,
            "providers": state
                .providers
                .iter()
                .map(|provider| &provider.quota)
                .collect::<Vec<_>>(),
            "usage": {
                "codex": codex_usage,
                "claude": claude_usage,
            },
            "sync": state.sync,
            "profile": state.profile,
        });
        if contract_version == 1 {
            value.as_object_mut().unwrap().remove("profile");
        }
        if contract_version <= 2 {
            for provider in ["codex", "claude"] {
                let periods = value["usage"][provider].as_object_mut().unwrap();
                periods.remove("scanStatus");
                periods.remove("todayScanStatus");
                periods.remove("sevenDayScanStatus");
                periods.remove("thirtyDayScanStatus");
            }
        }
        value
    }

    fn test_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_775_347_200).unwrap()
    }

    struct ClaudeTogglePolicy {
        enabled: AtomicBool,
    }

    impl ProviderEnablementPolicy for ClaudeTogglePolicy {
        fn is_provider_enabled(&self, provider: CodingProvider) -> bool {
            provider == CodingProvider::Codex || self.enabled.load(Ordering::Acquire)
        }
    }

    #[test]
    fn unavailable_snapshot_never_invents_zero_usage() {
        let value = serde_json::to_value(unavailable_state(1)).unwrap();
        assert_eq!(value["contractVersion"], CONTRACT_VERSION);
        assert_eq!(value["revision"], "1");
        let codex = value["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["provider"] == "codex")
            .unwrap();
        assert_eq!(
            codex["usage"]["today"],
            json!({ "availability": "unavailable" })
        );
        assert_eq!(
            value["combinedUsage"]["today"],
            json!({ "availability": "unavailable" })
        );
        assert!(value.to_string().find("observedTokens").is_none());
    }

    #[test]
    fn disabling_provider_keeps_unavailable_panel_card_and_excludes_usage_immediately() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let mut state = observed_state(test_time(), 42);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut state
            .provider_mut(CodingProvider::Codex)
            .unwrap()
            .usage
            .today
        else {
            panic!("Codex fixture usage must be current");
        };
        *api_equivalent_cost_usd = Some(4.2);
        *api_equivalent_cost_basis = Some("openai-fixture".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::Reconciled);
        let mut claude_usage = state.provider(CodingProvider::Codex).unwrap().usage.clone();
        let UsageTotal::Current {
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut claude_usage.today
        else {
            panic!("Claude fixture usage must be current");
        };
        *evidence_basis = UsageEvidenceBasis::LocallyDerived;
        *observed_tokens = 58;
        *api_equivalent_cost_usd = Some(5.8);
        *api_equivalent_cost_basis = Some("anthropic-fixture".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::LocalOnly);
        let claude = state.provider_mut(CodingProvider::Claude).unwrap();
        claude.presence = ProviderPresenceStatus::Detected;
        claude.quota = ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: format_time(test_time()),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(50.0),
                reset_at: None,
            }],
        };
        claude.usage = claude_usage;
        state.refresh_combined_usage();
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(true),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        );
        let notices = core.revision_notices().unwrap();

        policy.enabled.store(false, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();
        let cached = core.inner.projection.snapshot().unwrap();
        let panel = core.panel_state().unwrap();

        assert_eq!(panel.revision, notice.revision);
        assert_eq!(cached.providers.len(), 2);
        assert_eq!(cached.providers[0].provider, CodingProvider::Codex);
        assert_eq!(cached.providers[1].provider, CodingProvider::Claude);
        let cached_claude = cached.provider(CodingProvider::Claude).unwrap();
        assert!(matches!(
            cached_claude.quota,
            ProviderSnapshot::Current {
                provider: CodingProvider::Claude,
                ..
            }
        ));
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = &cached_claude.usage.today
        else {
            panic!("disabled provider history must remain cached");
        };
        assert_eq!(*observed_tokens, 58);
        assert_eq!(*api_equivalent_cost_usd, Some(5.8));
        assert_eq!(panel.providers.len(), 2);
        assert_eq!(panel.providers[0].provider, CodingProvider::Codex);
        assert_eq!(panel.providers[1].provider, CodingProvider::Claude);
        let claude = panel.provider(CodingProvider::Claude).unwrap();
        assert!(matches!(
            claude.quota,
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: []
            }
        ));
        assert_eq!(claude.usage, unavailable_periods());
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            ..
        } = panel.combined_usage.today
        else {
            panic!("Codex usage must remain in Combined");
        };
        assert_eq!(observed_tokens, 42);
        assert_eq!(api_equivalent_cost_usd, Some(4.2));
        assert_eq!(api_equivalent_cost_basis.as_deref(), Some("openai-fixture"));
    }

    #[test]
    fn panel_state_restores_disabled_provider_as_unavailable_in_registry_order() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let mut state = observed_state(test_time(), 42);
        state
            .providers
            .retain(|provider| provider.provider == CodingProvider::Codex);
        state.refresh_combined_usage();
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(false),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy;
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        );

        let panel = core.panel_state().unwrap();

        assert_eq!(panel.providers.len(), 2);
        assert_eq!(panel.providers[0].provider, CodingProvider::Codex);
        assert_eq!(panel.providers[1].provider, CodingProvider::Claude);
        let claude = panel.provider(CodingProvider::Claude).unwrap();
        assert!(matches!(
            claude.quota,
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: []
            }
        ));
        assert_eq!(claude.usage, unavailable_periods());
        assert_eq!(panel.combined_usage, panel.providers[0].usage);
    }

    #[test]
    fn enabling_provider_keeps_unavailable_panel_card_until_refresh_completes() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(false),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let mut state = unavailable_state(1);
        state.provider_mut(CodingProvider::Claude).unwrap().presence =
            ProviderPresenceStatus::NotDetected;
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        );
        assert_eq!(core.panel_state().unwrap().providers.len(), 2);

        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        let cached = core.inner.projection.snapshot().unwrap();
        let panel = core.panel_state().unwrap();

        assert_eq!(cached.providers.len(), 2);
        assert_eq!(panel.providers.len(), 2);
        let claude = panel.provider(CodingProvider::Claude).unwrap();
        assert!(matches!(
            claude.quota,
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: []
            }
        ));
        assert_eq!(claude.usage.scan_status, UsageScanStatus::Indexing);
        let mut reconnecting_usage = unavailable_periods();
        reconnecting_usage.scan_status = UsageScanStatus::Indexing;
        assert_eq!(claude.usage, reconnecting_usage);
    }

    #[test]
    fn reenabling_provider_keeps_last_known_cost_while_refresh_is_pending() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let mut state = observed_state(test_time(), 42);
        let mut claude_usage = state.provider(CodingProvider::Codex).unwrap().usage.clone();
        let UsageTotal::Current {
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut claude_usage.today
        else {
            panic!("Claude fixture usage must be current");
        };
        *evidence_basis = UsageEvidenceBasis::LocallyDerived;
        *observed_tokens = 58;
        *api_equivalent_cost_usd = Some(5.8);
        *api_equivalent_cost_basis = Some("anthropic-fixture".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::LocalOnly);
        let claude = state.provider_mut(CodingProvider::Claude).unwrap();
        claude.presence = ProviderPresenceStatus::Detected;
        claude.quota = ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: format_time(test_time()),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(50.0),
                reset_at: None,
            }],
        };
        claude.usage = claude_usage;
        state.refresh_combined_usage();
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(true),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        );

        policy.enabled.store(false, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        assert!(matches!(
            core.panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .today,
            UsageTotal::Unavailable
        ));

        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        let panel = core.panel_state().unwrap();
        let claude = panel.provider(CodingProvider::Claude).unwrap();

        assert_eq!(claude.usage.scan_status, UsageScanStatus::Indexing);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = claude.usage.today
        else {
            panic!("last known Claude usage must remain visible");
        };
        assert_eq!(observed_tokens, 58);
        assert_eq!(api_equivalent_cost_usd, Some(5.8));
    }

    #[test]
    fn refresh_commit_is_monotonic_and_notified_after_commit() {
        let database = TestDatabase::new();
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(ScriptedRefreshSource::new([Ok(Some(observed_state(
                test_time(),
                42,
            )))])),
        )
        .unwrap();
        let notices = core.revision_notices().unwrap();

        let receipt = core.request_refresh(RefreshSource::Manual).unwrap();
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();
        let after = core.panel_state().unwrap();

        assert!(receipt.accepted);
        assert_eq!(notice.revision, "2");
        assert_eq!(after.revision, notice.revision);
        assert!(matches!(
            &after.provider(CodingProvider::Codex).unwrap().usage.today,
            UsageTotal::Current { .. }
        ));
        assert_eq!(after.providers.len(), 2);
        assert!(matches!(
            after.provider(CodingProvider::Claude).unwrap().quota,
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: []
            }
        ));
    }

    #[test]
    fn claude_cli_observation_commits_native_panel_revision() {
        let database = TestDatabase::new();
        let now = test_time();
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(now));
        let refresh_adapter: Arc<dyn SnapshotRefreshAdapter> = Arc::new(
            crate::providers::test_claude_observation_coordinator(Arc::clone(&clock)),
        );
        let core = NativeCore::open_without_launch(&database.0, clock, refresh_adapter).unwrap();
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();
        let panel = core.panel_state().unwrap();
        let claude = panel.provider(CodingProvider::Claude).unwrap();

        assert_eq!(notice.revision, "2");
        assert_eq!(panel.revision, notice.revision);
        let ProviderSnapshot::Current { quota_lanes, .. } = &claude.quota else {
            panic!("Claude quota did not become current");
        };
        assert_eq!(quota_lanes.len(), 2);
        assert_eq!(quota_lanes[0].label, "5-hour limit");
        assert_eq!(quota_lanes[0].remaining, Some(76.5));
        assert_eq!(quota_lanes[1].label, "Weekly limit");
        assert_eq!(quota_lanes[1].remaining, Some(58.75));
    }

    #[test]
    fn refresh_drops_closed_notice_receivers_without_rejecting_work() {
        let core = NativeCore::with_refresh_adapter(Arc::new(ScriptedRefreshSource::new([Ok(
            Some(observed_state(test_time(), 42)),
        )])));
        drop(core.revision_notices().unwrap());
        let live_notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        live_notices.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(core.panel_state().unwrap().revision, "2");
    }

    struct BlockingRefreshSource {
        started: Barrier,
        release: Barrier,
        runs: AtomicUsize,
    }

    impl BlockingRefreshSource {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                release: Barrier::new(2),
                runs: AtomicUsize::new(0),
            }
        }
    }

    impl SnapshotRefreshAdapter for BlockingRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            attempt.remaining()?;
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.wait();
            self.release.wait();
            attempt.remaining()?;
            Ok(Some(observed_state(test_time(), 42)))
        }
    }

    struct BlockingStaleRegistryRefreshSource {
        started: Barrier,
        release: Barrier,
        runs: AtomicUsize,
    }

    impl BlockingStaleRegistryRefreshSource {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                release: Barrier::new(2),
                runs: AtomicUsize::new(0),
            }
        }
    }

    impl SnapshotRefreshAdapter for BlockingStaleRegistryRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            attempt.remaining()?;
            let run = self.runs.fetch_add(1, Ordering::SeqCst);
            if run < 2 {
                self.started.wait();
                self.release.wait();
            }
            attempt.remaining()?;
            let mut state = observed_state(test_time(), 42);
            if run == 0 {
                state
                    .providers
                    .retain(|provider| provider.provider == CodingProvider::Codex);
                state.refresh_combined_usage();
            }
            Ok(Some(state))
        }
    }

    #[test]
    fn concurrent_refresh_requests_join_one_in_flight_commit() {
        let source = Arc::new(BlockingRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        source.started.wait();

        assert_eq!(core.panel_state().unwrap().revision, "1");
        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        assert_eq!(source.runs.load(Ordering::SeqCst), 1);

        source.release.wait();
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(notice.revision, "2");
        assert_eq!(core.panel_state().unwrap().revision, "2");
        assert!(notices.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(source.runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn provider_setting_change_during_refresh_runs_with_the_latest_policy() {
        let source = Arc::new(BlockingRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());

        assert!(core.request_provider_refresh().unwrap().accepted);
        source.started.wait();

        assert!(core.request_provider_refresh().unwrap().accepted);
        assert_eq!(source.runs.load(Ordering::SeqCst), 1);
        source.release.wait();

        let deadline = Instant::now() + Duration::from_millis(250);
        while source.runs.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(source.runs.load(Ordering::SeqCst), 2);
        source.started.wait();
        source.release.wait();
    }

    #[test]
    fn stale_refresh_commit_preserves_reconnecting_provider_marker() {
        let source = Arc::new(BlockingStaleRegistryRefreshSource::new());
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(false),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let core = NativeCore::with_components(
            unavailable_state(1),
            ReadModelStore::Memory,
            Arc::new(FixtureClock::new(test_time())),
            source.clone(),
            enablement,
        );

        assert!(core.request_provider_refresh().unwrap().accepted);
        source.started.wait();

        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        assert!(core.request_provider_refresh().unwrap().accepted);
        let scan_status_before_stale_commit = core
            .panel_state()
            .unwrap()
            .provider(CodingProvider::Claude)
            .unwrap()
            .usage
            .scan_status;

        source.release.wait();
        source.started.wait();
        let scan_status = core
            .panel_state()
            .unwrap()
            .provider(CodingProvider::Claude)
            .unwrap()
            .usage
            .scan_status;
        source.release.wait();
        wait_for_idle(&core);

        assert_eq!(scan_status_before_stale_commit, UsageScanStatus::Indexing);
        assert_eq!(scan_status, UsageScanStatus::Indexing);
    }

    struct JoinedManualRefreshSource {
        started: Barrier,
        release: Barrier,
        manual_attempts: Mutex<Vec<bool>>,
    }

    impl JoinedManualRefreshSource {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                release: Barrier::new(2),
                manual_attempts: Mutex::new(Vec::new()),
            }
        }
    }

    impl SnapshotRefreshAdapter for JoinedManualRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            let first = {
                let mut attempts = self
                    .manual_attempts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                attempts.push(attempt.is_manual());
                attempts.len() == 1
            };
            if first {
                self.started.wait();
                self.release.wait();
            }
            attempt.remaining()?;
            Ok(Some(observed_state(test_time(), 42)))
        }
    }

    #[test]
    fn manual_request_joining_non_manual_refresh_runs_as_follow_up() {
        let source = Arc::new(JoinedManualRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::ProviderNotification)
                .unwrap()
                .accepted
        );
        source.started.wait();
        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        source.release.wait();

        notices.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while source
            .manual_attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            < 2
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(
            *source
                .manual_attempts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![false, true]
        );
    }

    struct ShutdownRefreshSource {
        started: Barrier,
        cancellation_observed: Barrier,
        release: Barrier,
    }

    impl ShutdownRefreshSource {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                cancellation_observed: Barrier::new(2),
                release: Barrier::new(2),
            }
        }
    }

    impl SnapshotRefreshAdapter for ShutdownRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            self.started.wait();
            while !attempt.is_cancelled() {
                std::thread::yield_now();
            }
            self.cancellation_observed.wait();
            self.release.wait();
            Err(RefreshFailure::Cancelled)
        }
    }

    #[test]
    fn shutdown_cancels_active_refresh_and_joins_the_coordinator() {
        let source = Arc::new(ShutdownRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());
        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        source.started.wait();

        let shutdown_core = core.clone();
        let (shutdown_complete, shutdown_completed) = mpsc::channel();
        let shutdown_thread = std::thread::spawn(move || {
            shutdown_core.shutdown();
            shutdown_complete.send(()).unwrap();
        });

        source.cancellation_observed.wait();
        assert!(
            shutdown_completed
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        source.release.wait();
        shutdown_thread.join().unwrap();
        shutdown_completed
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(core.request_refresh(RefreshSource::Manual).is_err());
    }

    struct ShutdownManagedRefreshSource {
        started: Barrier,
        released: (Mutex<bool>, Condvar),
        shutdown_called: AtomicBool,
    }

    impl ShutdownManagedRefreshSource {
        fn new() -> Self {
            Self {
                started: Barrier::new(2),
                released: (Mutex::new(false), Condvar::new()),
                shutdown_called: AtomicBool::new(false),
            }
        }

        fn release(&self) {
            let (released, changed) = &self.released;
            let mut released = released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *released = true;
            changed.notify_all();
        }
    }

    impl SnapshotRefreshAdapter for ShutdownManagedRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            _attempt: &RefreshAttempt,
        ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
            self.started.wait();
            let (released, changed) = &self.released;
            let mut released = released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = changed
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            Err(RefreshFailure::Cancelled)
        }

        fn shutdown(&self) {
            self.shutdown_called.store(true, Ordering::Release);
            self.release();
        }
    }

    #[test]
    fn shutdown_stops_adapter_resources_before_it_joins_the_worker() {
        let source = Arc::new(ShutdownManagedRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());
        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        source.started.wait();

        let shutdown_core = core.clone();
        let (complete, completed) = mpsc::channel();
        let shutdown_thread = std::thread::spawn(move || {
            shutdown_core.shutdown();
            let _ = complete.send(());
        });
        let result = completed.recv_timeout(Duration::from_secs(1));
        if result.is_err() {
            source.release();
        }
        shutdown_thread.join().unwrap();

        assert!(
            result.is_ok(),
            "adapter shutdown must run before worker join"
        );
        assert!(source.shutdown_called.load(Ordering::Acquire));
    }

    fn process_exists(pid: libc::pid_t) -> bool {
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[test]
    fn native_core_shutdown_stops_a_descendant_after_its_root_exits() {
        let (adapter, ready) = crate::providers::test_descendant_held_output_refresh_adapter();
        let core = NativeCore::with_refresh_adapter(adapter);
        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        let (root_pid, descendant_pid) = ready
            .recv_timeout(Duration::from_secs(2))
            .expect("the provider fixture must reach its orphaned descendant state");
        assert!(process_exists(root_pid));
        assert!(process_exists(descendant_pid));

        let shutdown_core = core.clone();
        let (complete, completed) = mpsc::channel();
        let started = Instant::now();
        let shutdown_thread = std::thread::spawn(move || {
            shutdown_core.shutdown();
            let _ = complete.send(());
        });
        completed
            .recv_timeout(Duration::from_secs(2))
            .expect("native shutdown must not wait on descendant-owned output");
        shutdown_thread.join().unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(2);
        while (process_exists(root_pid) || process_exists(descendant_pid))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!process_exists(root_pid));
        assert!(!process_exists(descendant_pid));
    }

    #[test]
    #[ignore = "subprocess fixture"]
    fn crash_writer_fixture() {
        let Some(database_path) = env::var_os("TOUCHGRASS_CRASH_DB_PATH") else {
            return;
        };
        let connection = Connection::open(database_path).unwrap();
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 UPDATE sanitized_desktop_state
                 SET revision = '999'
                 WHERE singleton = 1;",
            )
            .unwrap();
        process::exit(97);
    }

    #[test]
    fn restores_cached_snapshot_after_interrupted_transaction_without_panel_io() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(observed_state(
            test_time(),
            42,
        )))]));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "2"
        );
        drop(core);

        let crash_status = process::Command::new(env::current_exe().unwrap())
            .arg("--ignored")
            .arg("--exact")
            .arg("sanitized::tests::crash_writer_fixture")
            .env("TOUCHGRASS_CRASH_DB_PATH", &database.0)
            .status()
            .unwrap();
        assert_eq!(crash_status.code(), Some(97));

        let relaunched = NativeCore::open_without_launch(
            &database.0,
            clock,
            Arc::new(ScriptedRefreshSource::new([])),
        )
        .unwrap();
        fs::remove_file(&database.0).unwrap();
        let cached = relaunched.panel_state().unwrap();

        assert_eq!(cached.revision, "2");
        assert!(matches!(
            &cached.provider(CodingProvider::Codex).unwrap().usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
    }

    #[test]
    fn migrates_contract_v1_cache_without_losing_provider_state() {
        let database = TestDatabase::new();
        let legacy = legacy_observed_state_value(1, test_time(), 42);
        let legacy_json = serde_json::to_string(&legacy).unwrap();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 1),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 1),
                   revision TEXT NOT NULL,
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version)
                 VALUES (?1, 1)",
                [READ_MODEL_SCHEMA_MODULE],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, 1, 1, '1', ?1)",
                [legacy_json],
            )
            .unwrap();
        drop(connection);
        let older_backup = read_model_backup_path(&database.0, 0);
        fs::write(&older_backup, b"existing version-zero backup").unwrap();
        let migration_backup = read_model_backup_path(&database.0, 1);
        fs::write(&migration_backup, b"incomplete version-one backup").unwrap();

        assert!(
            NativeCore::open_without_launch(
                &database.0,
                Arc::new(FixtureClock::new(test_time())),
                Arc::new(CachedProjectionRefreshAdapter),
            )
            .is_err()
        );
        let source = Connection::open(&database.0).unwrap();
        let source_versions = source
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u8>(1)?)),
            )
            .unwrap();
        assert_eq!(source_versions, (1, 1));
        drop(source);
        fs::remove_file(&migration_backup).unwrap();
        let partial_backup = read_model_backup_partial_path(&database.0, 1);
        fs::write(&partial_backup, b"interrupted version-one backup").unwrap();

        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        let restored = core.panel_state().unwrap();

        assert_eq!(restored.contract_version, CONTRACT_VERSION);
        assert_eq!(restored.profile, SanitizedProfileOutcome::NotAuthorized);
        assert!(matches!(
            &restored
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        let connection = Connection::open(&database.0).unwrap();
        let versions = connection
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u8>(1)?)),
            )
            .unwrap();
        assert_eq!(versions, (READ_MODEL_SCHEMA_VERSION, CONTRACT_VERSION));
        assert!(migration_backup.is_file());
        assert!(!partial_backup.exists());
        let backup = Connection::open(migration_backup).unwrap();
        let backup_versions = backup
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u8>(1)?)),
            )
            .unwrap();
        assert_eq!(backup_versions, (1, 1));
        assert_eq!(
            fs::read(older_backup).unwrap(),
            b"existing version-zero backup"
        );
    }

    #[test]
    fn migrates_v2_cache_without_resetting_the_sanitized_snapshot() {
        let database = TestDatabase::new();
        let legacy = legacy_observed_state_value(2, test_time(), 42);
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 2),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 2),
                   revision TEXT NOT NULL,
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version) VALUES (?1, 2)",
                [READ_MODEL_SCHEMA_MODULE],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, 2, 2, '1', ?1)",
                [legacy.to_string()],
            )
            .unwrap();
        drop(connection);

        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        let migrated = core.panel_state().unwrap();
        assert_eq!(migrated.revision, "1");
        assert!(matches!(
            &migrated
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        assert_eq!(
            migrated
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Unavailable
        );
        let connection = Connection::open(&database.0).unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT schema_version FROM sanitized_desktop_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, READ_MODEL_SCHEMA_VERSION);
        assert!(read_model_backup_path(&database.0, 2).is_file());
    }

    #[test]
    fn migrates_fixed_v3_cache_to_dynamic_providers_without_a_refresh() {
        let database = TestDatabase::new();
        let mut legacy = legacy_observed_state_value(3, test_time(), 42);
        legacy["usage"]["codex"]["scanStatus"] = json!("complete");
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 3),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 3),
                   revision TEXT NOT NULL,
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version) VALUES (?1, 3)",
                [READ_MODEL_SCHEMA_MODULE],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, 3, 3, '1', ?1)",
                [legacy.to_string()],
            )
            .unwrap();
        drop(connection);

        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        let migrated = core.panel_state().unwrap();

        assert_eq!(migrated.revision, "1");
        let codex = migrated.provider(CodingProvider::Codex).unwrap();
        assert_eq!(codex.display_name, "Codex");
        assert_eq!(codex.usage.scan_status, UsageScanStatus::Complete);
        assert!(matches!(
            &codex.usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        assert!(matches!(
            &migrated.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        assert!(read_model_backup_path(&database.0, 3).is_file());
    }

    #[test]
    fn migration_failure_preserves_the_existing_database_and_backup() {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE migration_sentinel (value TEXT NOT NULL);
                 INSERT INTO migration_sentinel (value) VALUES ('keep');
                 CREATE TABLE sanitized_desktop_state (broken TEXT);",
            )
            .unwrap();
        drop(connection);

        assert!(
            NativeCore::open_without_launch(
                &database.0,
                Arc::new(FixtureClock::new(test_time())),
                Arc::new(ScriptedRefreshSource::new([])),
            )
            .is_err()
        );

        let connection = Connection::open(&database.0).unwrap();
        let sentinel: String = connection
            .query_row("SELECT value FROM migration_sentinel", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sentinel, "keep");
        assert!(read_model_backup_path(&database.0, 0).is_file());
    }

    #[test]
    fn every_refresh_trigger_uses_the_fake_clock_coordinator() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let launch_gate = Arc::new(RefreshGate::new());
        let source = Arc::new(
            ScriptedRefreshSource::new([
                Ok(Some(observed_state(test_time(), 1))),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(5),
                    2,
                ))),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(5),
                    3,
                ))),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(5),
                    4,
                ))),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(5),
                    5,
                ))),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(10),
                    6,
                ))),
            ])
            .with_first_refresh_gate(Arc::clone(&launch_gate)),
        );
        let core = NativeCore::open_with(&database.0, clock.clone(), source.clone()).unwrap();
        launch_gate.started.wait();
        let notices = core.revision_notices().unwrap();
        launch_gate.release.wait();

        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "2"
        );

        clock.advance(REFRESH_INTERVAL);
        assert!(
            core.request_refresh(RefreshSource::StalePanelOpen)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "3"
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "4"
        );
        wait_for_completed_runs(&source, 2);

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "5"
        );
        wait_for_completed_runs(&source, 3);
        assert!(core.request_refresh(RefreshSource::Wake).unwrap().accepted);
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "6"
        );
        wait_for_completed_runs(&source, 4);
        assert!(
            core.request_refresh(RefreshSource::NetworkRecovery)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "7"
        );
        wait_for_completed_runs(&source, 5);

        clock.advance(REFRESH_INTERVAL - Duration::from_secs(1));
        assert!(
            core.request_refresh(RefreshSource::Schedule)
                .unwrap()
                .accepted
        );
        assert!(notices.recv_timeout(Duration::from_millis(50)).is_err());
        clock.advance(Duration::from_secs(1));
        assert!(
            core.request_refresh(RefreshSource::Schedule)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "8"
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "9"
        );
        wait_for_completed_runs(&source, 6);
        assert_eq!(source.runs.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn provider_notification_requests_a_sanitized_refresh() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(test_time(), 1))),
            Ok(Some(observed_state(
                test_time() + time::Duration::seconds(1),
                2,
            ))),
        ]));
        let core =
            NativeCore::open_without_launch(&database.0, clock.clone(), source.clone()).unwrap();
        let notices = core.revision_notices().unwrap();

        core.request_refresh(RefreshSource::Launch).unwrap();
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "2"
        );
        wait_for_completed_runs(&source, 1);
        wait_for_idle(&core);

        clock.advance(Duration::from_secs(1));
        source.notify();
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "3"
        );
        assert!(matches!(
            &core
                .panel_state()
                .unwrap()
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 2,
                ..
            }
        ));
        assert_eq!(source.runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn any_provider_indexing_starts_an_independent_local_catch_up_pass() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let mut indexing = observed_state(test_time(), 42);
        let indexing_claude = indexing.provider_mut(CodingProvider::Claude).unwrap();
        indexing_claude.presence = ProviderPresenceStatus::Detected;
        indexing_claude.usage.scan_status = UsageScanStatus::Indexing;
        indexing.refresh_combined_usage();
        let mut complete = observed_state(test_time(), 42);
        let complete_claude = complete.provider_mut(CodingProvider::Claude).unwrap();
        complete_claude.presence = ProviderPresenceStatus::Detected;
        complete_claude.usage.scan_status = UsageScanStatus::Complete;
        complete.refresh_combined_usage();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(indexing)),
            Ok(Some(complete)),
        ]));
        let core = NativeCore::open_without_launch(&database.0, clock, source.clone()).unwrap();

        core.request_refresh(RefreshSource::Launch).unwrap();
        wait_for_completed_runs(&source, 2);
        wait_for_idle(&core);

        assert_eq!(source.local_runs.load(Ordering::SeqCst), 1);
        assert_eq!(
            core.panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Complete
        );
    }

    #[test]
    fn offline_and_persistence_failures_preserve_stale_values_and_back_off() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let source = Arc::new(
            ScriptedRefreshSource::new([
                Ok(Some(observed_state(test_time(), 42))),
                Err(RefreshFailure::SourceUnavailable),
                Ok(Some(observed_state(
                    test_time() + time::Duration::minutes(5),
                    43,
                ))),
            ])
            .with_elapsed(
                Arc::clone(&clock),
                [Duration::ZERO, REFRESH_BACKOFF_BASE * 2],
            ),
        );
        let core =
            NativeCore::open_without_launch(&database.0, clock.clone(), source.clone()).unwrap();
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Launch)
                .unwrap()
                .accepted
        );
        notices.recv_timeout(Duration::from_secs(1)).unwrap();
        Connection::open(&database.0)
            .unwrap()
            .execute_batch("DROP TABLE sanitized_desktop_state;")
            .unwrap();
        clock.advance(REFRESH_INTERVAL);

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "3"
        );
        let stale = core.panel_state().unwrap();
        assert!(matches!(
            &stale.providers[0].quota,
            ProviderSnapshot::Stale { .. }
        ));
        assert!(matches!(
            &stale.provider(CodingProvider::Codex).unwrap().usage.today,
            UsageTotal::Stale {
                observed_tokens: 42,
                ..
            }
        ));
        assert_eq!(stale.sync.status, SyncStatus::Unavailable);
        wait_for_completed_runs(&source, 2);
        wait_for_idle(&core);

        assert!(
            core.request_refresh(RefreshSource::NetworkRecovery)
                .unwrap()
                .accepted
        );
        assert_eq!(source.runs.load(Ordering::SeqCst), 2);
        clock.advance(REFRESH_BACKOFF_BASE);
        assert!(
            core.request_refresh(RefreshSource::Schedule)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "4"
        );
        assert!(matches!(
            &core
                .panel_state()
                .unwrap()
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 43,
                ..
            }
        ));
        assert_eq!(
            core.panel_state().unwrap().sync.status,
            SyncStatus::Unavailable
        );
        assert_eq!(source.runs.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn freshness_transitions_before_a_provider_read_can_block() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let gate = Arc::new(RefreshGate::new());
        let source = Arc::new(
            ScriptedRefreshSource::new([
                Ok(Some(observed_state(test_time(), 42))),
                Err(RefreshFailure::SourceUnavailable),
            ])
            .with_refresh_gate(1, Arc::clone(&gate)),
        );
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        let notices = core.revision_notices().unwrap();

        core.request_refresh(RefreshSource::Launch).unwrap();
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "2"
        );
        clock.advance(REFRESH_INTERVAL);
        core.request_refresh(RefreshSource::Manual).unwrap();
        gate.started.wait();

        let transition_notice = notices.recv_timeout(Duration::from_millis(100));
        let state_during_refresh = core.panel_state().unwrap();
        gate.release.wait();

        assert_eq!(transition_notice.unwrap().revision, "3");
        assert!(matches!(
            &state_during_refresh.providers[0].quota,
            ProviderSnapshot::Stale { .. }
        ));
    }

    #[test]
    fn reset_deadline_expires_only_the_affected_quota_lane() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let mut observed = observed_state(test_time(), 42);
        if let ProviderSnapshot::Current { quota_lanes, .. } = &mut observed.providers[0].quota {
            quota_lanes[0].reset_at = Some(format_time(test_time() + TimeDuration::minutes(2)));
        }
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(observed)), Ok(None)]));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Launch)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "2"
        );

        clock.advance(Duration::from_secs(2 * 60));
        assert!(
            core.request_refresh(RefreshSource::Schedule)
                .unwrap()
                .accepted
        );
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "3"
        );
        let after_reset = core.panel_state().unwrap();
        assert!(matches!(
            &after_reset.providers[0].quota,
            ProviderSnapshot::Unavailable { .. }
        ));
        assert!(matches!(
            &after_reset
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
    }

    #[test]
    fn sanitized_snapshot_cannot_contain_privileged_field_names() {
        let value = serde_json::to_value(unavailable_state(1)).unwrap();
        let prohibited = [
            "credential",
            "cookie",
            "path",
            "prompt",
            "raw",
            "session",
            "tokenmaxxerId",
        ];

        fn assert_clean(value: &Value, prohibited: &[&str]) {
            match value {
                Value::Object(fields) => {
                    for (key, child) in fields {
                        let normalized = key.to_lowercase();
                        assert!(
                            prohibited.iter().all(|word| !normalized.contains(word)),
                            "prohibited field serialized: {key}"
                        );
                        assert_clean(child, prohibited);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        assert_clean(child, prohibited);
                    }
                }
                _ => {}
            }
        }

        assert_clean(&value, &prohibited);
    }
}
