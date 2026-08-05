use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, params};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::lifecycle::{
    LIFECYCLE_CONTRACT_VERSION, SETTINGS_NAVIGATION_EVENT, bootstrap_state_schema,
    settings_navigation_schema, settings_state_schema,
};

pub const CONTRACT_VERSION: u8 = 1;
pub const REVISION_NOTICE_EVENT: &str = "sanitized-desktop-state-revision";
const READ_MODEL_SCHEMA_VERSION: i64 = 1;
const READ_MODEL_SCHEMA_MODULE: &str = "sanitized-desktop-state";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REFRESH_BACKOFF_BASE: Duration = Duration::from_secs(5);
const REFRESH_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDesktopStateV1 {
    pub contract_version: u8,
    pub generated_at: String,
    pub revision: String,
    pub providers: [ProviderSnapshot; 2],
    pub usage: UsageByProvider,
    pub sync: SyncState,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodingProvider {
    Codex,
    Claude,
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
pub struct UsageByProvider {
    pub codex: UsagePeriods,
    pub claude: UsagePeriods,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriods {
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
    },
    Stale {
        evidence_basis: UsageEvidenceBasis,
        coverage: UsageCoverage,
        observed_at: String,
        observed_tokens: u64,
        api_equivalent_cost_usd: Option<f64>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageEvidenceBasis {
    ProviderReported,
    LocallyDerived,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageCoverage {
    Complete,
    Partial,
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

trait SnapshotRefreshAdapter: Send + Sync {
    fn refresh(
        &self,
        cached: SanitizedDesktopStateV1,
    ) -> Result<Option<SanitizedDesktopStateV1>, &'static str>;
}

struct CachedProjectionRefreshAdapter;

impl SnapshotRefreshAdapter for CachedProjectionRefreshAdapter {
    fn refresh(
        &self,
        _cached: SanitizedDesktopStateV1,
    ) -> Result<Option<SanitizedDesktopStateV1>, &'static str> {
        // Provider observation is not wired yet. An unchanged cached projection
        // does not create a false revision or notice.
        Ok(None)
    }
}

trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

struct SqliteReadModelStore {
    connection: Mutex<Connection>,
}

impl SqliteReadModelStore {
    fn open(
        path: &Path,
        initial: &SanitizedDesktopStateV1,
    ) -> Result<(Self, SanitizedDesktopStateV1), &'static str> {
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
        Ok((
            Self {
                connection: Mutex::new(connection),
            },
            state,
        ))
    }

    fn migrate(
        connection: &mut Connection,
        path: &Path,
        initial: &SanitizedDesktopStateV1,
    ) -> Result<(), &'static str> {
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
        let version = if schema_table_exists {
            connection
                .query_row(
                    "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1",
                    [READ_MODEL_SCHEMA_MODULE],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|_| "native state persistence unavailable")?
                .unwrap_or(0)
        } else {
            0
        };

        if version > READ_MODEL_SCHEMA_VERSION {
            return Err("native state persistence unavailable");
        }
        if version == READ_MODEL_SCHEMA_VERSION {
            return Ok(());
        }

        let backup_path = read_model_backup_path(path);
        if !backup_path.exists() {
            connection
                .backup(rusqlite::MAIN_DB, &backup_path, None)
                .map_err(|_| "native state persistence unavailable")?;
        }

        let snapshot_json =
            serde_json::to_string(initial).map_err(|_| "native state persistence unavailable")?;
        let transaction = connection
            .transaction()
            .map_err(|_| "native state persistence unavailable")?;
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 1),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 1),
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
                    initial.revision,
                    snapshot_json
                ],
            )
            .map_err(|_| "native state persistence unavailable")?;
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

    fn read_from(connection: &Connection) -> Result<SanitizedDesktopStateV1, &'static str> {
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
        let snapshot: SanitizedDesktopStateV1 = serde_json::from_str(&snapshot_json)
            .map_err(|_| "native state persistence unavailable")?;
        validate_snapshot(&snapshot)?;
        if snapshot.revision != revision {
            return Err("native state persistence unavailable");
        }
        Ok(snapshot)
    }

    fn commit(&self, state: &SanitizedDesktopStateV1) -> Result<(), &'static str> {
        validate_snapshot(state)?;
        let snapshot_json =
            serde_json::to_string(state).map_err(|_| "native state persistence unavailable")?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "native state persistence unavailable")?;
        let transaction = connection
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
}

enum ReadModelStore {
    Persistent(SqliteReadModelStore),
    Memory,
}

struct SnapshotCommitOutcome {
    notice: Option<RevisionNotice>,
    persistence_failed: bool,
}

struct RefreshCoordinatorState {
    consecutive_failures: u32,
    in_flight: bool,
    next_scheduled_at: OffsetDateTime,
    retry_not_before: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshSource {
    Launch,
    StalePanelOpen,
    Manual,
    Wake,
    NetworkRecovery,
    Schedule,
}

struct NativeCoreInner {
    state: Mutex<SanitizedDesktopStateV1>,
    store: Mutex<ReadModelStore>,
    revision_subscribers: Mutex<Vec<Sender<RevisionNotice>>>,
    refresh_coordinator: Mutex<RefreshCoordinatorState>,
    refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    clock: Arc<dyn Clock>,
    scheduler_started: AtomicBool,
    scheduler_thread: Mutex<Option<thread::Thread>>,
}

impl NativeCoreInner {
    fn run_refresh(&self) {
        let now = self.clock.now();
        let cached = self.state.lock().map(|state| state.clone());
        let mut source_failed = false;
        let candidate = match cached.as_ref() {
            Ok(cached) => match catch_unwind(AssertUnwindSafe(|| {
                self.refresh_adapter.refresh(cached.clone())
            })) {
                Ok(Ok(Some(refreshed))) => {
                    Some(transition_snapshot_at(&refreshed, now).unwrap_or(refreshed))
                }
                Ok(Ok(None)) => transition_snapshot_at(cached, now),
                Ok(Err(_)) | Err(_) => {
                    source_failed = true;
                    transition_snapshot_at(cached, now)
                }
            },
            Err(_) => {
                source_failed = true;
                None
            }
        };

        let commit_result = candidate
            .map(|candidate| self.commit_refreshed_snapshot(candidate, now))
            .transpose();
        let failed = source_failed
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
        self.finish_refresh(failed, now);
        if let Some(notice) = notice {
            self.publish_notice(notice);
        }
    }

    fn commit_refreshed_snapshot(
        &self,
        mut refreshed: SanitizedDesktopStateV1,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let cached = self
            .state
            .lock()
            .map_err(|_| "native state unavailable")?
            .clone();
        refreshed.contract_version = CONTRACT_VERSION;
        refreshed.generated_at.clone_from(&cached.generated_at);
        refreshed.revision.clone_from(&cached.revision);
        let memory_only = self
            .store
            .lock()
            .map_err(|_| "native state persistence unavailable")
            .map(|store| matches!(*store, ReadModelStore::Memory))?;
        if memory_only {
            refreshed.sync.status = SyncStatus::Unavailable;
        }
        validate_snapshot(&refreshed)?;
        if refreshed == cached {
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
        let mut store = self
            .store
            .lock()
            .map_err(|_| "native state persistence unavailable")?;
        let persistence_failed = match &*store {
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
        drop(store);
        validate_snapshot(&refreshed)?;
        *self.state.lock().map_err(|_| "native state unavailable")? = refreshed;
        Ok(SnapshotCommitOutcome {
            notice: Some(RevisionNotice {
                revision: revision.to_string(),
            }),
            persistence_failed,
        })
    }

    fn publish_notice(&self, notice: RevisionNotice) {
        let Ok(mut subscribers) = self.revision_subscribers.lock() else {
            return;
        };
        subscribers.retain(|subscriber| subscriber.send(notice.clone()).is_ok());
    }

    fn finish_refresh(&self, failed: bool, now: OffsetDateTime) {
        let next_refresh_at = self
            .state
            .lock()
            .map(|state| next_refresh_at(&state, now))
            .unwrap_or(now + to_time_duration(REFRESH_INTERVAL));
        let Ok(mut coordinator) = self.refresh_coordinator.lock() else {
            return;
        };
        coordinator.in_flight = false;
        coordinator.next_scheduled_at = next_refresh_at;
        if failed {
            coordinator.consecutive_failures = coordinator.consecutive_failures.saturating_add(1);
            let backoff = refresh_backoff(coordinator.consecutive_failures);
            let retry_not_before = now + to_time_duration(backoff);
            coordinator.retry_not_before = Some(retry_not_before);
            coordinator.next_scheduled_at = coordinator.next_scheduled_at.min(retry_not_before);
        } else {
            coordinator.consecutive_failures = 0;
            coordinator.retry_not_before = None;
        }
        drop(coordinator);
        self.wake_scheduler();
    }

    fn begin_refresh(&self, source: RefreshSource) -> Result<bool, &'static str> {
        let now = self.clock.now();
        let mut coordinator = self
            .refresh_coordinator
            .lock()
            .map_err(|_| "refresh coordinator unavailable")?;
        if coordinator.in_flight {
            return Ok(false);
        }
        if let Some(retry_not_before) = coordinator.retry_not_before
            && now < retry_not_before
        {
            if source == RefreshSource::NetworkRecovery {
                coordinator.next_scheduled_at = coordinator.next_scheduled_at.min(retry_not_before);
                drop(coordinator);
                self.wake_scheduler();
            }
            return Ok(false);
        }
        if source == RefreshSource::Schedule && now < coordinator.next_scheduled_at {
            return Ok(false);
        }
        coordinator.in_flight = true;
        coordinator.next_scheduled_at = now + to_time_duration(REFRESH_INTERVAL);
        Ok(true)
    }

    fn schedule_wait(&self) -> Duration {
        let Ok(coordinator) = self.refresh_coordinator.lock() else {
            return Duration::from_secs(1);
        };
        let remaining = coordinator.next_scheduled_at - self.clock.now();
        if remaining.is_negative() || remaining.is_zero() {
            Duration::ZERO
        } else {
            Duration::from_secs(u64::try_from(remaining.whole_seconds()).unwrap_or(1))
        }
    }

    fn wake_scheduler(&self) {
        if let Ok(scheduler) = self.scheduler_thread.lock()
            && let Some(scheduler) = scheduler.as_ref()
        {
            scheduler.unpark();
        }
    }
}

#[derive(Clone)]
pub struct NativeCore {
    inner: Arc<NativeCoreInner>,
}

impl NativeCore {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        Self::open_with(
            path,
            Arc::new(SystemClock),
            Arc::new(CachedProjectionRefreshAdapter),
        )
    }

    fn open_with(
        path: &Path,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    ) -> Result<Self, &'static str> {
        let now = clock.now();
        let initial = unavailable_state_at(1, now);
        let (store, state) = SqliteReadModelStore::open(path, &initial)?;
        let transition = transition_snapshot_at(&state, now);
        let core = Self::with_components(
            state,
            ReadModelStore::Persistent(store),
            clock,
            refresh_adapter,
        );
        if let Some(transitioned) = transition {
            core.inner.commit_refreshed_snapshot(transitioned, now)?;
        }
        Ok(core)
    }

    pub fn unavailable() -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self::with_components(
            unavailable_state_at(1, clock.now()),
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
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
        )
    }

    #[cfg(test)]
    fn force_persistence_failure(&self) -> Result<(), &'static str> {
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state persistence unavailable")?;
        let ReadModelStore::Persistent(store) = &*store else {
            return Err("native state persistence unavailable");
        };
        store
            .connection
            .lock()
            .map_err(|_| "native state persistence unavailable")?
            .execute_batch("PRAGMA query_only = ON;")
            .map_err(|_| "native state persistence unavailable")
    }

    fn with_components(
        state: SanitizedDesktopStateV1,
        store: ReadModelStore,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
    ) -> Self {
        let now = clock.now();
        let next_scheduled_at = next_refresh_at(&state, now);
        Self {
            inner: Arc::new(NativeCoreInner {
                state: Mutex::new(state),
                store: Mutex::new(store),
                revision_subscribers: Mutex::new(Vec::new()),
                refresh_coordinator: Mutex::new(RefreshCoordinatorState {
                    consecutive_failures: 0,
                    in_flight: false,
                    next_scheduled_at,
                    retry_not_before: None,
                }),
                refresh_adapter,
                clock,
                scheduler_started: AtomicBool::new(false),
                scheduler_thread: Mutex::new(None),
            }),
        }
    }

    pub fn panel_state(&self) -> Result<SanitizedDesktopStateV1, &'static str> {
        self.inner
            .state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "native state unavailable")
    }

    pub fn revision_notices(&self) -> Result<Receiver<RevisionNotice>, &'static str> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .revision_subscribers
            .lock()
            .map_err(|_| "revision notices unavailable")?
            .push(sender);
        Ok(receiver)
    }

    pub fn request_refresh(&self, source: RefreshSource) -> Result<RefreshReceipt, &'static str> {
        if source == RefreshSource::StalePanelOpen {
            let stale = self
                .inner
                .state
                .lock()
                .map_err(|_| "native state unavailable")
                .map(|state| snapshot_needs_refresh(&state, self.inner.clock.now()))?;
            if !stale {
                return Ok(RefreshReceipt { accepted: true });
            }
        }
        if self.inner.begin_refresh(source)? {
            let refresh_inner = Arc::clone(&self.inner);
            if thread::Builder::new()
                .name("sanitized-state-refresh".to_owned())
                .spawn(move || refresh_inner.run_refresh())
                .is_err()
            {
                if let Ok(mut coordinator) = self.inner.refresh_coordinator.lock() {
                    coordinator.in_flight = false;
                }
                return Err("refresh coordinator unavailable");
            }
        }
        Ok(RefreshReceipt { accepted: true })
    }

    pub fn start_scheduler(&self) -> Result<(), &'static str> {
        if self.inner.scheduler_started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let inner = Arc::downgrade(&self.inner);
        if thread::Builder::new()
            .name("sanitized-state-schedule".to_owned())
            .spawn(move || scheduled_refresh_loop(inner))
            .is_err()
        {
            self.inner.scheduler_started.store(false, Ordering::SeqCst);
            return Err("refresh coordinator unavailable");
        }
        Ok(())
    }
}

fn scheduled_refresh_loop(inner: Weak<NativeCoreInner>) {
    let Some(current) = inner.upgrade() else {
        return;
    };
    if let Ok(mut scheduler) = current.scheduler_thread.lock() {
        *scheduler = Some(thread::current());
    }
    drop(current);

    loop {
        let Some(current) = inner.upgrade() else {
            return;
        };
        let wait = current.schedule_wait().max(Duration::from_secs(1));
        drop(current);
        thread::park_timeout(wait);
        let Some(current) = inner.upgrade() else {
            return;
        };
        let core = NativeCore { inner: current };
        let _ = core.request_refresh(RefreshSource::Schedule);
    }
}

fn read_model_backup_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.read-model-v0.backup")
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
            } if timestamp_is_due(observed_at, now) => (
                Self::Stale {
                    evidence_basis: *evidence_basis,
                    coverage: *coverage,
                    observed_at: observed_at.clone(),
                    observed_tokens: *observed_tokens,
                    api_equivalent_cost_usd: *api_equivalent_cost_usd,
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

fn snapshot_needs_refresh(snapshot: &SanitizedDesktopStateV1, now: OffsetDateTime) -> bool {
    snapshot
        .providers
        .iter()
        .any(|provider| provider.needs_refresh(now))
        || [
            &snapshot.usage.codex.today,
            &snapshot.usage.codex.seven_days,
            &snapshot.usage.codex.thirty_days,
            &snapshot.usage.claude.today,
            &snapshot.usage.claude.seven_days,
            &snapshot.usage.claude.thirty_days,
        ]
        .into_iter()
        .any(|usage| usage.needs_refresh(now))
}

fn transition_periods_at(periods: &UsagePeriods, now: OffsetDateTime) -> (UsagePeriods, bool) {
    let (today, today_changed) = periods.today.transition_at(now);
    let (seven_days, seven_days_changed) = periods.seven_days.transition_at(now);
    let (thirty_days, thirty_days_changed) = periods.thirty_days.transition_at(now);
    (
        UsagePeriods {
            today,
            seven_days,
            thirty_days,
        },
        today_changed || seven_days_changed || thirty_days_changed,
    )
}

fn transition_snapshot_at(
    snapshot: &SanitizedDesktopStateV1,
    now: OffsetDateTime,
) -> Option<SanitizedDesktopStateV1> {
    let (codex, codex_changed) = snapshot.providers[0].transition_at(now);
    let (claude, claude_changed) = snapshot.providers[1].transition_at(now);
    let (codex_usage, codex_usage_changed) = transition_periods_at(&snapshot.usage.codex, now);
    let (claude_usage, claude_usage_changed) = transition_periods_at(&snapshot.usage.claude, now);
    (codex_changed || claude_changed || codex_usage_changed || claude_usage_changed).then(|| {
        let mut transitioned = snapshot.clone();
        transitioned.providers = [codex, claude];
        transitioned.usage = UsageByProvider {
            codex: codex_usage,
            claude: claude_usage,
        };
        transitioned
    })
}

fn next_refresh_at(snapshot: &SanitizedDesktopStateV1, now: OffsetDateTime) -> OffsetDateTime {
    let provider_deadline = snapshot
        .providers
        .iter()
        .filter_map(|provider| provider.next_transition_after(now))
        .min();
    let usage_deadline = [
        &snapshot.usage.codex.today,
        &snapshot.usage.codex.seven_days,
        &snapshot.usage.codex.thirty_days,
        &snapshot.usage.claude.today,
        &snapshot.usage.claude.seven_days,
        &snapshot.usage.claude.thirty_days,
    ]
    .into_iter()
    .filter_map(|usage| usage.next_transition_after(now))
    .min();
    provider_deadline
        .into_iter()
        .chain(usage_deadline)
        .min()
        .unwrap_or(now + to_time_duration(REFRESH_INTERVAL))
        .min(now + to_time_duration(REFRESH_INTERVAL))
}

fn validate_snapshot(snapshot: &SanitizedDesktopStateV1) -> Result<(), &'static str> {
    if snapshot.contract_version != CONTRACT_VERSION
        || snapshot.revision.parse::<u64>().is_err()
        || OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339).is_err()
    {
        return Err("native state unavailable");
    }
    let provider = |snapshot: &ProviderSnapshot| match snapshot {
        ProviderSnapshot::Unavailable { provider, .. }
        | ProviderSnapshot::Current { provider, .. }
        | ProviderSnapshot::Stale { provider, .. } => *provider,
    };
    if provider(&snapshot.providers[0]) != CodingProvider::Codex
        || provider(&snapshot.providers[1]) != CodingProvider::Claude
    {
        return Err("native state unavailable");
    }
    Ok(())
}

fn unavailable_periods() -> UsagePeriods {
    UsagePeriods {
        today: UsageTotal::Unavailable,
        seven_days: UsageTotal::Unavailable,
        thirty_days: UsageTotal::Unavailable,
    }
}

fn format_time(now: OffsetDateTime) -> String {
    now.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

pub fn unavailable_state(revision: u64) -> SanitizedDesktopStateV1 {
    unavailable_state_at(revision, OffsetDateTime::now_utc())
}

fn unavailable_state_at(revision: u64, now: OffsetDateTime) -> SanitizedDesktopStateV1 {
    SanitizedDesktopStateV1 {
        contract_version: CONTRACT_VERSION,
        generated_at: format_time(now),
        revision: revision.max(1).to_string(),
        providers: [
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Codex,
                quota_lanes: [],
            },
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: [],
            },
        ],
        usage: UsageByProvider {
            codex: unavailable_periods(),
            claude: unavailable_periods(),
        },
        sync: SyncState {
            status: SyncStatus::Unavailable,
            last_successful_at: None,
        },
    }
}

pub fn native_contract_schema() -> Schema {
    schema_for!(SanitizedDesktopStateV1)
}

pub fn native_contract_export() -> Value {
    json!({
        "bootstrapContractVersion": LIFECYCLE_CONTRACT_VERSION,
        "bootstrapStateSchema": bootstrap_state_schema(),
        "contractVersion": CONTRACT_VERSION,
        "refreshReceiptSchema": schema_for!(RefreshReceipt),
        "revisionNoticeEvent": REVISION_NOTICE_EVENT,
        "revisionNoticeSchema": schema_for!(RevisionNotice),
        "settingsContractVersion": LIFECYCLE_CONTRACT_VERSION,
        "settingsNavigationEvent": SETTINGS_NAVIGATION_EVENT,
        "settingsNavigationSchema": settings_navigation_schema(),
        "settingsStateSchema": settings_state_schema(),
        "stateSchema": native_contract_schema(),
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
            Barrier,
            atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering},
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
                read_model_backup_path(&self.0),
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
        responses: Mutex<VecDeque<Result<Option<SanitizedDesktopStateV1>, &'static str>>>,
        runs: AtomicUsize,
    }

    impl ScriptedRefreshSource {
        fn new(
            responses: impl IntoIterator<Item = Result<Option<SanitizedDesktopStateV1>, &'static str>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                runs: AtomicUsize::new(0),
            }
        }
    }

    impl SnapshotRefreshAdapter for ScriptedRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV1,
        ) -> Result<Option<SanitizedDesktopStateV1>, &'static str> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(None))
        }
    }

    fn observed_state(
        observed_at: OffsetDateTime,
        observed_tokens: u64,
    ) -> SanitizedDesktopStateV1 {
        let observed_at = format_time(observed_at);
        SanitizedDesktopStateV1 {
            contract_version: CONTRACT_VERSION,
            generated_at: observed_at.clone(),
            revision: "1".to_owned(),
            providers: [
                ProviderSnapshot::Current {
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
                ProviderSnapshot::Unavailable {
                    provider: CodingProvider::Claude,
                    quota_lanes: [],
                },
            ],
            usage: UsageByProvider {
                codex: UsagePeriods {
                    today: UsageTotal::Current {
                        evidence_basis: UsageEvidenceBasis::ProviderReported,
                        coverage: UsageCoverage::Complete,
                        observed_at: observed_at.clone(),
                        observed_tokens,
                        api_equivalent_cost_usd: None,
                    },
                    seven_days: UsageTotal::Unavailable,
                    thirty_days: UsageTotal::Unavailable,
                },
                claude: unavailable_periods(),
            },
            sync: SyncState {
                status: SyncStatus::Unavailable,
                last_successful_at: None,
            },
        }
    }

    fn test_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_775_347_200).unwrap()
    }

    #[test]
    fn unavailable_snapshot_never_invents_zero_usage() {
        let value = serde_json::to_value(unavailable_state(1)).unwrap();
        assert_eq!(value["contractVersion"], CONTRACT_VERSION);
        assert_eq!(value["revision"], "1");
        assert_eq!(
            value["usage"]["codex"]["today"],
            json!({ "availability": "unavailable" })
        );
        assert!(value.to_string().find("observedTokens").is_none());
    }

    #[test]
    fn refresh_commit_is_monotonic_and_notified_after_commit() {
        let database = TestDatabase::new();
        let core = NativeCore::open_with(
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
            after.usage.codex.today,
            UsageTotal::Current { .. }
        ));
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
            _cached: SanitizedDesktopStateV1,
        ) -> Result<Option<SanitizedDesktopStateV1>, &'static str> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.wait();
            self.release.wait();
            Ok(Some(observed_state(test_time(), 42)))
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
        let core = NativeCore::open_with(&database.0, clock.clone(), source).unwrap();
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

        let relaunched =
            NativeCore::open_with(&database.0, clock, Arc::new(ScriptedRefreshSource::new([])))
                .unwrap();
        fs::remove_file(&database.0).unwrap();
        let cached = relaunched.panel_state().unwrap();

        assert_eq!(cached.revision, "2");
        assert!(matches!(
            cached.usage.codex.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
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
            NativeCore::open_with(
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
        assert!(read_model_backup_path(&database.0).is_file());
    }

    #[test]
    fn every_refresh_trigger_uses_the_fake_clock_coordinator() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let source = Arc::new(ScriptedRefreshSource::new(
            (1..=6).map(|tokens| Ok(Some(observed_state(test_time(), tokens)))),
        ));
        let core = NativeCore::open_with(&database.0, clock.clone(), source.clone()).unwrap();
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
            "4"
        );
        assert!(core.request_refresh(RefreshSource::Wake).unwrap().accepted);
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "5"
        );
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
            "6"
        );

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
            "7"
        );
        assert_eq!(source.runs.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn offline_and_persistence_failures_preserve_stale_values_and_back_off() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(test_time(), 42))),
            Err("offline"),
            Ok(Some(observed_state(
                test_time() + time::Duration::minutes(5),
                43,
            ))),
        ]));
        let core = NativeCore::open_with(&database.0, clock.clone(), source.clone()).unwrap();
        let notices = core.revision_notices().unwrap();

        assert!(
            core.request_refresh(RefreshSource::Launch)
                .unwrap()
                .accepted
        );
        notices.recv_timeout(Duration::from_secs(1)).unwrap();
        core.force_persistence_failure().unwrap();
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
        assert!(matches!(stale.providers[0], ProviderSnapshot::Stale { .. }));
        assert!(matches!(
            stale.usage.codex.today,
            UsageTotal::Stale {
                observed_tokens: 42,
                ..
            }
        ));
        assert_eq!(stale.sync.status, SyncStatus::Unavailable);

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
            core.panel_state().unwrap().usage.codex.today,
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
    fn reset_deadline_expires_only_the_affected_quota_lane() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let mut observed = observed_state(test_time(), 42);
        if let ProviderSnapshot::Current { quota_lanes, .. } = &mut observed.providers[0] {
            quota_lanes[0].reset_at = Some(format_time(test_time() + TimeDuration::minutes(2)));
        }
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(observed)), Ok(None)]));
        let core = NativeCore::open_with(&database.0, clock.clone(), source).unwrap();
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
            after_reset.providers[0],
            ProviderSnapshot::Unavailable { .. }
        ));
        assert!(matches!(
            after_reset.usage.codex.today,
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
