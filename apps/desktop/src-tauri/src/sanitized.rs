use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::daily_usage_aggregate::combine_usage_periods;
use crate::doomerboard::{
    ADD_TOKENMAXXER_CONTRACT_VERSION, DOOMERBOARD_CONTRACT_VERSION, add_tokenmaxxer_outcome_schema,
    doomerboard_view_schema,
};
use crate::lifecycle::{
    LIFECYCLE_CONTRACT_VERSION, SETTINGS_CONTRACT_VERSION, SETTINGS_NAVIGATION_EVENT,
    SETTINGS_RECOVERY_CLEAR_EVENT, bootstrap_state_schema, settings_navigation_schema,
    settings_state_schema,
};
use crate::profile::ActiveMacActivation;
pub use crate::providers::{CodingProvider, ProviderPresenceStatus};
use crate::providers::{
    PROVIDER_REGISTRY, ProviderCorrection, ProviderEnablementPolicy, all_providers_enabled_policy,
    detect_provider_presence, production_observation_coordinator, provider_descriptor,
};
use crate::quota_headroom::{RevisionedOverallQuotaHeadroom, overall_quota_headroom};
use crate::updater::{UPDATE_CONTRACT_VERSION, UPDATE_STATE_CHANGED_EVENT, update_state_schema};
#[cfg(test)]
use crate::usage_sync::{
    DailyUsageAggregate, ProviderSettingsAcknowledgement, SyncCoverage, SyncEvidenceBasis,
    UsageSyncAcknowledgement, generation_one_profile_backfill_is_pending, queue_daily_aggregate,
};
use crate::usage_sync::{
    PendingUsageBatch, QueueState, QueueUpdate, UsageQueueRequest, UsageSyncAcknowledgements,
    UsageSyncAttemptResult, UsageSyncCorrections, activate_generation,
    apply_provider_settings_acknowledgement, apply_usage_acknowledgements,
    capture_generation_baselines, has_current_terminal_usage_conflict, install_usage_sync_schema,
    load_active_usage_sync_generation, load_next_pending_usage_batch,
    load_usage_sync_generation_state, mark_generation_authority_rejected,
    migrate_usage_sync_schema_from_v6, queue_provider_settings, queue_usage_for_commit,
    replace_profile_generation, stage_usage_sync_corrections,
};

pub const CONTRACT_VERSION: u8 = 4;
pub const PANEL_ADD_TOKENMAXXER_EVENT: &str = "panel-add-tokenmaxxer-requested";
pub const REVISION_NOTICE_EVENT: &str = "sanitized-desktop-state-revision";
pub(crate) const READ_MODEL_SCHEMA_VERSION: i64 = 7;
pub(crate) const READ_MODEL_SCHEMA_MODULE: &str = "sanitized-desktop-state";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REFRESH_BACKOFF_BASE: Duration = Duration::from_secs(5);
const REFRESH_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const REFRESH_ATTEMPT_TIMEOUT: Duration = REFRESH_INTERVAL;
const NETWORK_RECOVERY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const LOCAL_USAGE_CATCH_UP_SUCCESS_DELAY: Duration = Duration::from_millis(250);
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
    #[serde(default)]
    pub top_model_usage: Option<TopModelUsage>,
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
    #[serde(default)]
    pub top_model_usage: Option<TopModelUsage>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopModelUsage {
    #[serde(default)]
    #[schemars(length(min = 1, max = 48))]
    pub model: Option<String>,
    #[schemars(range(min = 1))]
    pub observed_tokens: u64,
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
                    top_model_usage: None,
                }
            })
            .collect();
        let mut current = SanitizedDesktopStateV3 {
            contract_version: CONTRACT_VERSION,
            generated_at,
            revision,
            providers,
            top_model_usage: None,
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
        let visible = self
            .providers
            .iter()
            .filter(|presentation| presentation.is_visible())
            .collect::<Vec<_>>();
        let periods = visible
            .iter()
            .map(|presentation| &presentation.usage)
            .collect::<Vec<_>>();
        self.combined_usage = combine_usage_periods(&periods);
        self.top_model_usage = combined_top_model_usage(&visible);
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
                    presentation.top_model_usage = None;
                }
                presentation
            })
            .collect();
        let periods = self
            .providers
            .iter()
            .filter(|presentation| {
                enablement.is_provider_enabled(presentation.provider) && presentation.is_visible()
            })
            .map(|presentation| &presentation.usage)
            .collect::<Vec<_>>();
        self.combined_usage = combine_usage_periods(&periods);
        self.top_model_usage = combined_top_model_usage(
            &self
                .providers
                .iter()
                .filter(|presentation| {
                    enablement.is_provider_enabled(presentation.provider)
                        && presentation.is_visible()
                })
                .collect::<Vec<_>>(),
        );
        disabled_providers
    }
}

fn combined_top_model_usage(providers: &[&ProviderPresentation]) -> Option<TopModelUsage> {
    providers
        .iter()
        .filter_map(|provider| provider.top_model_usage.as_ref())
        .cloned()
        .reduce(preferred_top_model)
}

pub(crate) fn preferred_top_model(
    current: TopModelUsage,
    candidate: TopModelUsage,
) -> TopModelUsage {
    if candidate.observed_tokens > current.observed_tokens
        || (candidate.observed_tokens == current.observed_tokens
            && candidate.model.is_none()
            && current.model.is_some())
        || (candidate.observed_tokens == current.observed_tokens
            && candidate.model.is_some()
            && current.model.is_some()
            && candidate.model < current.model)
    {
        candidate
    } else {
        current
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
            top_model_usage: None,
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.presence == ProviderPresenceStatus::Detected
            || !matches!(self.quota, ProviderSnapshot::Unavailable { .. })
            || self.usage.scan_status == UsageScanStatus::Indexing
            || !matches!(self.usage.today, UsageTotal::Unavailable)
            || !matches!(self.usage.seven_days, UsageTotal::Unavailable)
            || !matches!(self.usage.thirty_days, UsageTotal::Unavailable)
    }

    fn has_cached_quota_or_observed_usage(&self) -> bool {
        !matches!(self.quota, ProviderSnapshot::Unavailable { .. })
            || [
                &self.usage.today,
                &self.usage.seven_days,
                &self.usage.thirty_days,
            ]
            .into_iter()
            .any(|usage| !matches!(usage, UsageTotal::Unavailable))
    }

    fn is_waiting_for_first_observation(&self) -> bool {
        !self.has_cached_quota_or_observed_usage()
            && self.usage.scan_status == UsageScanStatus::Indexing
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageSyncAuthorityIdentity {
    active_mac_generation: Option<u64>,
    touch_grass_id: Option<String>,
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
#[serde(rename_all = "kebab-case")]
pub enum SyncStatus {
    Synced,
    Pending,
    Stale,
    Offline,
    AuthorityRejected,
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
    provider_cancellation: Option<(Arc<AtomicU64>, u64)>,
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
            provider_cancellation: None,
            deadline: Instant::now() + REFRESH_ATTEMPT_TIMEOUT,
            sources,
        }
    }

    pub(crate) fn with_provider_cancellation(
        &self,
        generation: Arc<AtomicU64>,
        expected_generation: u64,
    ) -> Self {
        let mut attempt = self.clone();
        attempt.provider_cancellation = Some((generation, expected_generation));
        attempt
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
            || self
                .provider_cancellation
                .as_ref()
                .is_some_and(|(generation, expected)| {
                    generation.load(Ordering::Acquire) != *expected
                })
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

    fn cancel_provider(&self, _provider: CodingProvider) {}

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
    ) -> Result<SnapshotRefreshOutcome, RefreshFailure>;

    /// Reports complete refresh outcomes as soon as they are safe to commit.
    /// Adapters that have one result use the default atomic report. A
    /// multi-provider coordinator can report one provider at a time.
    fn refresh_with_progress(
        &self,
        cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
        progress: &dyn SnapshotRefreshProgress,
    ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
        let outcome = self.refresh(cached, attempt)?;
        progress.report(outcome)?;
        Ok(SnapshotRefreshOutcome::default())
    }
}

pub(crate) trait SnapshotRefreshProgress: Send + Sync {
    fn report(&self, outcome: SnapshotRefreshOutcome) -> Result<(), RefreshFailure>;
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct SnapshotRefreshOutcome {
    /// A complete replacement candidate. `None` means provider data did not change.
    pub(crate) snapshot: Option<SanitizedDesktopStateV3>,
    /// Providers whose own refresh work reached a non-cancelled terminal result.
    /// The native commit uses this set to end only matching first-observation waits.
    pub(crate) completed_providers: BTreeSet<CodingProvider>,
    /// Content-free correction proof from provider-private aggregation.
    pub(crate) corrections: BTreeMap<CodingProvider, ProviderCorrection>,
}

impl From<Option<SanitizedDesktopStateV3>> for SnapshotRefreshOutcome {
    fn from(snapshot: Option<SanitizedDesktopStateV3>) -> Self {
        Self {
            snapshot,
            completed_providers: BTreeSet::new(),
            corrections: BTreeMap::new(),
        }
    }
}

impl SnapshotRefreshOutcome {
    fn is_empty(&self) -> bool {
        self.snapshot.is_none()
            && self.completed_providers.is_empty()
            && self.corrections.is_empty()
    }
}

struct UnavailableRefreshAdapter;

impl SnapshotRefreshAdapter for UnavailableRefreshAdapter {
    fn refresh(
        &self,
        _cached: SanitizedDesktopStateV3,
        _attempt: &RefreshAttempt,
    ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
        Err(RefreshFailure::SourceUnavailable)
    }
}

#[cfg(test)]
struct CachedProjectionRefreshAdapter;

#[cfg(test)]
impl SnapshotRefreshAdapter for CachedProjectionRefreshAdapter {
    fn refresh(
        &self,
        _cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
    ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
        attempt.remaining()?;
        // Provider observation is not wired yet. An unchanged cached projection
        // does not create a false revision or notice.
        Ok(SnapshotRefreshOutcome::default())
    }
}

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub(crate) type RefreshTrigger = Arc<dyn Fn() + Send + Sync>;
pub(crate) type UsageSyncRequest = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct UsageSyncRequests {
    request: Mutex<Option<UsageSyncRequest>>,
}

impl UsageSyncRequests {
    fn install(&self, request: UsageSyncRequest) -> Result<(), &'static str> {
        *self
            .request
            .lock()
            .map_err(|_| "usage synchronization unavailable")? = Some(request);
        Ok(())
    }

    fn request(&self) {
        let request = self.request.lock().ok().and_then(|request| request.clone());
        if let Some(request) = request {
            request();
        }
    }

    fn clear(&self) {
        if let Ok(mut request) = self.request.lock() {
            *request = None;
        }
    }
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

fn active_mac_activation_time(milliseconds: u64) -> Result<OffsetDateTime, &'static str> {
    if milliseconds > MAX_SAFE_INTEGER {
        return Err("native state persistence unavailable");
    }
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
        .map_err(|_| "native state persistence unavailable")
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
    active_mac_generation: Option<u64>,
    connection: Connection,
}

enum UsageSyncCommit<'a> {
    QueueCurrent(UsageSyncCorrections),
    Activate {
        activated_at: OffsetDateTime,
        generation: u64,
    },
    ReplaceAuthority {
        activated_at: OffsetDateTime,
        generation: u64,
    },
    Acknowledge {
        acknowledgements: &'a UsageSyncAcknowledgements,
        batch: &'a PendingUsageBatch,
    },
    Pending,
    Offline,
    AuthorityRejected(Option<u64>),
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
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| "native state persistence unavailable")?;
        Self::migrate(&mut connection, path, initial)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
            .map_err(|_| "native state persistence unavailable")?;
        let state = Self::read_from(&connection)?;
        let active_mac_generation = load_active_usage_sync_generation(&connection)
            .map_err(|_| "native state persistence unavailable")?;
        Ok((
            Self {
                active_mac_generation,
                connection,
            },
            state,
        ))
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
            let stored_versions = connection
                .query_row(
                    "SELECT schema_version, contract_version
                     FROM sanitized_desktop_state
                     WHERE singleton = 1",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(|_| "native state persistence unavailable")?;
            if stored_versions != (READ_MODEL_SCHEMA_VERSION, i64::from(CONTRACT_VERSION)) {
                return Err("native state persistence unavailable");
            }
            install_usage_sync_schema(connection)
                .map_err(|_| "native state persistence unavailable")?;
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
            let mut snapshot = snapshot.into_current(revision.clone(), version)?;
            normalize_legacy_sync_state(&mut snapshot);
            validate_snapshot(&snapshot)?;
            (snapshot, revision)
        } else if (4..=5).contains(&version) {
            let (schema_version, contract_version, revision, snapshot_json) = connection
                .query_row(
                    "SELECT schema_version, contract_version, revision, snapshot_json
                     FROM sanitized_desktop_state WHERE singleton = 1",
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
            if schema_version != version || contract_version != 3 {
                return Err("native state persistence unavailable");
            }
            let mut snapshot_value: Value = serde_json::from_str(&snapshot_json)
                .map_err(|_| "native state persistence unavailable")?;
            let snapshot_object = snapshot_value
                .as_object_mut()
                .ok_or("native state persistence unavailable")?;
            if snapshot_object
                .get("contractVersion")
                .and_then(Value::as_u64)
                != Some(3)
            {
                return Err("native state persistence unavailable");
            }
            snapshot_object.insert("contractVersion".to_owned(), json!(CONTRACT_VERSION));
            let mut snapshot: SanitizedDesktopStateV3 = serde_json::from_value(snapshot_value)
                .map_err(|_| "native state persistence unavailable")?;
            if snapshot.revision != revision {
                return Err("native state persistence unavailable");
            }
            normalize_legacy_sync_state(&mut snapshot);
            validate_snapshot(&snapshot)?;
            (snapshot, revision)
        } else if version == 6 {
            let (schema_version, contract_version, revision, snapshot_json) = connection
                .query_row(
                    "SELECT schema_version, contract_version, revision, snapshot_json
                     FROM sanitized_desktop_state WHERE singleton = 1",
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
            if schema_version != version || contract_version != CONTRACT_VERSION {
                return Err("native state persistence unavailable");
            }
            let snapshot: SanitizedDesktopStateV3 = serde_json::from_str(&snapshot_json)
                .map_err(|_| "native state persistence unavailable")?;
            validate_snapshot(&snapshot)?;
            if snapshot.revision != revision {
                return Err("native state persistence unavailable");
            }
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
                   schema_version INTEGER NOT NULL CHECK (schema_version = 7),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 4),
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
        if version == 6 {
            migrate_usage_sync_schema_from_v6(&transaction)
                .map_err(|_| "native state persistence unavailable")?;
        } else {
            install_usage_sync_schema(&transaction)
                .map_err(|_| "native state persistence unavailable")?;
        }
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

    fn commit(
        &mut self,
        state: &mut SanitizedDesktopStateV3,
        now: OffsetDateTime,
        usage_sync: UsageSyncCommit<'_>,
        previous_enabled_providers: &BTreeSet<CodingProvider>,
        enabled_providers: &BTreeSet<CodingProvider>,
    ) -> Result<bool, &'static str> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| "native state persistence unavailable")?;
        let pending_before = self
            .active_mac_generation
            .map(|generation| {
                load_next_pending_usage_batch(
                    &transaction,
                    generation,
                    now,
                    previous_enabled_providers,
                )
            })
            .transpose()
            .map_err(|_| "native state persistence unavailable")?
            .flatten();
        let mut pending_usage_changed = false;
        match usage_sync {
            UsageSyncCommit::QueueCurrent(corrections) => {
                if let Some(generation) = self.active_mac_generation {
                    if previous_enabled_providers != enabled_providers {
                        pending_usage_changed |=
                            queue_provider_settings(&transaction, generation, enabled_providers)
                                .map_err(|_| "native state persistence unavailable")?;
                    }
                    let updates = queue_usage_for_commit(
                        &transaction,
                        generation,
                        state,
                        now,
                        enabled_providers,
                        UsageQueueRequest::Refresh(&corrections),
                    )
                    .map_err(|_| "native state persistence unavailable")?;
                    pending_usage_changed |= updates.iter().any(|update| {
                        matches!(
                            update,
                            QueueUpdate::Stored {
                                state: QueueState::Pending,
                                ..
                            }
                        )
                    });
                    apply_queue_status(
                        &transaction,
                        generation,
                        state,
                        now,
                        &updates,
                        enabled_providers,
                    )?;
                } else {
                    stage_usage_sync_corrections(&transaction, now, &corrections)
                        .map_err(|_| "native state persistence unavailable")?;
                }
            }
            UsageSyncCommit::Activate {
                activated_at,
                generation,
            } => {
                activate_generation(&transaction, generation)
                    .map_err(|_| "native state persistence unavailable")?;
                capture_generation_baselines(&transaction, generation, state, activated_at, now)
                    .map_err(|_| "native state persistence unavailable")?;
                self.active_mac_generation = Some(generation);
                pending_usage_changed |=
                    queue_provider_settings(&transaction, generation, enabled_providers)
                        .map_err(|_| "native state persistence unavailable")?;
                state.sync.status = SyncStatus::Pending;
                let anchor_day = activated_at.to_offset(time::UtcOffset::UTC).date();
                let updates = queue_usage_for_commit(
                    &transaction,
                    generation,
                    state,
                    now,
                    enabled_providers,
                    UsageQueueRequest::ProfileActivation { anchor_day },
                )
                .map_err(|_| "native state persistence unavailable")?;
                apply_queue_status(
                    &transaction,
                    generation,
                    state,
                    now,
                    &updates,
                    enabled_providers,
                )?;
            }
            UsageSyncCommit::ReplaceAuthority {
                activated_at,
                generation,
            } => {
                replace_profile_generation(&transaction, generation)
                    .map_err(|_| "native state persistence unavailable")?;
                capture_generation_baselines(&transaction, generation, state, activated_at, now)
                    .map_err(|_| "native state persistence unavailable")?;
                self.active_mac_generation = Some(generation);
                pending_usage_changed |=
                    queue_provider_settings(&transaction, generation, enabled_providers)
                        .map_err(|_| "native state persistence unavailable")?;
                state.sync.status = SyncStatus::Pending;
                let anchor_day = activated_at.to_offset(time::UtcOffset::UTC).date();
                let updates = queue_usage_for_commit(
                    &transaction,
                    generation,
                    state,
                    now,
                    enabled_providers,
                    UsageQueueRequest::ProfileActivation { anchor_day },
                )
                .map_err(|_| "native state persistence unavailable")?;
                apply_queue_status(
                    &transaction,
                    generation,
                    state,
                    now,
                    &updates,
                    enabled_providers,
                )?;
            }
            UsageSyncCommit::Acknowledge {
                acknowledgements,
                batch,
            } => {
                if self.active_mac_generation != Some(batch.active_mac_generation()) {
                    return Err("native state persistence unavailable");
                }
                pending_usage_changed |= apply_provider_settings_acknowledgement(
                    &transaction,
                    batch,
                    acknowledgements.provider_settings.as_ref(),
                )
                .map_err(|_| "native state persistence unavailable")?;
                if !acknowledgements.usage.is_empty() && !acknowledgements.usage_mutation_completed
                {
                    return Err("native state persistence unavailable");
                }
                if !acknowledgements.usage.is_empty()
                    || (batch.is_empty_profile_backfill()
                        && acknowledgements.usage_mutation_completed)
                {
                    apply_usage_acknowledgements(&transaction, batch, &acknowledgements.usage)
                        .map_err(|_| "native state persistence unavailable")?;
                } else if batch.has_usage_snapshots()
                    && !acknowledgements
                        .provider_settings
                        .as_ref()
                        .is_some_and(|acknowledgement| {
                            acknowledgement.outcome
                                == crate::usage_sync::AcknowledgementOutcome::Stale
                        })
                {
                    return Err("native state persistence unavailable");
                }
                let has_stale = acknowledgements.usage.iter().any(|acknowledgement| {
                    acknowledgement.outcome == crate::usage_sync::AcknowledgementOutcome::Stale
                });
                let has_terminal_conflict = has_current_terminal_usage_conflict(
                    &transaction,
                    batch.active_mac_generation(),
                    now,
                    enabled_providers,
                )
                .map_err(|_| "native state persistence unavailable")?;
                pending_usage_changed |= has_stale;
                update_sync_status_after_acknowledgement(
                    state,
                    batch.has_successful_current_day_acknowledgement(&acknowledgements.usage, now),
                    has_terminal_conflict,
                    now,
                );
                let generation = batch.active_mac_generation();
                let updates = queue_usage_for_commit(
                    &transaction,
                    generation,
                    state,
                    now,
                    enabled_providers,
                    UsageQueueRequest::AfterAcknowledgement,
                )
                .map_err(|_| "native state persistence unavailable")?;
                pending_usage_changed |= updates.iter().any(|update| {
                    matches!(
                        update,
                        QueueUpdate::Stored {
                            state: QueueState::Pending,
                            ..
                        }
                    )
                });
                apply_queue_status(
                    &transaction,
                    generation,
                    state,
                    now,
                    &updates,
                    enabled_providers,
                )?;
            }
            UsageSyncCommit::Offline => {
                state.sync.status = SyncStatus::Offline;
            }
            UsageSyncCommit::Pending => {
                state.sync.status = SyncStatus::Pending;
                if let Some(generation) = self.active_mac_generation {
                    apply_queue_status(
                        &transaction,
                        generation,
                        state,
                        now,
                        &[],
                        enabled_providers,
                    )?;
                }
            }
            UsageSyncCommit::AuthorityRejected(generation) => {
                if let Some(generation) = generation {
                    if self.active_mac_generation != Some(generation) {
                        return Err("native state persistence unavailable");
                    }
                    mark_generation_authority_rejected(&transaction, generation)
                        .map_err(|_| "native state persistence unavailable")?;
                }
                state.sync.status = SyncStatus::AuthorityRejected;
            }
        }
        let pending_after = self
            .active_mac_generation
            .map(|generation| {
                load_next_pending_usage_batch(&transaction, generation, now, enabled_providers)
            })
            .transpose()
            .map_err(|_| "native state persistence unavailable")?
            .flatten();
        pending_usage_changed |= pending_after.is_some() && pending_after != pending_before;
        persist_snapshot(&transaction, state)?;
        transaction
            .commit()
            .map_err(|_| "native state persistence unavailable")?;
        Ok(pending_usage_changed)
    }

    fn pending_usage_batch(
        &self,
        active_mac_generation: u64,
        now: OffsetDateTime,
        enabled_providers: &BTreeSet<CodingProvider>,
    ) -> Result<Option<PendingUsageBatch>, &'static str> {
        if self.active_mac_generation != Some(active_mac_generation) {
            return Ok(None);
        }
        load_next_pending_usage_batch(
            &self.connection,
            active_mac_generation,
            now,
            enabled_providers,
        )
        .map_err(|_| "native state persistence unavailable")
    }

    fn confirm_active_mac_activation(
        &mut self,
        active_mac_generation: u64,
        active_mac_activated_at: OffsetDateTime,
        now: OffsetDateTime,
        state: &SanitizedDesktopStateV3,
    ) -> Result<(), &'static str> {
        if self.active_mac_generation != Some(active_mac_generation) {
            return Err("native state persistence unavailable");
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| "native state persistence unavailable")?;
        capture_generation_baselines(
            &transaction,
            active_mac_generation,
            state,
            active_mac_activated_at,
            now,
        )
        .map_err(|_| "native state persistence unavailable")?;
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

fn normalize_legacy_sync_state(snapshot: &mut SanitizedDesktopStateV3) {
    if snapshot.sync.last_successful_at.is_none()
        && matches!(snapshot.sync.status, SyncStatus::Synced | SyncStatus::Stale)
    {
        snapshot.sync.status = SyncStatus::Unavailable;
    }
}

fn sync_status_from_last_successful_day(
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
) -> SyncStatus {
    let Some(last_successful_at) = state.sync.last_successful_at.as_deref() else {
        return SyncStatus::Unavailable;
    };
    if OffsetDateTime::parse(last_successful_at, &Rfc3339).is_ok_and(|last_success| {
        last_success.to_offset(time::UtcOffset::UTC).date()
            == now.to_offset(time::UtcOffset::UTC).date()
    }) {
        SyncStatus::Synced
    } else {
        SyncStatus::Stale
    }
}

fn update_sync_status_after_acknowledgement(
    state: &mut SanitizedDesktopStateV3,
    acknowledged_current_day: bool,
    has_current_terminal_conflict: bool,
    now: OffsetDateTime,
) {
    if acknowledged_current_day {
        state.sync.last_successful_at = Some(format_time(now));
    }
    state.sync.status = if has_current_terminal_conflict {
        if state.sync.last_successful_at.is_some() {
            SyncStatus::Stale
        } else {
            SyncStatus::Unavailable
        }
    } else {
        sync_status_from_last_successful_day(state, now)
    };
}

fn apply_queue_status(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &mut SanitizedDesktopStateV3,
    now: OffsetDateTime,
    updates: &[QueueUpdate],
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<(), &'static str> {
    if updates.iter().any(|update| {
        matches!(
            update,
            QueueUpdate::Stored {
                state: QueueState::Blocked,
                ..
            }
        )
    }) {
        state.sync.status = SyncStatus::AuthorityRejected;
        return Ok(());
    }

    let pending =
        load_next_pending_usage_batch(transaction, active_mac_generation, now, enabled_providers)
            .map_err(|_| "native state persistence unavailable")?
            .is_some();
    if pending {
        if !matches!(
            state.sync.status,
            SyncStatus::Offline | SyncStatus::AuthorityRejected
        ) {
            state.sync.status = SyncStatus::Pending;
        }
    } else if updates
        .iter()
        .any(|update| matches!(update, QueueUpdate::Stale { .. }))
        || state.sync.status == SyncStatus::Pending
        || (state.sync.status == SyncStatus::Synced
            && !state
                .sync
                .last_successful_at
                .as_deref()
                .is_some_and(|value| {
                    OffsetDateTime::parse(value, &Rfc3339).is_ok_and(|last_success| {
                        last_success.date() == now.to_offset(time::UtcOffset::UTC).date()
                    })
                }))
    {
        state.sync.status = if state.sync.last_successful_at.is_some() {
            SyncStatus::Stale
        } else {
            SyncStatus::Unavailable
        };
    }
    Ok(())
}

fn persist_snapshot(
    transaction: &Transaction<'_>,
    state: &SanitizedDesktopStateV3,
) -> Result<(), &'static str> {
    validate_snapshot(state)?;
    let snapshot_json =
        serde_json::to_string(state).map_err(|_| "native state persistence unavailable")?;
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
    (updated == 1)
        .then_some(())
        .ok_or("native state persistence unavailable")
}

enum ReadModelStore {
    Persistent(SqliteReadModelStore),
    Memory,
}

struct SnapshotCommitOutcome {
    notice: Option<RevisionNotice>,
    pending_usage_changed: bool,
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
    state: Mutex<CachedProjectionState>,
}

struct CachedProjectionState {
    snapshot: SanitizedDesktopStateV3,
    first_observation_waits: BTreeSet<CodingProvider>,
    enabled_providers: BTreeSet<CodingProvider>,
}

#[derive(Clone, Copy)]
struct ProviderEnablementChange {
    provider: CodingProvider,
    enabled: bool,
}

#[derive(Default)]
struct SnapshotCommitOptions {
    force_revision: bool,
    provider_enablement_change: Option<ProviderEnablementChange>,
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
    fn new(state: SanitizedDesktopStateV3, enabled_providers: BTreeSet<CodingProvider>) -> Self {
        Self {
            state: Mutex::new(CachedProjectionState {
                snapshot: state,
                first_observation_waits: BTreeSet::new(),
                enabled_providers,
            }),
        }
    }

    fn snapshot(&self) -> Result<SanitizedDesktopStateV3, &'static str> {
        self.state
            .lock()
            .map(|state| state.snapshot.clone())
            .map_err(|_| "native state unavailable")
    }

    fn panel_snapshot(&self) -> Result<SanitizedDesktopStateV3, &'static str> {
        let state = self.state.lock().map_err(|_| "native state unavailable")?;
        let mut snapshot = state.snapshot.clone();
        for provider in &state.first_observation_waits {
            if let Some(presentation) = snapshot.provider_mut(*provider) {
                presentation.usage.scan_status = UsageScanStatus::Indexing;
            }
        }
        Ok(snapshot)
    }

    fn menu_bar_snapshot(
        &self,
    ) -> Result<(SanitizedDesktopStateV3, BTreeSet<CodingProvider>), &'static str> {
        let state = self.state.lock().map_err(|_| "native state unavailable")?;
        Ok((state.snapshot.clone(), state.enabled_providers.clone()))
    }

    fn enabled_providers(&self) -> Result<BTreeSet<CodingProvider>, &'static str> {
        self.state
            .lock()
            .map(|state| state.enabled_providers.clone())
            .map_err(|_| "native state unavailable")
    }

    fn snapshot_with_first_observation_waits(
        &self,
    ) -> Result<(SanitizedDesktopStateV3, BTreeSet<CodingProvider>), &'static str> {
        let state = self.state.lock().map_err(|_| "native state unavailable")?;
        Ok((
            state.snapshot.clone(),
            state.first_observation_waits.clone(),
        ))
    }

    fn commit_transitioned_snapshot(
        &self,
        store: &mut ReadModelStore,
        transitioned: SanitizedDesktopStateV3,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        self.commit_snapshot_with_force(
            store,
            transitioned,
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions::default(),
        )
    }

    fn commit_refreshed_snapshot_with_completed(
        &self,
        store: &mut ReadModelStore,
        mut refreshed: SanitizedDesktopStateV3,
        enablement: &dyn ProviderEnablementPolicy,
        now: OffsetDateTime,
        completed_first_observations: &BTreeSet<CodingProvider>,
        corrections: &BTreeMap<CodingProvider, ProviderCorrection>,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, mut first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        let previous_first_observation_waits = first_observation_waits.clone();
        for provider in completed_first_observations {
            first_observation_waits.remove(provider);
        }
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
                                .filter(|presentation| {
                                    presentation.is_waiting_for_first_observation()
                                })
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
        let first_observation_wait_changed =
            first_observation_waits != previous_first_observation_waits;
        let mut usage_sync_corrections = UsageSyncCorrections::default();
        for (provider, correction) in corrections {
            match correction {
                ProviderCorrection::ParserCorrection { source_revision } => {
                    usage_sync_corrections
                        .record_parser_correction(*provider, *source_revision)
                        .map_err(|_| "native state persistence unavailable")?;
                }
            }
        }
        self.commit_snapshot_with_force_and_corrections(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions {
                force_revision: first_observation_wait_changed || !corrections.is_empty(),
                ..SnapshotCommitOptions::default()
            },
            usage_sync_corrections,
        )
    }

    fn commit_profile_outcome(
        &self,
        store: &mut ReadModelStore,
        profile: SanitizedProfileOutcome,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        if cached.profile == profile {
            return Ok(SnapshotCommitOutcome {
                notice: None,
                pending_usage_changed: false,
                persistence_failed: false,
            });
        }
        let mut refreshed = cached.clone();
        refreshed.profile = profile;
        self.commit_snapshot_with_force(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions::default(),
        )
    }

    fn commit_profile_recovery(
        &self,
        store: &mut ReadModelStore,
        profile: SanitizedProfileOutcome,
        activation: ActiveMacActivation,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        let replaces_profile = matches!(
            (&cached.profile, &profile),
            (
                SanitizedProfileOutcome::Ready {
                    touch_grass_id: previous,
                    ..
                },
                SanitizedProfileOutcome::Ready {
                    touch_grass_id: recovered,
                    ..
                }
            ) if previous != recovered
        );
        let mut refreshed = cached.clone();
        refreshed.profile = profile;
        let activated_at = active_mac_activation_time(activation.activated_at)?;
        let usage_sync = if replaces_profile {
            UsageSyncCommit::ReplaceAuthority {
                activated_at,
                generation: activation.generation,
            }
        } else {
            UsageSyncCommit::Activate {
                activated_at,
                generation: activation.generation,
            }
        };
        self.commit_snapshot_with_usage_sync(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions {
                force_revision: true,
                ..SnapshotCommitOptions::default()
            },
            usage_sync,
        )
    }

    fn commit_provider_enablement(
        &self,
        store: &mut ReadModelStore,
        change: Option<ProviderEnablementChange>,
        now: OffsetDateTime,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, mut first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        let mut refreshed = cached.clone();
        let previous_generated_at =
            OffsetDateTime::parse(&refreshed.generated_at, &Rfc3339).unwrap_or(now);
        if let Some(change) = change
            && change.enabled
            && let Some(presentation) = refreshed.provider_mut(change.provider)
        {
            let (reenabled, _) = presentation.transition_at(previous_generated_at, now);
            let waits_for_first_observation = !reenabled.has_cached_quota_or_observed_usage()
                && (first_observation_waits.contains(&change.provider)
                    || reenabled.usage.scan_status != UsageScanStatus::Indexing);
            if waits_for_first_observation {
                first_observation_waits.insert(change.provider);
            } else {
                first_observation_waits.remove(&change.provider);
            }
            *presentation = reenabled;
        }
        refreshed.refresh_combined_usage();
        self.commit_snapshot_with_force(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions {
                force_revision: change.is_some(),
                provider_enablement_change: change,
            },
        )
    }

    fn commit_snapshot_with_force(
        &self,
        store: &mut ReadModelStore,
        refreshed: SanitizedDesktopStateV3,
        cached: SanitizedDesktopStateV3,
        first_observation_waits: BTreeSet<CodingProvider>,
        now: OffsetDateTime,
        options: SnapshotCommitOptions,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        self.commit_snapshot_with_force_and_corrections(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            options,
            UsageSyncCorrections::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_snapshot_with_force_and_corrections(
        &self,
        store: &mut ReadModelStore,
        refreshed: SanitizedDesktopStateV3,
        cached: SanitizedDesktopStateV3,
        first_observation_waits: BTreeSet<CodingProvider>,
        now: OffsetDateTime,
        options: SnapshotCommitOptions,
        corrections: UsageSyncCorrections,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        self.commit_snapshot_with_usage_sync(
            store,
            refreshed,
            cached,
            first_observation_waits,
            now,
            options,
            UsageSyncCommit::QueueCurrent(corrections),
        )
    }

    fn commit_usage_sync(
        &self,
        store: &mut ReadModelStore,
        now: OffsetDateTime,
        usage_sync: UsageSyncCommit<'_>,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let (cached, first_observation_waits) = self.snapshot_with_first_observation_waits()?;
        self.commit_snapshot_with_usage_sync(
            store,
            cached.clone(),
            cached,
            first_observation_waits,
            now,
            SnapshotCommitOptions {
                force_revision: true,
                ..SnapshotCommitOptions::default()
            },
            usage_sync,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_snapshot_with_usage_sync(
        &self,
        store: &mut ReadModelStore,
        mut refreshed: SanitizedDesktopStateV3,
        cached: SanitizedDesktopStateV3,
        first_observation_waits: BTreeSet<CodingProvider>,
        now: OffsetDateTime,
        options: SnapshotCommitOptions,
        usage_sync: UsageSyncCommit<'_>,
    ) -> Result<SnapshotCommitOutcome, &'static str> {
        let SnapshotCommitOptions {
            force_revision,
            provider_enablement_change,
        } = options;
        let previous_enabled_providers = self.enabled_providers()?;
        let mut enabled_providers = previous_enabled_providers.clone();
        if let Some(change) = provider_enablement_change {
            if change.enabled {
                enabled_providers.insert(change.provider);
            } else {
                enabled_providers.remove(&change.provider);
            }
        }
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
                pending_usage_changed: false,
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
        let (persistence_failed, pending_usage_changed) = match store {
            ReadModelStore::Persistent(persistent) => {
                match persistent.commit(
                    &mut refreshed,
                    now,
                    usage_sync,
                    &previous_enabled_providers,
                    &enabled_providers,
                ) {
                    Ok(pending_usage_changed) => (false, pending_usage_changed),
                    Err(_) => {
                        *store = ReadModelStore::Memory;
                        (true, false)
                    }
                }
            }
            ReadModelStore::Memory => {
                refreshed.sync.status = SyncStatus::Unavailable;
                (false, false)
            }
        };
        if persistence_failed {
            refreshed.sync.status = SyncStatus::Unavailable;
        }
        validate_snapshot(&refreshed)?;
        let mut state = self.state.lock().map_err(|_| "native state unavailable")?;
        *state = CachedProjectionState {
            snapshot: refreshed,
            first_observation_waits,
            enabled_providers,
        };
        Ok(SnapshotCommitOutcome {
            notice: Some(RevisionNotice {
                revision: revision.to_string(),
            }),
            pending_usage_changed,
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
    provider_settings_generation: Arc<AtomicU64>,
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
    fn unavailable(subscribers: Arc<RevisionSubscribers>) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        Self {
            inbox: Arc::new(RefreshInbox {
                admission: Mutex::new(()),
                pending_sources: AtomicU8::new(0),
                provider_settings_pending: AtomicBool::new(false),
                provider_settings_generation: Arc::new(AtomicU64::new(0)),
                in_flight: AtomicBool::new(false),
                paused: AtomicBool::new(true),
                stopping: AtomicBool::new(true),
                wake,
            }),
            cancelled: Arc::new(AtomicBool::new(true)),
            refresh_adapter: Arc::new(UnavailableRefreshAdapter),
            worker: Mutex::new(None),
            subscribers,
        }
    }

    fn start(
        projection: Arc<CachedProjection>,
        store: Arc<Mutex<ReadModelStore>>,
        subscribers: Arc<RevisionSubscribers>,
        usage_sync_requests: Arc<UsageSyncRequests>,
        clock: Arc<dyn Clock>,
        refresh_adapter: Arc<dyn SnapshotRefreshAdapter>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let (wake, wake_receiver) = mpsc::sync_channel(1);
        let inbox = Arc::new(RefreshInbox {
            admission: Mutex::new(()),
            pending_sources: AtomicU8::new(0),
            provider_settings_pending: AtomicBool::new(false),
            provider_settings_generation: Arc::new(AtomicU64::new(0)),
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
            usage_sync_requests,
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

    fn cancel_provider(&self, provider: CodingProvider) {
        self.refresh_adapter.cancel_provider(provider);
    }

    fn note_provider_setting_commit(&self) {
        self.inbox
            .provider_settings_generation
            .fetch_add(1, Ordering::AcqRel);
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
    Completed { failed: bool },
    Cancelled,
}

#[derive(Default)]
struct SnapshotRefreshProgressState {
    persistence_failed: bool,
}

struct NativeSnapshotRefreshProgress {
    projection: Arc<CachedProjection>,
    store: Arc<Mutex<ReadModelStore>>,
    subscribers: Arc<RevisionSubscribers>,
    usage_sync_requests: Arc<UsageSyncRequests>,
    clock: Arc<dyn Clock>,
    inbox: Arc<RefreshInbox>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
    attempt: RefreshAttempt,
    provider_settings_generation: u64,
    state: Mutex<SnapshotRefreshProgressState>,
}

impl NativeSnapshotRefreshProgress {
    fn persistence_failed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .persistence_failed
    }
}

impl SnapshotRefreshProgress for NativeSnapshotRefreshProgress {
    fn report(&self, outcome: SnapshotRefreshOutcome) -> Result<(), RefreshFailure> {
        if outcome.is_empty() {
            return Ok(());
        }
        self.attempt.remaining()?;
        if self
            .inbox
            .provider_settings_generation
            .load(Ordering::Acquire)
            != self.provider_settings_generation
        {
            return Ok(());
        }

        let completed_at = self.clock.now();
        let refreshed = match outcome.snapshot {
            Some(refreshed) => {
                transition_snapshot_at(&refreshed, completed_at).unwrap_or(refreshed)
            }
            None => self
                .projection
                .snapshot()
                .map_err(|_| RefreshFailure::SourceUnavailable)?,
        };
        let completed_providers = outcome
            .completed_providers
            .into_iter()
            .filter(|provider| self.enablement.is_provider_enabled(*provider))
            .collect::<BTreeSet<_>>();
        let corrections = outcome
            .corrections
            .into_iter()
            .filter(|(provider, _)| self.enablement.is_provider_enabled(*provider))
            .collect::<BTreeMap<_, _>>();
        let mut store = self
            .store
            .lock()
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        if self
            .inbox
            .provider_settings_generation
            .load(Ordering::Acquire)
            != self.provider_settings_generation
        {
            return Ok(());
        }
        let commit = self
            .projection
            .commit_refreshed_snapshot_with_completed(
                &mut store,
                refreshed,
                self.enablement.as_ref(),
                completed_at,
                &completed_providers,
                &corrections,
            )
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        drop(store);

        if commit.persistence_failed {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .persistence_failed = true;
        }
        if let Some(notice) = commit.notice {
            self.subscribers.publish(notice);
        }
        if commit.pending_usage_changed {
            self.usage_sync_requests.request();
        }
        Ok(())
    }
}

struct CoordinatorWorker {
    projection: Arc<CachedProjection>,
    store: Arc<Mutex<ReadModelStore>>,
    subscribers: Arc<RevisionSubscribers>,
    usage_sync_requests: Arc<UsageSyncRequests>,
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
            match result {
                RefreshRunResult::Completed { failed } => {
                    if sources.contains(RefreshSource::LocalUsageCatchUp) {
                        self.record_local_usage_catch_up_result(failed, refresh_started.elapsed());
                    }
                    if !sources.is_only(RefreshSource::LocalUsageCatchUp) {
                        self.record_refresh_result(failed, self.clock.now());
                    }
                }
                RefreshRunResult::Cancelled => {}
            }
            self.inbox.in_flight.store(false, Ordering::Release);
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
        let delay = local_usage_catch_up_delay(failed);
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
            self.usage_sync_requests.request();
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
        let provider_settings_generation = self
            .inbox
            .provider_settings_generation
            .load(Ordering::Acquire);
        let mut cached = match self.projection.snapshot() {
            Ok(cached) => cached,
            Err(_) => {
                return RefreshRunResult::Completed { failed: true };
            }
        };
        let mut pre_refresh_failed = false;
        if let Some(transitioned) = transition_snapshot_at(&cached, self.clock.now()) {
            let transition = self
                .store
                .lock()
                .map_err(|_| "native state unavailable")
                .and_then(|mut store| {
                    self.projection.commit_transitioned_snapshot(
                        &mut store,
                        transitioned,
                        self.clock.now(),
                    )
                });
            match transition {
                Ok(outcome) => {
                    pre_refresh_failed = outcome.persistence_failed;
                    if let Some(notice) = outcome.notice {
                        self.subscribers.publish(notice);
                    }
                    if outcome.pending_usage_changed {
                        self.usage_sync_requests.request();
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
        let progress = NativeSnapshotRefreshProgress {
            projection: Arc::clone(&self.projection),
            store: Arc::clone(&self.store),
            subscribers: Arc::clone(&self.subscribers),
            usage_sync_requests: Arc::clone(&self.usage_sync_requests),
            clock: Arc::clone(&self.clock),
            inbox: Arc::clone(&self.inbox),
            enablement: Arc::clone(&self.enablement),
            attempt: attempt.clone(),
            provider_settings_generation,
            state: Mutex::new(SnapshotRefreshProgressState::default()),
        };
        let observation = catch_unwind(AssertUnwindSafe(|| {
            self.refresh_adapter
                .refresh_with_progress(cached, &attempt, &progress)
        }));
        if attempt.is_cancelled() {
            return RefreshRunResult::Cancelled;
        }

        let mut source_failed = match observation {
            Ok(Ok(outcome)) if attempt.remaining().is_ok() => match progress.report(outcome) {
                Ok(()) => false,
                Err(RefreshFailure::Cancelled) => return RefreshRunResult::Cancelled,
                Err(_) => true,
            },
            Ok(Err(RefreshFailure::Cancelled)) => return RefreshRunResult::Cancelled,
            Ok(Err(_)) | Ok(Ok(_)) | Err(_) => true,
        };
        if attempt.is_cancelled() {
            return RefreshRunResult::Cancelled;
        }
        if !source_failed {
            let transition = self
                .projection
                .snapshot()
                .map_err(|_| RefreshFailure::SourceUnavailable)
                .and_then(|current| {
                    let Some(transitioned) = transition_snapshot_at(&current, self.clock.now())
                    else {
                        return Ok(());
                    };
                    progress.report(SnapshotRefreshOutcome::from(Some(transitioned)))
                });
            match transition {
                Ok(()) => {}
                Err(RefreshFailure::Cancelled) => return RefreshRunResult::Cancelled,
                Err(_) => source_failed = true,
            }
        }
        RefreshRunResult::Completed {
            failed: pre_refresh_failed || source_failed || progress.persistence_failed(),
        }
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

fn local_usage_catch_up_delay(failed: bool) -> Duration {
    if failed {
        LOCAL_USAGE_CATCH_UP_ERROR_DELAY
    } else {
        LOCAL_USAGE_CATCH_UP_SUCCESS_DELAY
    }
}

struct NativeCoreInner {
    projection: Arc<CachedProjection>,
    store: Arc<Mutex<ReadModelStore>>,
    subscribers: Arc<RevisionSubscribers>,
    usage_sync_requests: Arc<UsageSyncRequests>,
    coordinator: RefreshCoordinator,
    clock: Arc<dyn Clock>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
}

#[derive(Clone)]
pub struct NativeCore {
    inner: Arc<NativeCoreInner>,
}

fn enabled_provider_set(enablement: &dyn ProviderEnablementPolicy) -> BTreeSet<CodingProvider> {
    PROVIDER_REGISTRY
        .iter()
        .filter(|descriptor| enablement.is_provider_enabled(descriptor.provider))
        .map(|descriptor| descriptor.provider)
        .collect()
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
    pub(crate) fn open_with(
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
        let projection = Arc::new(CachedProjection::new(
            state,
            enabled_provider_set(enablement.as_ref()),
        ));
        if let Some(transitioned) = restore_snapshot_at(&projection.snapshot()?, now) {
            projection.commit_transitioned_snapshot(&mut store, transitioned, now)?;
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

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn no_io_unavailable() -> Self {
        Self::no_io_unavailable_with_provider_enablement(all_providers_enabled_policy())
    }

    pub(crate) fn no_io_unavailable_with_provider_enablement(
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let mut state = unavailable_state_at(1, clock.now());
        drop(state.apply_provider_enablement(enablement.as_ref()));
        let projection = Arc::new(CachedProjection::new(
            state,
            enabled_provider_set(enablement.as_ref()),
        ));
        let subscribers = Arc::new(RevisionSubscribers::new());
        let usage_sync_requests = Arc::new(UsageSyncRequests::default());
        Self {
            inner: Arc::new(NativeCoreInner {
                projection,
                store: Arc::new(Mutex::new(ReadModelStore::Memory)),
                coordinator: RefreshCoordinator::unavailable(Arc::clone(&subscribers)),
                subscribers,
                usage_sync_requests,
                clock,
                enablement,
            }),
        }
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
        let enabled_providers = enabled_provider_set(enablement.as_ref());
        Self::from_components(
            Arc::new(CachedProjection::new(state, enabled_providers)),
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
        let usage_sync_requests = Arc::new(UsageSyncRequests::default());
        let store = Arc::new(Mutex::new(store));
        let coordinator = RefreshCoordinator::start(
            Arc::clone(&projection),
            Arc::clone(&store),
            Arc::clone(&subscribers),
            Arc::clone(&usage_sync_requests),
            Arc::clone(&clock),
            refresh_adapter,
            Arc::clone(&enablement),
        );
        Self {
            inner: Arc::new(NativeCoreInner {
                projection,
                store,
                subscribers,
                usage_sync_requests,
                coordinator,
                clock,
                enablement,
            }),
        }
    }

    /// Returns the complete panel projection with provider visibility already
    /// applied by the native policy.
    pub fn panel_state(&self) -> Result<SanitizedDesktopStateV3, &'static str> {
        let mut snapshot = self.inner.projection.panel_snapshot()?;
        drop(snapshot.apply_provider_enablement(self.inner.enablement.as_ref()));
        Ok(snapshot)
    }

    /// Returns one menu-bar projection from the committed in-memory snapshot.
    /// This method does not perform provider, disk, Keychain, or network I/O.
    pub(crate) fn menu_bar_headroom(&self) -> Result<RevisionedOverallQuotaHeadroom, &'static str> {
        let (snapshot, enabled_providers) = self.inner.projection.menu_bar_snapshot()?;
        let revision = snapshot
            .revision
            .parse::<u64>()
            .map_err(|_| "native revision unavailable")?;
        let enabled_quotas = PROVIDER_REGISTRY
            .iter()
            .filter(|descriptor| enabled_providers.contains(&descriptor.provider))
            .map(|descriptor| {
                snapshot
                    .provider(descriptor.provider)
                    .map(|provider| provider.quota.clone())
                    .unwrap_or(ProviderSnapshot::Unavailable {
                        provider: descriptor.provider,
                        quota_lanes: [],
                    })
            })
            .collect::<Vec<_>>();
        Ok(RevisionedOverallQuotaHeadroom {
            revision,
            headroom: overall_quota_headroom(enabled_quotas.iter(), self.inner.clock.now()),
        })
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
        let commit = self.inner.projection.commit_provider_enablement(
            &mut store,
            Some(ProviderEnablementChange { provider, enabled }),
            self.inner.clock.now(),
        )?;
        self.inner.coordinator.note_provider_setting_commit();
        drop(store);
        if let Some(notice) = commit.notice {
            self.inner.subscribers.publish(notice);
        }
        self.inner.usage_sync_requests.request();
        if !enabled {
            self.inner.coordinator.cancel_provider(provider);
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
        if outcome.pending_usage_changed {
            self.inner.usage_sync_requests.request();
        }
        Ok(())
    }

    pub(crate) fn install_usage_sync_authority(
        &self,
        active_mac_generation: u64,
        active_mac_activated_at: u64,
    ) -> Result<(), &'static str> {
        let active_mac_activated_at = active_mac_activation_time(active_mac_activated_at)?;
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let already_active = match &*store {
            ReadModelStore::Persistent(persistent)
                if persistent.active_mac_generation == Some(active_mac_generation) =>
            {
                true
            }
            ReadModelStore::Persistent(_) => false,
            ReadModelStore::Memory => return Err("native state persistence unavailable"),
        };
        let notice = if already_active {
            let state = self.inner.projection.snapshot()?;
            let ReadModelStore::Persistent(persistent) = &mut *store else {
                return Err("native state persistence unavailable");
            };
            persistent.confirm_active_mac_activation(
                active_mac_generation,
                active_mac_activated_at,
                self.inner.clock.now(),
                &state,
            )?;
            None
        } else {
            self.inner
                .projection
                .commit_usage_sync(
                    &mut store,
                    self.inner.clock.now(),
                    UsageSyncCommit::Activate {
                        activated_at: active_mac_activated_at,
                        generation: active_mac_generation,
                    },
                )?
                .notice
        };
        drop(store);
        if let Some(notice) = notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    pub(crate) fn recover_profile_authority(
        &self,
        profile: SanitizedProfileOutcome,
        active_mac_generation: u64,
        active_mac_activated_at: u64,
    ) -> Result<(), &'static str> {
        let activation = ActiveMacActivation {
            activated_at: active_mac_activated_at,
            generation: active_mac_generation,
        };
        let active_mac_activated_at = active_mac_activation_time(active_mac_activated_at)?;
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        if matches!(&*store, ReadModelStore::Memory) {
            return Err("native state persistence unavailable");
        }
        let state = self.inner.projection.snapshot()?;
        let already_committed = state.profile == profile
            && matches!(
                &*store,
                ReadModelStore::Persistent(persistent)
                    if persistent.active_mac_generation == Some(active_mac_generation)
            );
        let outcome = if already_committed {
            let ReadModelStore::Persistent(persistent) = &mut *store else {
                return Err("native state persistence unavailable");
            };
            persistent.confirm_active_mac_activation(
                active_mac_generation,
                active_mac_activated_at,
                self.inner.clock.now(),
                &state,
            )?;
            SnapshotCommitOutcome {
                notice: None,
                pending_usage_changed: false,
                persistence_failed: false,
            }
        } else {
            self.inner.projection.commit_profile_recovery(
                &mut store,
                profile,
                activation,
                self.inner.clock.now(),
            )?
        };
        if outcome.persistence_failed {
            return Err("native state persistence unavailable");
        }
        drop(store);
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        if outcome.pending_usage_changed {
            self.inner.usage_sync_requests.request();
        }
        Ok(())
    }

    pub(crate) fn prepare_usage_sync_attempt(
        &self,
        active_mac_generation: u64,
        active_mac_activated_at: u64,
    ) -> Result<Option<PendingUsageBatch>, &'static str> {
        self.install_usage_sync_authority(active_mac_generation, active_mac_activated_at)?;
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let enabled_providers = self.inner.projection.enabled_providers()?;
        match &*store {
            ReadModelStore::Persistent(persistent) => persistent.pending_usage_batch(
                active_mac_generation,
                self.inner.clock.now(),
                &enabled_providers,
            ),
            ReadModelStore::Memory => Ok(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn activate_usage_sync_generation(
        &self,
        active_mac_generation: u64,
    ) -> Result<(), &'static str> {
        let activated_at = self
            .inner
            .clock
            .now()
            .unix_timestamp_nanos()
            .div_euclid(1_000_000);
        let activated_at =
            u64::try_from(activated_at).map_err(|_| "native state persistence unavailable")?;
        self.install_usage_sync_authority(active_mac_generation, activated_at)
    }

    #[cfg(test)]
    pub(crate) fn active_usage_sync_generation(&self) -> Result<Option<u64>, &'static str> {
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        match &*store {
            ReadModelStore::Persistent(persistent) => Ok(persistent.active_mac_generation),
            ReadModelStore::Memory => Ok(None),
        }
    }

    pub(crate) fn usage_sync_authority_identity(
        &self,
    ) -> Result<UsageSyncAuthorityIdentity, &'static str> {
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let active_mac_generation = match &*store {
            ReadModelStore::Persistent(persistent) => persistent.active_mac_generation,
            ReadModelStore::Memory => None,
        };
        let touch_grass_id = match self.inner.projection.snapshot()?.profile {
            SanitizedProfileOutcome::Ready { touch_grass_id, .. } => Some(touch_grass_id),
            SanitizedProfileOutcome::NotAuthorized | SanitizedProfileOutcome::ProfilePending => {
                None
            }
        };
        Ok(UsageSyncAuthorityIdentity {
            active_mac_generation,
            touch_grass_id,
        })
    }

    #[cfg(test)]
    pub(crate) fn pending_usage_sync_batch(
        &self,
        active_mac_generation: u64,
    ) -> Result<Option<PendingUsageBatch>, &'static str> {
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let enabled_providers = self.inner.projection.enabled_providers()?;
        match &*store {
            ReadModelStore::Persistent(persistent) => persistent.pending_usage_batch(
                active_mac_generation,
                self.inner.clock.now(),
                &enabled_providers,
            ),
            ReadModelStore::Memory => Ok(None),
        }
    }

    pub(crate) fn finish_usage_sync_attempt(
        &self,
        batch: &PendingUsageBatch,
        result: UsageSyncAttemptResult,
    ) -> Result<(), &'static str> {
        if matches!(result, UsageSyncAttemptResult::Offline)
            && self.inner.projection.snapshot()?.sync.status == SyncStatus::Offline
        {
            return Ok(());
        }
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let enabled_providers = self.inner.projection.enabled_providers()?;
        if matches!(result, UsageSyncAttemptResult::Deferred)
            && self.inner.projection.snapshot()?.sync.status == SyncStatus::Pending
            && matches!(
                &*store,
                ReadModelStore::Persistent(persistent)
                    if persistent
                        .pending_usage_batch(
                            batch.active_mac_generation(),
                            self.inner.clock.now(),
                            &enabled_providers,
                        )?
                        .is_some()
            )
        {
            return Ok(());
        }
        let usage_sync = match &result {
            UsageSyncAttemptResult::Committed(acknowledgements) => UsageSyncCommit::Acknowledge {
                acknowledgements,
                batch,
            },
            UsageSyncAttemptResult::Offline => UsageSyncCommit::Offline,
            UsageSyncAttemptResult::Deferred => UsageSyncCommit::Pending,
        };
        let outcome = self.inner.projection.commit_usage_sync(
            &mut store,
            self.inner.clock.now(),
            usage_sync,
        )?;
        drop(store);
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        if outcome.pending_usage_changed {
            self.inner.usage_sync_requests.request();
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn acknowledge_usage_sync(
        &self,
        batch: &PendingUsageBatch,
        acknowledgements: &[UsageSyncAcknowledgement],
    ) -> Result<(), &'static str> {
        self.finish_usage_sync_attempt(
            batch,
            UsageSyncAttemptResult::Committed(UsageSyncAcknowledgements {
                provider_settings: batch.provider_settings().map(|settings| {
                    ProviderSettingsAcknowledgement {
                        revision: settings.revision(),
                        outcome: crate::usage_sync::AcknowledgementOutcome::Committed,
                    }
                }),
                usage: acknowledgements.to_vec(),
                usage_mutation_completed: batch.requires_usage_mutation(),
            }),
        )
    }

    #[cfg(test)]
    pub(crate) fn mark_usage_sync_offline(&self) -> Result<(), &'static str> {
        if self.inner.projection.snapshot()?.sync.status == SyncStatus::Offline {
            return Ok(());
        }
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let outcome = self.inner.projection.commit_usage_sync(
            &mut store,
            self.inner.clock.now(),
            UsageSyncCommit::Offline,
        )?;
        drop(store);
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_usage_sync_pending(&self) -> Result<(), &'static str> {
        if self.inner.projection.snapshot()?.sync.status == SyncStatus::Pending
            && let Some(generation) = self.active_usage_sync_generation()?
            && self.pending_usage_sync_batch(generation)?.is_some()
        {
            return Ok(());
        }
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let outcome = self.inner.projection.commit_usage_sync(
            &mut store,
            self.inner.clock.now(),
            UsageSyncCommit::Pending,
        )?;
        drop(store);
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    pub(crate) fn reject_usage_sync_authority_if_current(
        &self,
        expected_authority: &UsageSyncAuthorityIdentity,
    ) -> Result<(), &'static str> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| "native state unavailable")?;
        let active_mac_generation = match &*store {
            ReadModelStore::Persistent(persistent) => persistent.active_mac_generation,
            ReadModelStore::Memory => None,
        };
        let touch_grass_id = match self.inner.projection.snapshot()?.profile {
            SanitizedProfileOutcome::Ready { touch_grass_id, .. } => Some(touch_grass_id),
            SanitizedProfileOutcome::NotAuthorized | SanitizedProfileOutcome::ProfilePending => {
                None
            }
        };
        if *expected_authority
            != (UsageSyncAuthorityIdentity {
                active_mac_generation,
                touch_grass_id,
            })
        {
            return Ok(());
        }
        if self.inner.projection.snapshot()?.sync.status == SyncStatus::AuthorityRejected {
            match active_mac_generation {
                Some(active_mac_generation)
                    if matches!(
                        &*store,
                        ReadModelStore::Persistent(persistent)
                            if persistent.active_mac_generation == Some(active_mac_generation)
                                && load_usage_sync_generation_state(
                                    &persistent.connection,
                                    active_mac_generation,
                                )
                                .is_ok_and(|state| state == Some(QueueState::Blocked))
                    ) =>
                {
                    return Ok(());
                }
                None => return Ok(()),
                Some(_) => {}
            }
        }
        let outcome = self.inner.projection.commit_usage_sync(
            &mut store,
            self.inner.clock.now(),
            UsageSyncCommit::AuthorityRejected(active_mac_generation),
        )?;
        drop(store);
        if let Some(notice) = outcome.notice {
            self.inner.subscribers.publish(notice);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn reject_active_usage_sync_authority(&self) -> Result<(), &'static str> {
        self.reject_usage_sync_authority_if_current(&self.usage_sync_authority_identity()?)
    }

    #[cfg(test)]
    pub(crate) fn mark_usage_sync_authority_rejected(
        &self,
        active_mac_generation: u64,
    ) -> Result<(), &'static str> {
        if self.active_usage_sync_generation()? != Some(active_mac_generation) {
            return Err("native state persistence unavailable");
        }
        self.reject_active_usage_sync_authority()
    }

    pub fn revision_notices(&self) -> Result<Receiver<RevisionNotice>, &'static str> {
        self.inner.subscribers.subscribe()
    }

    pub fn request_refresh(&self, source: RefreshSource) -> Result<RefreshReceipt, &'static str> {
        self.inner.coordinator.request(source)
    }

    pub(crate) fn wait_for_refresh_completion(&self) -> Result<(), &'static str> {
        let deadline = Instant::now() + REFRESH_ATTEMPT_TIMEOUT + Duration::from_secs(5);
        loop {
            if self
                .inner
                .coordinator
                .inbox
                .stopping
                .load(Ordering::Acquire)
            {
                return Err("refresh coordinator unavailable");
            }
            let pending = self
                .inner
                .coordinator
                .inbox
                .pending_sources
                .load(Ordering::Acquire);
            let in_flight = self
                .inner
                .coordinator
                .inbox
                .in_flight
                .load(Ordering::Acquire);
            if pending == 0 && !in_flight {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("refresh completion unavailable");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub(crate) fn request_provider_refresh(&self) -> Result<RefreshReceipt, &'static str> {
        self.inner.coordinator.request_provider_refresh()
    }

    pub(crate) fn install_usage_sync_request(
        &self,
        request: UsageSyncRequest,
    ) -> Result<(), &'static str> {
        self.inner.usage_sync_requests.install(request)
    }

    pub(crate) fn clear_usage_sync_request(&self) {
        self.inner.usage_sync_requests.clear();
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

pub(crate) fn read_model_schema_version(connection: &Connection) -> Result<i64, &'static str> {
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

pub(crate) fn prepare_database(path: &Path) -> Result<(), &'static str> {
    let initial = unavailable_state(1);
    let _ = SqliteReadModelStore::open(path, &initial)?;
    Ok(())
}

pub(crate) fn read_database_state(
    connection: &Connection,
) -> Result<SanitizedDesktopStateV3, &'static str> {
    SqliteReadModelStore::read_from(connection)
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
        3..=5 => 3,
        6..=7 => i64::from(CONTRACT_VERSION),
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
    pub(crate) fn is_active_at(&self, now: OffsetDateTime) -> bool {
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

    fn reset_crossed_since(
        &self,
        previous_generated_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> bool {
        self.reset_at.as_deref().is_some_and(|reset_at| {
            OffsetDateTime::parse(reset_at, &Rfc3339)
                .is_ok_and(|reset_at| previous_generated_at < reset_at && reset_at <= now)
        })
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
                    || quota_lanes.iter().any(|lane| !lane.is_active_at(now))
            }
        }
    }

    fn transition_at(
        &self,
        previous_generated_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> (Self, bool) {
        match self {
            Self::Unavailable { .. } => (self.clone(), false),
            Self::Stale { quota_lanes, .. }
                if quota_lanes
                    .iter()
                    .any(|lane| lane.reset_crossed_since(previous_generated_at, now)) =>
            {
                (self.clone(), true)
            }
            Self::Stale { .. } => (self.clone(), false),
            Self::Current {
                provider,
                observed_at,
                quota_lanes,
            } if timestamp_is_due(observed_at, now)
                || quota_lanes.iter().any(|lane| !lane.is_active_at(now)) =>
            {
                (
                    Self::Stale {
                        provider: *provider,
                        observed_at: observed_at.clone(),
                        quota_lanes: quota_lanes.clone(),
                    },
                    true,
                )
            }
            Self::Current { .. } => (self.clone(), false),
        }
    }

    fn transition_on_restore(&self) -> (Self, bool) {
        match self {
            Self::Current {
                provider,
                observed_at,
                quota_lanes,
            } => (
                Self::Stale {
                    provider: *provider,
                    observed_at: observed_at.clone(),
                    quota_lanes: quota_lanes.clone(),
                },
                true,
            ),
            Self::Unavailable { .. } | Self::Stale { .. } => (self.clone(), false),
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

    fn transition_at(
        &self,
        previous_generated_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> (Self, bool) {
        let (quota, quota_changed) = self.quota.transition_at(previous_generated_at, now);
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
    let previous_generated_at =
        OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339).unwrap_or(now);
    let mut changed = false;
    let providers = snapshot
        .providers
        .iter()
        .map(|provider| {
            let (provider, provider_changed) = provider.transition_at(previous_generated_at, now);
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

fn restore_snapshot_at(
    snapshot: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
) -> Option<SanitizedDesktopStateV3> {
    let previous_generated_at =
        OffsetDateTime::parse(&snapshot.generated_at, &Rfc3339).unwrap_or(now);
    let mut changed = false;
    let providers = snapshot
        .providers
        .iter()
        .map(|provider| {
            let (mut restored, time_changed) = provider.transition_at(previous_generated_at, now);
            let (quota, restore_changed) = restored.quota.transition_on_restore();
            restored.quota = quota;
            changed |= time_changed || restore_changed;
            restored
        })
        .collect::<Vec<_>>();
    changed.then(|| {
        let mut restored = snapshot.clone();
        restored.providers = providers;
        restored.refresh_combined_usage();
        restored
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
        || snapshot
            .sync
            .last_successful_at
            .as_ref()
            .is_some_and(|value| OffsetDateTime::parse(value, &Rfc3339).is_err())
        || (matches!(snapshot.sync.status, SyncStatus::Synced | SyncStatus::Stale)
            && snapshot.sync.last_successful_at.is_none())
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
        validate_top_model_usage(presentation.top_model_usage.as_ref())?;
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
    validate_top_model_usage(snapshot.top_model_usage.as_ref())?;
    let expected_top_model = combined_top_model_usage(
        &snapshot
            .providers
            .iter()
            .filter(|presentation| presentation.is_visible())
            .collect::<Vec<_>>(),
    );
    if snapshot.top_model_usage != expected_top_model {
        return Err("native state unavailable");
    }
    Ok(())
}

fn validate_top_model_usage(usage: Option<&TopModelUsage>) -> Result<(), &'static str> {
    let Some(usage) = usage else {
        return Ok(());
    };
    if usage.observed_tokens == 0
        || usage.model.as_ref().is_some_and(|model| {
            model.is_empty()
                || model.len() > 48
                || !model.bytes().all(|character| {
                    character.is_ascii_alphanumeric() || character == b' ' || character == b'.'
                })
        })
    {
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
        top_model_usage: None,
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
        "addTokenmaxxerContractVersion": ADD_TOKENMAXXER_CONTRACT_VERSION,
        "addTokenmaxxerOutcomeSchema": add_tokenmaxxer_outcome_schema(),
        "bootstrapContractVersion": LIFECYCLE_CONTRACT_VERSION,
        "bootstrapStateSchema": bootstrap_state_schema(),
        "contractVersion": CONTRACT_VERSION,
        "doomerboardContractVersion": DOOMERBOARD_CONTRACT_VERSION,
        "doomerboardViewSchema": doomerboard_view_schema(),
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
mod product_acceptance;

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
                read_model_backup_path(&self.0, 5),
                read_model_backup_partial_path(&self.0, 5),
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
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
            response.map(SnapshotRefreshOutcome::from)
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
            provider_settings_generation: Arc::new(AtomicU64::new(0)),
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
                    top_model_usage: None,
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
                    top_model_usage: None,
                },
            ],
            top_model_usage: None,
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

    fn claude_observed_state(
        observed_at: OffsetDateTime,
        observed_tokens: u64,
    ) -> SanitizedDesktopStateV3 {
        let mut state = observed_state(observed_at, observed_tokens);
        let mut usage = state
            .provider(CodingProvider::Codex)
            .expect("Codex fixture")
            .usage
            .clone();
        let UsageTotal::Current { evidence_basis, .. } = &mut usage.today else {
            panic!("fixture usage must be current");
        };
        *evidence_basis = UsageEvidenceBasis::LocallyDerived;
        state
            .provider_mut(CodingProvider::Codex)
            .expect("Codex fixture")
            .usage = unavailable_periods();
        let claude = state
            .provider_mut(CodingProvider::Claude)
            .expect("Claude fixture");
        claude.presence = ProviderPresenceStatus::Detected;
        claude.usage = usage;
        state.refresh_combined_usage();
        state
    }

    fn claude_provider_reported_state(
        observed_at: OffsetDateTime,
        observed_tokens: u64,
    ) -> SanitizedDesktopStateV3 {
        let mut state = claude_observed_state(observed_at, observed_tokens);
        let UsageTotal::Current { evidence_basis, .. } = &mut state
            .provider_mut(CodingProvider::Claude)
            .expect("Claude fixture")
            .usage
            .today
        else {
            panic!("Claude fixture usage must be current");
        };
        *evidence_basis = UsageEvidenceBasis::ProviderReported;
        state.refresh_combined_usage();
        state
    }

    fn both_provider_observed_state(
        observed_at: OffsetDateTime,
        codex_tokens: u64,
        codex_cost_usd: f64,
        claude_tokens: u64,
        claude_cost_usd: f64,
    ) -> SanitizedDesktopStateV3 {
        let mut state = observed_state(observed_at, codex_tokens);
        let codex_usage = &mut state
            .provider_mut(CodingProvider::Codex)
            .expect("Codex fixture")
            .usage;
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut codex_usage.today
        else {
            panic!("Codex fixture usage must be current");
        };
        *api_equivalent_cost_usd = Some(codex_cost_usd);
        *api_equivalent_cost_basis = Some("openai-api-2026-08-09-v3".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::Reconciled);

        let mut claude_usage = codex_usage.clone();
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
        *observed_tokens = claude_tokens;
        *api_equivalent_cost_usd = Some(claude_cost_usd);
        *api_equivalent_cost_basis = Some("anthropic-standard-2026-08-07-v1".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::LocalOnly);
        let claude = state
            .provider_mut(CodingProvider::Claude)
            .expect("Claude fixture");
        claude.presence = ProviderPresenceStatus::Detected;
        claude.usage = claude_usage;
        state.refresh_combined_usage();
        state
    }

    struct CorrectionRefreshSource {
        responses: Mutex<VecDeque<SnapshotRefreshOutcome>>,
    }

    impl SnapshotRefreshAdapter for CorrectionRefreshSource {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            attempt: &RefreshAttempt,
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
            attempt.remaining()?;
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default())
        }
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

    fn expected_headroom(
        remaining_percent: f64,
        freshness: crate::quota_headroom::HeadroomFreshness,
        completeness: crate::quota_headroom::HeadroomCompleteness,
    ) -> crate::quota_headroom::OverallQuotaHeadroom {
        crate::quota_headroom::OverallQuotaHeadroom::Calculated {
            remaining_percent,
            freshness,
            completeness,
        }
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
    fn sync_state_uses_only_sanitized_outcomes() {
        assert_eq!(
            serde_json::to_value(SyncStatus::AuthorityRejected).unwrap(),
            json!("authority-rejected")
        );
        assert_eq!(
            serde_json::to_value(SyncStatus::Offline).unwrap(),
            json!("offline")
        );

        let mut state = observed_state(test_time(), 42);
        state.sync.status = SyncStatus::Synced;
        assert!(validate_snapshot(&state).is_err());

        state.sync.last_successful_at = Some("not-a-time".to_owned());
        assert!(validate_snapshot(&state).is_err());

        state.sync.last_successful_at = Some(format_time(test_time()));
        assert!(validate_snapshot(&state).is_ok());

        state.sync.status = SyncStatus::Stale;
        assert!(validate_snapshot(&state).is_ok());

        state.sync.status = SyncStatus::Offline;
        state.sync.last_successful_at = None;
        assert!(validate_snapshot(&state).is_ok());
    }

    #[test]
    fn historical_acknowledgement_preserves_a_current_day_success_status() {
        let now = test_time();
        let mut state = observed_state(now, 42);
        state.sync.last_successful_at = Some(format_time(now));
        state.sync.status = SyncStatus::Pending;

        update_sync_status_after_acknowledgement(&mut state, false, false, now);

        assert_eq!(state.sync.status, SyncStatus::Synced);
        assert_eq!(state.sync.last_successful_at, Some(format_time(now)));
    }

    #[test]
    fn historical_acknowledgement_does_not_invent_a_current_day_success() {
        let now = test_time();
        let mut state = observed_state(now, 42);
        state.sync.last_successful_at = None;
        state.sync.status = SyncStatus::Pending;

        update_sync_status_after_acknowledgement(&mut state, false, false, now);

        assert_eq!(state.sync.status, SyncStatus::Unavailable);
        assert_eq!(state.sync.last_successful_at, None);
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
        let codex = state.provider_mut(CodingProvider::Codex).unwrap();
        codex.top_model_usage = Some(TopModelUsage {
            model: Some("GPT 5.6 Sol".to_owned()),
            observed_tokens: 42,
        });
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
        claude.top_model_usage = Some(TopModelUsage {
            model: Some("Claude Sonnet 4.5".to_owned()),
            observed_tokens: 58,
        });
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
        let before_disable = core.menu_bar_headroom().unwrap();
        assert_eq!(
            before_disable.headroom,
            expected_headroom(
                62.0,
                crate::quota_headroom::HeadroomFreshness::Current,
                crate::quota_headroom::HeadroomCompleteness::Complete,
            )
        );

        policy.enabled.store(false, Ordering::Release);
        assert_eq!(
            core.menu_bar_headroom().unwrap(),
            before_disable,
            "live lifecycle changes must not alter an uncommitted native revision"
        );
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
        assert_eq!(
            cached_claude
                .top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("Claude Sonnet 4.5")
        );
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
        assert_eq!(claude.top_model_usage, None);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            ref api_equivalent_cost_basis,
            ..
        } = panel.combined_usage.today
        else {
            panic!("Codex usage must remain in Combined");
        };
        assert_eq!(observed_tokens, 42);
        assert_eq!(api_equivalent_cost_usd, Some(4.2));
        assert_eq!(api_equivalent_cost_basis.as_deref(), Some("openai-fixture"));
        assert_eq!(
            panel
                .top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("GPT 5.6 Sol")
        );
        assert_eq!(
            panel.combined_usage,
            panel.provider(CodingProvider::Codex).unwrap().usage
        );
        assert_eq!(
            core.menu_bar_headroom().unwrap(),
            crate::quota_headroom::RevisionedOverallQuotaHeadroom {
                revision: notice.revision.parse().unwrap(),
                headroom: expected_headroom(
                    74.0,
                    crate::quota_headroom::HeadroomFreshness::Current,
                    crate::quota_headroom::HeadroomCompleteness::Complete,
                ),
            }
        );
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
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        );
        let disabled = core.menu_bar_headroom().unwrap();
        assert_eq!(
            disabled.headroom,
            expected_headroom(
                74.0,
                crate::quota_headroom::HeadroomFreshness::Current,
                crate::quota_headroom::HeadroomCompleteness::Complete,
            )
        );

        policy.enabled.store(true, Ordering::Release);
        assert_eq!(
            core.menu_bar_headroom().unwrap(),
            disabled,
            "enablement must change only with a committed native revision"
        );
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        assert_eq!(
            core.menu_bar_headroom().unwrap().headroom,
            expected_headroom(
                74.0,
                crate::quota_headroom::HeadroomFreshness::Current,
                crate::quota_headroom::HeadroomCompleteness::Incomplete,
            ),
            "a missing enabled registry provider must make headroom incomplete"
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
        let mut indexing_usage = unavailable_periods();
        indexing_usage.scan_status = UsageScanStatus::Indexing;
        assert_eq!(claude.usage, indexing_usage);
    }

    #[test]
    fn refresh_without_a_completed_provider_keeps_the_first_observation_wait() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(false),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let source = Arc::new(ScriptedRefreshSource::new([Ok(None)]));
        let core = NativeCore::with_components(
            unavailable_state(1),
            ReadModelStore::Memory,
            clock,
            source.clone(),
            enablement,
        );

        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        wait_for_completed_runs(source.as_ref(), 1);

        assert_eq!(
            core.panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Indexing
        );
    }

    #[test]
    fn a_restart_does_not_restore_a_loading_state_without_an_active_refresh() {
        let database = TestDatabase::new();
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(false),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let core = NativeCore::open_without_launch_with_enablement(
            &database.0,
            Arc::clone(&clock),
            Arc::new(CachedProjectionRefreshAdapter),
            Arc::clone(&enablement),
        )
        .unwrap();

        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        assert_eq!(
            core.panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Indexing
        );
        assert_eq!(
            core.inner
                .projection
                .snapshot()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Unavailable
        );
        drop(core);

        let reopened = NativeCore::open_with_enablement(
            &database.0,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            enablement,
        )
        .unwrap();
        wait_for_idle(&reopened);

        assert_eq!(
            reopened
                .panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Unavailable
        );
    }

    #[test]
    fn reenabling_provider_restores_the_cached_provider_row_while_refresh_is_pending() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let mut state = observed_state(test_time(), 42);
        let mut claude_usage = state.provider(CodingProvider::Codex).unwrap().usage.clone();
        claude_usage.scan_status = UsageScanStatus::Complete;
        claude_usage.today_scan_status = UsageScanStatus::Complete;
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
        let expected_claude = {
            let claude = state.provider_mut(CodingProvider::Claude).unwrap();
            claude.presence = ProviderPresenceStatus::Detected;
            claude.quota = ProviderSnapshot::Current {
                provider: CodingProvider::Claude,
                observed_at: format_time(test_time() - TimeDuration::minutes(10)),
                quota_lanes: vec![
                    QuotaLane {
                        label: "Weekly limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(50.0),
                        reset_at: Some(format_time(test_time() + TimeDuration::hours(1))),
                    },
                    QuotaLane {
                        label: "Expired limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(10.0),
                        reset_at: Some(format_time(test_time() - TimeDuration::minutes(1))),
                    },
                ],
            };
            claude.usage = claude_usage;
            claude.transition_at(test_time(), test_time()).0
        };
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

        assert_eq!(claude, &expected_claude);
        assert_eq!(claude.usage.scan_status, UsageScanStatus::Complete);
        assert!(matches!(
            &claude.quota,
            ProviderSnapshot::Stale {
                provider: CodingProvider::Claude,
                quota_lanes,
                ..
            } if quota_lanes.len() == 2
                && quota_lanes[0].label == "Weekly limit"
                && quota_lanes[0].remaining == Some(50.0)
                && quota_lanes[1].label == "Expired limit"
                && quota_lanes[1].remaining == Some(10.0)
        ));
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
        assert_eq!(
            core.menu_bar_headroom().unwrap().headroom,
            expected_headroom(
                62.0,
                crate::quota_headroom::HeadroomFreshness::Stale,
                crate::quota_headroom::HeadroomCompleteness::Complete,
            )
        );
    }

    #[test]
    fn reenabling_provider_keeps_all_expired_claude_quota_lanes_stale() {
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let mut state = observed_state(test_time(), 42);
        let mut claude_usage = state.provider(CodingProvider::Codex).unwrap().usage.clone();
        claude_usage.scan_status = UsageScanStatus::Complete;
        claude_usage.today_scan_status = UsageScanStatus::Complete;
        let expected_claude = {
            let claude = state.provider_mut(CodingProvider::Claude).unwrap();
            claude.presence = ProviderPresenceStatus::Detected;
            claude.quota = ProviderSnapshot::Current {
                provider: CodingProvider::Claude,
                observed_at: format_time(test_time()),
                quota_lanes: vec![QuotaLane {
                    label: "Expired limit".to_owned(),
                    unit: "percent".to_owned(),
                    allowance: Some(100.0),
                    remaining: Some(10.0),
                    reset_at: Some(format_time(test_time() - TimeDuration::minutes(1))),
                }],
            };
            claude.usage = claude_usage;
            claude.transition_at(test_time(), test_time()).0
        };
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
        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();

        let panel = core.panel_state().unwrap();
        let claude = panel.provider(CodingProvider::Claude).unwrap();
        assert_eq!(claude, &expected_claude);
        assert!(matches!(
            &claude.quota,
            ProviderSnapshot::Stale {
                provider: CodingProvider::Claude,
                quota_lanes,
                ..
            } if quota_lanes.len() == 1
                && quota_lanes[0].label == "Expired limit"
                && quota_lanes[0].remaining == Some(10.0)
        ));
        assert!(matches!(claude.usage.today, UsageTotal::Current { .. }));
        assert_eq!(claude.usage.scan_status, UsageScanStatus::Complete);
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
        assert_eq!(
            core.menu_bar_headroom().unwrap(),
            crate::quota_headroom::RevisionedOverallQuotaHeadroom {
                revision: notice.revision.parse().unwrap(),
                headroom: expected_headroom(
                    74.0,
                    crate::quota_headroom::HeadroomFreshness::Current,
                    crate::quota_headroom::HeadroomCompleteness::Incomplete,
                ),
            }
        );
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
    fn disabled_provider_restores_stale_quota_before_reenable() {
        let database = TestDatabase::new();
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(true),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let mut observed = observed_state(test_time(), 42);
        let claude = observed.provider_mut(CodingProvider::Claude).unwrap();
        claude.presence = ProviderPresenceStatus::Detected;
        claude.quota = ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: format_time(test_time()),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(50.0),
                reset_at: Some(format_time(test_time() + TimeDuration::days(7))),
            }],
        };
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(observed))]));
        let core = NativeCore::open_without_launch_with_enablement(
            &database.0,
            Arc::clone(&clock),
            source.clone(),
            Arc::clone(&enablement),
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        wait_for_completed_runs(source.as_ref(), 1);
        core.wait_for_refresh_completion().unwrap();
        policy.enabled.store(false, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        drop(core);

        let reopened = NativeCore::open_without_launch_with_enablement(
            &database.0,
            clock,
            Arc::new(CachedProjectionRefreshAdapter),
            Arc::clone(&enablement),
        )
        .unwrap();
        assert!(matches!(
            &reopened
                .inner
                .projection
                .snapshot()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .quota,
            ProviderSnapshot::Stale { quota_lanes, .. }
                if quota_lanes.len() == 1 && quota_lanes[0].remaining == Some(50.0)
        ));

        policy.enabled.store(true, Ordering::Release);
        reopened
            .provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        assert!(matches!(
            &reopened
                .panel_state()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .quota,
            ProviderSnapshot::Stale { quota_lanes, .. }
                if quota_lanes.len() == 1 && quota_lanes[0].remaining == Some(50.0)
        ));
    }

    #[test]
    fn usage_sync_keeps_newer_revisions_and_stops_a_rejected_generation() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(test_time(), 42))),
            Ok(Some(observed_state(test_time(), 84))),
        ]));
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            source,
        )
        .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let first = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(first.snapshots()[0].revision, 1);
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Pending);

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let newer = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(newer.snapshots()[0].revision, 2);

        let late_acknowledgement = UsageSyncAcknowledgement {
            provider: first.snapshots()[0].provider,
            ranking_day: first.snapshots()[0].ranking_day.clone(),
            revision: first.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&first, &[late_acknowledgement])
            .unwrap();
        let after_late_ack = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(after_late_ack.snapshots()[0].revision, 2);
        let panel = core.panel_state().unwrap();
        assert_eq!(panel.sync.status, SyncStatus::Pending);
        assert!(panel.sync.last_successful_at.is_some());

        core.mark_usage_sync_offline().unwrap();
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Offline);
        assert!(core.pending_usage_sync_batch(1).unwrap().is_some());

        core.mark_usage_sync_authority_rejected(1).unwrap();
        assert_eq!(
            core.panel_state().unwrap().sync.status,
            SyncStatus::AuthorityRejected
        );
        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
    }

    #[test]
    fn transferred_generation_sends_only_both_provider_post_activation_segments() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let now = test_time();
        let database = TestDatabase::new();
        let initial = both_provider_observed_state(now, 100, 1.0, 200, 2.0);
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now)),
            Arc::new(ScriptedRefreshSource::new([Ok(Some(initial))])),
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();

        core.activate_usage_sync_generation(2).unwrap();

        let activation = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert!(activation.snapshots().is_empty());
        core.shutdown();
        drop(core);

        let later =
            both_provider_observed_state(now + TimeDuration::seconds(1), 150, 1.5, 275, 2.75);
        let latest =
            both_provider_observed_state(now + TimeDuration::seconds(2), 160, 1.6, 290, 2.9);
        let restored = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now + TimeDuration::seconds(2))),
            Arc::new(ScriptedRefreshSource::new([
                Ok(Some(later)),
                Ok(Some(latest)),
            ])),
        )
        .unwrap();
        restored.request_refresh(RefreshSource::Manual).unwrap();
        restored.wait_for_refresh_completion().unwrap();

        let pending = restored.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 2);
        let codex = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Codex)
            .unwrap();
        assert_eq!(codex.observed_tokens, 50);
        assert_eq!(codex.api_equivalent_cost.as_ref().unwrap().micros, 500_000);
        let claude = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Claude)
            .unwrap();
        assert_eq!(claude.observed_tokens, 75);
        assert_eq!(claude.api_equivalent_cost.as_ref().unwrap().micros, 750_000);

        let acknowledgements = pending
            .snapshots()
            .iter()
            .map(|snapshot| UsageSyncAcknowledgement {
                provider: snapshot.provider,
                ranking_day: snapshot.ranking_day.clone(),
                revision: snapshot.revision,
                outcome: AcknowledgementOutcome::Committed,
            })
            .collect::<Vec<_>>();
        restored
            .acknowledge_usage_sync(&pending, &acknowledgements)
            .unwrap();
        assert!(restored.pending_usage_sync_batch(2).unwrap().is_none());

        restored.request_refresh(RefreshSource::Manual).unwrap();
        restored.wait_for_refresh_completion().unwrap();
        let cumulative = restored.pending_usage_sync_batch(2).unwrap().unwrap();
        let codex = cumulative
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Codex)
            .unwrap();
        assert_eq!(codex.revision, 2);
        assert_eq!(codex.observed_tokens, 60);
        assert_eq!(codex.api_equivalent_cost.as_ref().unwrap().micros, 600_000);
        let claude = cumulative
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Claude)
            .unwrap();
        assert_eq!(claude.revision, 2);
        assert_eq!(claude.observed_tokens, 90);
        assert_eq!(claude.api_equivalent_cost.as_ref().unwrap().micros, 900_000);
    }

    #[test]
    fn transferred_generation_keeps_abandoned_same_day_usage_partial() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let now = test_time();
        let next_day = now + TimeDuration::days(1);
        let database = TestDatabase::new();
        let priced_state = |observed_at, observed_tokens, cost| {
            let mut state = observed_state(observed_at, observed_tokens);
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
            *api_equivalent_cost_usd = Some(cost);
            *api_equivalent_cost_basis = Some("openai-api-2026-08-09-v3".to_owned());
            *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::Reconciled);
            state.refresh_combined_usage();
            state
        };
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(priced_state(now, 100, 1.0))),
            Ok(Some(priced_state(now + TimeDuration::seconds(1), 150, 1.5))),
        ]));
        let clock = Arc::new(FixtureClock::new(now));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let abandoned = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(abandoned.snapshots()[0].observed_tokens, 100);
        assert_eq!(abandoned.snapshots()[0].coverage, SyncCoverage::Complete);

        core.activate_usage_sync_generation(2).unwrap();
        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        let activation_marker = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(activation_marker.snapshots().len(), 1);
        let marker = &activation_marker.snapshots()[0];
        assert_eq!(marker.provider, CodingProvider::Codex);
        assert_eq!(marker.ranking_day, now.date().to_string());
        assert_eq!(marker.revision, 1);
        assert_eq!(marker.evidence_basis, SyncEvidenceBasis::ProviderReported);
        assert_eq!(marker.coverage, SyncCoverage::Partial);
        assert_eq!(marker.observed_tokens, 0);
        assert_eq!(marker.api_equivalent_cost, None);
        assert_eq!(marker.correction_reason, None);
        assert_eq!(marker.correction_revision, None);

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let pending = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].provider, CodingProvider::Codex);
        assert_eq!(pending.snapshots()[0].ranking_day, now.date().to_string());
        assert_eq!(pending.snapshots()[0].revision, 2);
        assert_eq!(pending.snapshots()[0].observed_tokens, 50);
        assert_eq!(pending.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(pending.snapshots()[0].correction_reason, None);
        assert_eq!(pending.snapshots()[0].correction_revision, None);
        assert_eq!(
            pending.snapshots()[0]
                .api_equivalent_cost
                .as_ref()
                .unwrap()
                .micros,
            500_000
        );
        let expected_carryover = pending.snapshots()[0].clone();

        clock.advance(Duration::from_secs(24 * 60 * 60));
        let active_mac_activated_at =
            u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap();
        core.install_usage_sync_authority(2, active_mac_activated_at)
            .unwrap();
        let carryover_count = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(carryover_count, 1);
        core.shutdown();
        drop(core);

        let restored = NativeCore::open_without_launch(
            &database.0,
            clock,
            Arc::new(ScriptedRefreshSource::new([Ok(Some(observed_state(
                next_day, 25,
            )))])),
        )
        .unwrap();
        let carryover = restored.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(
            carryover.snapshots(),
            std::slice::from_ref(&expected_carryover)
        );
        assert!(
            carryover
                .mutation_args(
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    next_day,
                )
                .is_ok()
        );

        restored.request_refresh(RefreshSource::Manual).unwrap();
        restored.wait_for_refresh_completion().unwrap();
        assert_eq!(
            restored
                .pending_usage_sync_batch(2)
                .unwrap()
                .unwrap()
                .snapshots(),
            std::slice::from_ref(&expected_carryover)
        );
        restored
            .acknowledge_usage_sync(
                &carryover,
                &[UsageSyncAcknowledgement {
                    provider: expected_carryover.provider,
                    ranking_day: expected_carryover.ranking_day.clone(),
                    revision: expected_carryover.revision,
                    outcome: AcknowledgementOutcome::Committed,
                }],
            )
            .unwrap();

        restored
            .install_usage_sync_authority(2, active_mac_activated_at)
            .unwrap();
        let stored_carryovers = match &*restored.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(stored_carryovers, 0);
        let current = restored.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(current.snapshots().len(), 1);
        assert_eq!(
            current.snapshots()[0].ranking_day,
            next_day.date().to_string()
        );
        assert_eq!(current.snapshots()[0].observed_tokens, 25);
    }

    #[test]
    fn profile_recovery_atomically_replaces_a_same_generation_ledger() {
        let now = test_time();
        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([]));
        let clock = Arc::new(FixtureClock::new(now));
        let core = NativeCore::open_without_launch(&database.0, clock, source).unwrap();
        let previous_profile = SanitizedProfileOutcome::Ready {
            display_name: "Previous".to_owned(),
            touch_grass_id: "TG-ABC234".to_owned(),
        };
        core.set_profile_outcome(previous_profile).unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        let old_conflicts = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => {
                store
                    .connection
                    .execute(
                        "INSERT INTO usage_sync_terminal_conflicts(
                             active_generation, provider, ranking_day, revision
                         ) VALUES(2, 'codex', '2026-04-05', 99)",
                        [],
                    )
                    .unwrap();
                store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM usage_sync_terminal_conflicts",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap()
            }
            ReadModelStore::Memory => panic!("Profile recovery fixture requires SQLite"),
        };
        assert_eq!(old_conflicts, 1);

        let recovered_profile = SanitizedProfileOutcome::Ready {
            display_name: "Recovered".to_owned(),
            touch_grass_id: "TG-XYZ234".to_owned(),
        };
        let activated_at = u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap();
        core.recover_profile_authority(recovered_profile.clone(), 2, activated_at)
            .unwrap();

        assert_eq!(
            core.inner.projection.snapshot().unwrap().profile,
            recovered_profile
        );
        let remaining_conflicts = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_terminal_conflicts",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("Profile recovery fixture requires SQLite"),
        };
        assert_eq!(remaining_conflicts, 0);
        drop(core);

        let restored = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now)),
            Arc::new(ScriptedRefreshSource::new([])),
        )
        .unwrap();
        assert_eq!(
            restored.inner.projection.snapshot().unwrap().profile,
            recovered_profile
        );
        let restored_conflicts = match &*restored.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_terminal_conflicts",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("Profile recovery fixture requires SQLite"),
        };
        assert_eq!(restored_conflicts, 0);
    }

    #[test]
    fn stale_authority_rejection_does_not_block_recovered_same_generation() {
        let now = test_time();
        let database = TestDatabase::new();
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now)),
            Arc::new(ScriptedRefreshSource::new([])),
        )
        .unwrap();
        core.set_profile_outcome(SanitizedProfileOutcome::Ready {
            display_name: "Previous".to_owned(),
            touch_grass_id: "TG-ABC234".to_owned(),
        })
        .unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        let previous_authority = core.usage_sync_authority_identity().unwrap();

        let activated_at = u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap();
        core.recover_profile_authority(
            SanitizedProfileOutcome::Ready {
                display_name: "Recovered".to_owned(),
                touch_grass_id: "TG-XYZ234".to_owned(),
            },
            2,
            activated_at,
        )
        .unwrap();

        core.reject_usage_sync_authority_if_current(&previous_authority)
            .unwrap();

        assert_eq!(core.active_usage_sync_generation().unwrap(), Some(2));
        assert_ne!(
            core.panel_state().unwrap().sync.status,
            SyncStatus::AuthorityRejected
        );
        assert!(core.pending_usage_sync_batch(2).unwrap().is_some());
    }

    #[test]
    fn profile_recovery_requires_durable_local_persistence() {
        let now = test_time();
        let database = TestDatabase::new();
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now)),
            Arc::new(ScriptedRefreshSource::new([])),
        )
        .unwrap();
        let previous_profile = SanitizedProfileOutcome::Ready {
            display_name: "Previous".to_owned(),
            touch_grass_id: "TG-ABC234".to_owned(),
        };
        core.set_profile_outcome(previous_profile.clone()).unwrap();
        *core.inner.store.lock().unwrap() = ReadModelStore::Memory;

        let activated_at = u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap();
        assert_eq!(
            core.recover_profile_authority(
                SanitizedProfileOutcome::Ready {
                    display_name: "Recovered".to_owned(),
                    touch_grass_id: "TG-XYZ234".to_owned(),
                },
                2,
                activated_at,
            ),
            Err("native state persistence unavailable")
        );
        assert_eq!(
            core.inner.projection.snapshot().unwrap().profile,
            previous_profile
        );
    }

    #[test]
    fn transferred_generation_queues_full_totals_after_the_activation_day() {
        let now = test_time();
        let next_day = now + TimeDuration::days(1);
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(now));
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(both_provider_observed_state(now, 100, 1.0, 200, 2.0))),
            Ok(Some(both_provider_observed_state(
                next_day, 25, 0.25, 40, 0.4,
            ))),
        ]));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        assert!(
            core.pending_usage_sync_batch(2)
                .unwrap()
                .unwrap()
                .snapshots()
                .is_empty()
        );

        clock.advance(Duration::from_secs(24 * 60 * 60));
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();

        let pending = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 2);
        let codex = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Codex)
            .unwrap();
        assert_eq!(codex.observed_tokens, 25);
        assert_eq!(codex.api_equivalent_cost.as_ref().unwrap().micros, 250_000);
        let claude = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Claude)
            .unwrap();
        assert_eq!(claude.observed_tokens, 40);
        assert_eq!(claude.api_equivalent_cost.as_ref().unwrap().micros, 400_000);
    }

    #[test]
    fn delayed_install_after_rollover_does_not_relabel_current_usage() {
        let activation_time = test_time();
        let worker_time = activation_time + TimeDuration::days(1);
        let active_mac_activated_at =
            u64::try_from(activation_time.unix_timestamp_nanos() / 1_000_000).unwrap();
        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(
            both_provider_observed_state(worker_time, 25, 0.25, 40, 0.4),
        ))]));
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(worker_time)),
            source,
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();

        core.install_usage_sync_authority(2, active_mac_activated_at)
            .unwrap();

        let baseline_count = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_generation_baselines",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(baseline_count, 0);
        let pending = core.pending_usage_sync_batch(2).unwrap().unwrap();
        let codex = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Codex)
            .unwrap();
        assert_eq!(codex.ranking_day, worker_time.date().to_string());
        assert_eq!(codex.observed_tokens, 25);
        let claude = pending
            .snapshots()
            .iter()
            .find(|snapshot| snapshot.provider == CodingProvider::Claude)
            .unwrap();
        assert_eq!(claude.ranking_day, worker_time.date().to_string());
        assert_eq!(claude.observed_tokens, 40);
    }

    #[test]
    fn delayed_install_after_rollover_sends_the_transfer_day_partial_marker_first() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let ranking_day_start = test_time();
        let first_observation =
            ranking_day_start + TimeDuration::hours(23) + TimeDuration::minutes(58);
        let activation_time = first_observation + TimeDuration::minutes(1);
        let install_time = activation_time + TimeDuration::minutes(2);
        let active_mac_activated_at =
            u64::try_from(activation_time.unix_timestamp_nanos() / 1_000_000).unwrap();
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(first_observation));
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(first_observation, 100))),
            Ok(Some(observed_state(install_time, 25))),
        ]));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let abandoned = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(abandoned.snapshots()[0].observed_tokens, 100);
        assert_eq!(abandoned.snapshots()[0].coverage, SyncCoverage::Complete);

        clock.advance(Duration::from_secs(3 * 60));
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.install_usage_sync_authority(2, active_mac_activated_at)
            .unwrap();

        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        let carryover = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(carryover.snapshots().len(), 1);
        let marker = &carryover.snapshots()[0];
        assert_eq!(marker.ranking_day, activation_time.date().to_string());
        assert_eq!(marker.observed_at, active_mac_activated_at);
        assert_eq!(marker.observed_tokens, 0);
        assert_eq!(marker.coverage, SyncCoverage::Partial);
        assert_eq!(marker.api_equivalent_cost, None);
        assert_eq!(marker.correction_reason, None);
        assert_eq!(marker.correction_revision, None);
        let stored_carryovers = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(stored_carryovers, 1);
        assert!(
            carryover
                .mutation_args(
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    install_time,
                )
                .is_ok()
        );

        core.acknowledge_usage_sync(
            &carryover,
            &[UsageSyncAcknowledgement {
                provider: marker.provider,
                ranking_day: marker.ranking_day.clone(),
                revision: marker.revision,
                outcome: AcknowledgementOutcome::Committed,
            }],
        )
        .unwrap();

        let stored_carryovers = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(stored_carryovers, 0);

        let current = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(current.snapshots().len(), 1);
        assert_eq!(
            current.snapshots()[0].ranking_day,
            install_time.date().to_string()
        );
        assert_eq!(current.snapshots()[0].observed_tokens, 25);
        assert_eq!(current.snapshots()[0].coverage, SyncCoverage::Complete);
    }

    #[test]
    fn transferred_generation_ignores_a_stale_install_total() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let now = test_time();
        let activation_time = now + TimeDuration::minutes(1);
        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(now, 100))),
            Ok(Some(observed_state(
                activation_time + TimeDuration::seconds(1),
                200,
            ))),
            Ok(Some(observed_state(
                activation_time + TimeDuration::seconds(2),
                230,
            ))),
        ]));
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(activation_time)),
            source,
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        let stored_counts = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => (
                store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM usage_sync_generation_baselines",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                store
                    .connection
                    .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
            ),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(stored_counts, (0, 0));
        let activation = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert!(activation.snapshots().is_empty());
        core.acknowledge_usage_sync(&activation, &[]).unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let marker = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(marker.snapshots().len(), 1);
        assert_eq!(marker.snapshots()[0].revision, 1);
        assert_eq!(marker.snapshots()[0].observed_tokens, 0);
        assert_eq!(marker.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(marker.snapshots()[0].api_equivalent_cost, None);
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Codex,
            ranking_day: marker.snapshots()[0].ranking_day.clone(),
            revision: marker.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&marker, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();

        let pending = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].revision, 2);
        assert_eq!(pending.snapshots()[0].observed_tokens, 30);
        assert_eq!(pending.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(pending.snapshots()[0].api_equivalent_cost, None);
    }

    #[test]
    fn transferred_generation_keeps_a_missing_install_baseline_partial() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let activation_time = test_time();
        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(
                activation_time + TimeDuration::seconds(1),
                150,
            ))),
            Ok(Some(observed_state(
                activation_time + TimeDuration::seconds(2),
                180,
            ))),
        ]));
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(activation_time)),
            source,
        )
        .unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        let activation = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert!(activation.snapshots().is_empty());
        core.acknowledge_usage_sync(&activation, &[]).unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let marker = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(marker.snapshots().len(), 1);
        assert_eq!(marker.snapshots()[0].observed_tokens, 0);
        assert_eq!(marker.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(marker.snapshots()[0].api_equivalent_cost, None);
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Codex,
            ranking_day: marker.snapshots()[0].ranking_day.clone(),
            revision: marker.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&marker, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let pending = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].revision, 2);
        assert_eq!(pending.snapshots()[0].observed_tokens, 30);
        assert_eq!(pending.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(pending.snapshots()[0].api_equivalent_cost, None);
    }

    #[test]
    fn post_activation_zero_baseline_carries_over_after_rollover() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let activation_time = test_time();
        let next_day = activation_time + TimeDuration::days(1);
        let active_mac_activated_at =
            u64::try_from(activation_time.unix_timestamp_nanos() / 1_000_000).unwrap();
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(activation_time));
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(
                activation_time + TimeDuration::seconds(1),
                150,
            ))),
            Ok(Some(observed_state(next_day, 25))),
        ]));
        let core = NativeCore::open_without_launch(&database.0, clock.clone(), source).unwrap();
        core.activate_usage_sync_generation(2).unwrap();
        let activation = core.pending_usage_sync_batch(2).unwrap().unwrap();
        core.acknowledge_usage_sync(&activation, &[]).unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let expected = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(expected.snapshots().len(), 1);
        assert_eq!(expected.snapshots()[0].revision, 1);
        assert_eq!(expected.snapshots()[0].observed_tokens, 0);
        assert_eq!(expected.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(expected.snapshots()[0].api_equivalent_cost, None);
        assert!(expected.snapshots()[0].observed_at > active_mac_activated_at);

        clock.advance(Duration::from_secs(24 * 60 * 60));
        core.install_usage_sync_authority(2, active_mac_activated_at)
            .unwrap();
        let carryover_kind = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT carryover_kind FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("transfer fixture requires SQLite"),
        };
        assert_eq!(carryover_kind, "pending-segment");
        let carryover = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(carryover.snapshots(), expected.snapshots());
        assert!(
            carryover
                .mutation_args(
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    next_day,
                )
                .is_ok()
        );

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let snapshot = &carryover.snapshots()[0];
        core.acknowledge_usage_sync(
            &carryover,
            &[UsageSyncAcknowledgement {
                provider: snapshot.provider,
                ranking_day: snapshot.ranking_day.clone(),
                revision: snapshot.revision,
                outcome: AcknowledgementOutcome::Committed,
            }],
        )
        .unwrap();
        let current = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(current.snapshots().len(), 1);
        assert_eq!(
            current.snapshots()[0].ranking_day,
            next_day.date().to_string()
        );
        assert_eq!(current.snapshots()[0].observed_tokens, 25);
    }

    #[test]
    fn transferred_generation_keeps_its_baseline_after_a_parser_correction() {
        use crate::usage_sync::{
            AcknowledgementOutcome, CorrectionReason, UsageSyncAcknowledgement,
        };

        let now = test_time();
        let database = TestDatabase::new();
        let source = Arc::new(CorrectionRefreshSource {
            responses: Mutex::new(VecDeque::from([
                SnapshotRefreshOutcome::from(Some(claude_observed_state(now, 100))),
                SnapshotRefreshOutcome::from(Some(claude_observed_state(
                    now + TimeDuration::seconds(1),
                    150,
                ))),
                SnapshotRefreshOutcome {
                    snapshot: Some(claude_observed_state(now + TimeDuration::seconds(2), 90)),
                    completed_providers: BTreeSet::new(),
                    corrections: BTreeMap::from([(
                        CodingProvider::Claude,
                        ProviderCorrection::ParserCorrection { source_revision: 2 },
                    )]),
                },
                SnapshotRefreshOutcome::from(Some(claude_observed_state(
                    now + TimeDuration::seconds(3),
                    110,
                ))),
            ])),
        });
        let core =
            NativeCore::open_without_launch(&database.0, Arc::new(FixtureClock::new(now)), source)
                .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(2).unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let first = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(first.snapshots()[0].observed_tokens, 50);
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Claude,
            ranking_day: first.snapshots()[0].ranking_day.clone(),
            revision: 1,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&first, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let corrected = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(corrected.snapshots()[0].revision, 2);
        assert_eq!(corrected.snapshots()[0].observed_tokens, 0);
        assert_eq!(
            corrected.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Claude,
            ranking_day: corrected.snapshots()[0].ranking_day.clone(),
            revision: 2,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&corrected, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let later = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(later.snapshots()[0].revision, 3);
        assert_eq!(later.snapshots()[0].observed_tokens, 10);
        assert_eq!(later.snapshots()[0].correction_reason, None);
    }

    #[test]
    fn transferred_generation_keeps_its_baseline_after_a_provider_replacement() {
        use crate::usage_sync::{
            AcknowledgementOutcome, CorrectionReason, UsageSyncAcknowledgement,
        };

        let now = test_time();
        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(claude_observed_state(now, 100))),
            Ok(Some(claude_observed_state(
                now + TimeDuration::seconds(1),
                150,
            ))),
            Ok(Some(claude_provider_reported_state(
                now + TimeDuration::seconds(2),
                90,
            ))),
            Ok(Some(claude_provider_reported_state(
                now + TimeDuration::seconds(3),
                110,
            ))),
        ]));
        let core =
            NativeCore::open_without_launch(&database.0, Arc::new(FixtureClock::new(now)), source)
                .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(2).unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let first = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(first.snapshots()[0].observed_tokens, 50);
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Claude,
            ranking_day: first.snapshots()[0].ranking_day.clone(),
            revision: 1,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&first, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let corrected = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(corrected.snapshots()[0].revision, 2);
        assert_eq!(corrected.snapshots()[0].observed_tokens, 0);
        assert_eq!(
            corrected.snapshots()[0].correction_reason,
            Some(CorrectionReason::ProviderReplacement)
        );
        let acknowledgement = UsageSyncAcknowledgement {
            provider: CodingProvider::Claude,
            ranking_day: corrected.snapshots()[0].ranking_day.clone(),
            revision: 2,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&corrected, &[acknowledgement])
            .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        let later = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert_eq!(later.snapshots()[0].revision, 3);
        assert_eq!(later.snapshots()[0].observed_tokens, 10);
        assert_eq!(later.snapshots()[0].correction_reason, None);
    }

    #[test]
    fn profile_backfill_is_stable_across_provider_toggles() {
        let database = TestDatabase::new();
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let policy = Arc::new(ClaudeTogglePolicy {
            enabled: AtomicBool::new(true),
        });
        let enablement: Arc<dyn ProviderEnablementPolicy> = policy.clone();
        let mut state = observed_state(test_time(), 42);
        let mut claude_usage = state
            .provider(CodingProvider::Codex)
            .expect("Codex fixture")
            .usage
            .clone();
        let UsageTotal::Current { evidence_basis, .. } = &mut claude_usage.today else {
            panic!("fixture usage must be current");
        };
        *evidence_basis = UsageEvidenceBasis::LocallyDerived;
        let claude = state
            .provider_mut(CodingProvider::Claude)
            .expect("Claude fixture");
        claude.presence = ProviderPresenceStatus::Detected;
        claude.usage = claude_usage;
        state.refresh_combined_usage();
        let source = Arc::new(ScriptedRefreshSource::new([Ok(Some(state))]));
        let core =
            NativeCore::open_without_launch_with_enablement(&database.0, clock, source, enablement)
                .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        policy.enabled.store(false, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        core.activate_usage_sync_generation(1).unwrap();

        let before_activation = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let initial_profile_snapshots = before_activation.snapshots().to_vec();
        assert_eq!(
            before_activation
                .provider_settings()
                .unwrap()
                .enabled_providers(),
            &[CodingProvider::Codex]
        );
        assert_eq!(
            before_activation
                .snapshots()
                .iter()
                .map(|snapshot| snapshot.provider)
                .collect::<Vec<_>>(),
            vec![CodingProvider::Claude, CodingProvider::Codex]
        );
        assert!(matches!(
            core.inner
                .projection
                .snapshot()
                .unwrap()
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current { .. }
        ));

        policy.enabled.store(true, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, true)
            .unwrap();
        let enabled = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(enabled.snapshots().len(), 2);
        assert_eq!(enabled.snapshots(), initial_profile_snapshots);
        assert_eq!(enabled.provider_settings().unwrap().revision(), 2);
        assert_eq!(
            enabled.provider_settings().unwrap().enabled_providers(),
            &[CodingProvider::Codex, CodingProvider::Claude]
        );

        policy.enabled.store(false, Ordering::Release);
        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        let before_delivery = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(before_delivery.provider_settings().unwrap().revision(), 3);
        assert_eq!(before_delivery.snapshots(), initial_profile_snapshots);
        assert_eq!(
            before_delivery
                .provider_settings()
                .unwrap()
                .enabled_providers(),
            &[CodingProvider::Codex]
        );
        assert_eq!(
            before_delivery
                .snapshots()
                .iter()
                .map(|snapshot| snapshot.provider)
                .collect::<Vec<_>>(),
            vec![CodingProvider::Claude, CodingProvider::Codex]
        );
    }

    #[test]
    fn stale_provider_setting_acknowledgement_requeues_before_usage_delivery() {
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
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed_requests = Arc::clone(&requests);
        core.install_usage_sync_request(Arc::new(move || {
            observed_requests.fetch_add(1, Ordering::AcqRel);
        }))
        .unwrap();

        core.finish_usage_sync_attempt(
            &sent,
            UsageSyncAttemptResult::Committed(UsageSyncAcknowledgements {
                provider_settings: Some(ProviderSettingsAcknowledgement {
                    revision: 3,
                    outcome: crate::usage_sync::AcknowledgementOutcome::Stale,
                }),
                usage: Vec::new(),
                usage_mutation_completed: false,
            }),
        )
        .unwrap();

        let pending = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(pending.provider_settings().unwrap().revision(), 4);
        assert_eq!(
            pending.snapshots()[0].revision,
            sent.snapshots()[0].revision
        );
        assert_eq!(requests.load(Ordering::Acquire), 1);
    }

    #[test]
    fn empty_profile_backfill_waits_when_stale_settings_skip_daily_usage() {
        let database = TestDatabase::new();
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert!(sent.is_empty_profile_backfill());
        let settings_revision = sent.provider_settings().unwrap().revision();

        core.finish_usage_sync_attempt(
            &sent,
            UsageSyncAttemptResult::Committed(UsageSyncAcknowledgements {
                provider_settings: Some(ProviderSettingsAcknowledgement {
                    revision: settings_revision + 2,
                    outcome: crate::usage_sync::AcknowledgementOutcome::Stale,
                }),
                usage: Vec::new(),
                usage_mutation_completed: false,
            }),
        )
        .unwrap();

        let retry = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert!(retry.is_empty_profile_backfill());
        assert!(retry.requires_usage_mutation());
        core.finish_usage_sync_attempt(
            &retry,
            UsageSyncAttemptResult::Committed(UsageSyncAcknowledgements {
                provider_settings: retry.provider_settings().map(|settings| {
                    ProviderSettingsAcknowledgement {
                        revision: settings.revision(),
                        outcome: crate::usage_sync::AcknowledgementOutcome::Committed,
                    }
                }),
                usage: Vec::new(),
                usage_mutation_completed: true,
            }),
        )
        .unwrap();

        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        core.shutdown();
        drop(core);

        let restored = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        assert_eq!(restored.active_usage_sync_generation().unwrap(), Some(1));
        restored.activate_usage_sync_generation(1).unwrap();
        assert!(restored.pending_usage_sync_batch(1).unwrap().is_none());
    }

    #[test]
    fn authority_rejection_is_visible_before_a_generation_is_activated() {
        let database = TestDatabase::new();
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        let notices = core.revision_notices().unwrap();

        core.reject_active_usage_sync_authority().unwrap();

        notices
            .recv_timeout(Duration::from_secs(1))
            .expect("authority transition notice");
        assert!(core.active_usage_sync_generation().unwrap().is_none());
        assert_eq!(
            core.panel_state().unwrap().sync.status,
            SyncStatus::AuthorityRejected
        );
        let rejected_revision = core.panel_state().unwrap().revision;

        core.reject_active_usage_sync_authority().unwrap();

        assert_eq!(core.panel_state().unwrap().revision, rejected_revision);
        assert!(notices.try_recv().is_err());

        core.activate_usage_sync_generation(1).unwrap();

        let pending = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert!(pending.snapshots().is_empty());
        assert_eq!(
            pending.provider_settings().unwrap().enabled_providers(),
            &[CodingProvider::Codex, CodingProvider::Claude]
        );
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Pending);
    }

    #[test]
    fn stale_acknowledgement_requests_the_requeued_snapshot_immediately() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

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
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed_requests = Arc::clone(&requests);
        core.install_usage_sync_request(Arc::new(move || {
            observed_requests.fetch_add(1, Ordering::AcqRel);
        }))
        .unwrap();
        let acknowledgement = UsageSyncAcknowledgement {
            provider: sent.snapshots()[0].provider,
            ranking_day: sent.snapshots()[0].ranking_day.clone(),
            revision: 3,
            outcome: AcknowledgementOutcome::Stale,
        };

        core.acknowledge_usage_sync(&sent, &[acknowledgement])
            .unwrap();

        let pending = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(pending.snapshots()[0].revision, 4);
        assert_eq!(requests.load(Ordering::Acquire), 1);
    }

    #[test]
    fn lower_revision_conflict_stops_without_an_immediate_retry() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

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
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed_requests = Arc::clone(&requests);
        core.install_usage_sync_request(Arc::new(move || {
            observed_requests.fetch_add(1, Ordering::AcqRel);
        }))
        .unwrap();
        let acknowledgement = UsageSyncAcknowledgement {
            provider: sent.snapshots()[0].provider,
            ranking_day: sent.snapshots()[0].ranking_day.clone(),
            revision: sent.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Conflict,
        };

        core.acknowledge_usage_sync(&sent, &[acknowledgement])
            .unwrap();

        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        assert_eq!(requests.load(Ordering::Acquire), 0);
        assert_eq!(
            core.panel_state().unwrap().sync.status,
            SyncStatus::Unavailable
        );

        drop(core);
        let connection = Connection::open(&database.0).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_terminal_conflicts",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(connection);

        let relaunched = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(ScriptedRefreshSource::new([])),
        )
        .unwrap();
        assert!(relaunched.pending_usage_sync_batch(1).unwrap().is_none());
        assert_eq!(
            relaunched.panel_state().unwrap().sync.status,
            SyncStatus::Unavailable
        );

        relaunched
            .provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();
        let settings_only = relaunched.pending_usage_sync_batch(1).unwrap().unwrap();
        assert!(settings_only.snapshots().is_empty());
        relaunched
            .acknowledge_usage_sync(&settings_only, &[])
            .unwrap();
        assert_eq!(
            relaunched.panel_state().unwrap().sync.status,
            SyncStatus::Unavailable
        );
    }

    #[test]
    fn acknowledgement_keeps_an_unsupported_local_decrease_stale() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let database = TestDatabase::new();
        let source = Arc::new(ScriptedRefreshSource::new([
            Ok(Some(observed_state(test_time(), 100))),
            Ok(Some(observed_state(
                test_time() + TimeDuration::seconds(1),
                80,
            ))),
        ]));
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time() + TimeDuration::seconds(2))),
            source,
        )
        .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Pending);

        let acknowledgement = UsageSyncAcknowledgement {
            provider: sent.snapshots()[0].provider,
            ranking_day: sent.snapshots()[0].ranking_day.clone(),
            revision: sent.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&sent, &[acknowledgement])
            .unwrap();

        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Stale);
    }

    #[test]
    fn provider_parser_correction_reaches_the_atomic_snapshot_outbox_commit() {
        use crate::usage_sync::CorrectionReason;

        let now = test_time();
        let database = TestDatabase::new();
        let source = Arc::new(CorrectionRefreshSource {
            responses: Mutex::new(VecDeque::from([
                SnapshotRefreshOutcome::from(Some(claude_observed_state(now, 100))),
                SnapshotRefreshOutcome {
                    snapshot: Some(claude_observed_state(now + TimeDuration::seconds(1), 80)),
                    completed_providers: BTreeSet::new(),
                    corrections: BTreeMap::from([(
                        CodingProvider::Claude,
                        ProviderCorrection::ParserCorrection { source_revision: 2 },
                    )]),
                },
            ])),
        });
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now + TimeDuration::seconds(2))),
            source,
        )
        .unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();

        let pending = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].revision, 2);
        assert_eq!(pending.snapshots()[0].observed_tokens, 80);
        assert_eq!(
            pending.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
    }

    #[test]
    fn correction_survives_identity_pending_later_usage_and_restart() {
        use crate::usage_sync::CorrectionReason;

        let now = test_time();
        let database = TestDatabase::new();
        let correction = BTreeMap::from([(
            CodingProvider::Claude,
            ProviderCorrection::ParserCorrection { source_revision: 2 },
        )]);
        let source = Arc::new(CorrectionRefreshSource {
            responses: Mutex::new(VecDeque::from([
                SnapshotRefreshOutcome {
                    snapshot: Some(claude_observed_state(now, 40)),
                    completed_providers: BTreeSet::new(),
                    corrections: correction.clone(),
                },
                SnapshotRefreshOutcome {
                    snapshot: Some(claude_observed_state(now + TimeDuration::seconds(1), 50)),
                    completed_providers: BTreeSet::new(),
                    corrections: correction,
                },
            ])),
        });
        let core = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now + TimeDuration::seconds(2))),
            source,
        )
        .unwrap();
        for _ in 0..2 {
            core.request_refresh(RefreshSource::Manual).unwrap();
            core.wait_for_refresh_completion().unwrap();
        }
        assert!(core.active_usage_sync_generation().unwrap().is_none());
        core.shutdown();
        drop(core);

        let restored = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(now + TimeDuration::seconds(2))),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        restored.activate_usage_sync_generation(1).unwrap();

        let pending = restored.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(pending.snapshots()[0].observed_tokens, 50);
        assert_eq!(
            pending.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
    }

    #[test]
    fn pending_usage_revision_survives_a_native_restart() {
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
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        assert_eq!(
            core.pending_usage_sync_batch(1)
                .unwrap()
                .unwrap()
                .snapshots()[0]
                .revision,
            1
        );
        core.shutdown();
        drop(core);

        let restored = NativeCore::open_without_launch(
            &database.0,
            Arc::new(FixtureClock::new(test_time())),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        assert_eq!(restored.active_usage_sync_generation().unwrap(), Some(1));
        restored.activate_usage_sync_generation(1).unwrap();
        let pending = restored.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(pending.snapshots()[0].revision, 1);
        assert_eq!(
            restored.panel_state().unwrap().sync.status,
            SyncStatus::Pending
        );
        let notices = restored.revision_notices().unwrap();
        restored.mark_usage_sync_authority_rejected(1).unwrap();
        notices
            .recv_timeout(Duration::from_secs(1))
            .expect("authority transition notice");
        assert_eq!(
            restored.panel_state().unwrap().sync.status,
            SyncStatus::AuthorityRejected
        );
        assert!(restored.pending_usage_sync_batch(1).unwrap().is_none());
        let rejected_revision = restored.panel_state().unwrap().revision;
        restored.mark_usage_sync_authority_rejected(1).unwrap();
        assert_eq!(restored.panel_state().unwrap().revision, rejected_revision);
        assert!(notices.try_recv().is_err());
    }

    #[test]
    fn utc_rollover_keeps_persistence_when_today_cache_is_from_yesterday() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let core = NativeCore::open_without_launch(
            &database.0,
            clock.clone(),
            Arc::new(ScriptedRefreshSource::new([Ok(Some(observed_state(
                test_time(),
                42,
            )))])),
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let acknowledgement = UsageSyncAcknowledgement {
            provider: sent.snapshots()[0].provider,
            ranking_day: sent.snapshots()[0].ranking_day.clone(),
            revision: sent.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&sent, &[acknowledgement])
            .unwrap();
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Synced);

        clock.advance(Duration::from_secs(24 * 60 * 60));
        core.activate_usage_sync_generation(2).unwrap();

        assert!(matches!(
            &*core.inner.store.lock().unwrap(),
            ReadModelStore::Persistent(_)
        ));
        let settings = core.pending_usage_sync_batch(2).unwrap().unwrap();
        assert!(settings.snapshots().is_empty());
        core.acknowledge_usage_sync(&settings, &[]).unwrap();
        assert!(core.pending_usage_sync_batch(2).unwrap().is_none());
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Stale);
    }

    #[test]
    fn acknowledgement_after_utc_midnight_does_not_sync_the_new_day() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let before_midnight = OffsetDateTime::parse("2026-08-08T23:59:59Z", &Rfc3339).unwrap();
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(before_midnight));
        let core = NativeCore::open_without_launch(
            &database.0,
            clock.clone(),
            Arc::new(ScriptedRefreshSource::new([Ok(Some(observed_state(
                before_midnight,
                42,
            )))])),
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let sent = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let acknowledgement = UsageSyncAcknowledgement {
            provider: sent.snapshots()[0].provider,
            ranking_day: sent.snapshots()[0].ranking_day.clone(),
            revision: sent.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };

        clock.advance(Duration::from_secs(2));
        core.acknowledge_usage_sync(&sent, &[acknowledgement])
            .unwrap();

        let panel = core.panel_state().unwrap();
        assert_eq!(panel.sync.status, SyncStatus::Unavailable);
        assert_eq!(panel.sync.last_successful_at, None);
        assert!(core.pending_usage_sync_batch(1).unwrap().is_none());
        assert!(matches!(
            &*core.inner.store.lock().unwrap(),
            ReadModelStore::Persistent(_)
        ));
    }

    #[test]
    fn deferred_profile_correction_remains_pending_after_the_new_day_starts() {
        use crate::usage_sync::{AcknowledgementOutcome, UsageSyncAcknowledgement};

        let before_midnight = OffsetDateTime::parse("2026-08-08T23:59:59Z", &Rfc3339).unwrap();
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(before_midnight));
        let core = NativeCore::open_without_launch(
            &database.0,
            clock.clone(),
            Arc::new(ScriptedRefreshSource::new([
                Ok(Some(observed_state(before_midnight, 42))),
                Ok(Some(observed_state(before_midnight, 84))),
            ])),
        )
        .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        core.activate_usage_sync_generation(1).unwrap();
        let initial = core.pending_usage_sync_batch(1).unwrap().unwrap();
        let acknowledgement = UsageSyncAcknowledgement {
            provider: initial.snapshots()[0].provider,
            ranking_day: initial.snapshots()[0].ranking_day.clone(),
            revision: initial.snapshots()[0].revision,
            outcome: AcknowledgementOutcome::Committed,
        };
        core.acknowledge_usage_sync(&initial, &[acknowledgement])
            .unwrap();
        core.request_refresh(RefreshSource::Manual).unwrap();
        core.wait_for_refresh_completion().unwrap();
        assert!(core.pending_usage_sync_batch(1).unwrap().is_some());

        clock.advance(Duration::from_secs(2));
        core.mark_usage_sync_pending().unwrap();

        let correction = core.pending_usage_sync_batch(1).unwrap().unwrap();
        assert_eq!(correction.snapshots().len(), 1);
        assert_eq!(correction.snapshots()[0].ranking_day, "2026-08-08");
        assert_eq!(correction.snapshots()[0].revision, 2);
        assert!(
            correction
                .mutation_args(
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    clock.now(),
                )
                .is_ok()
        );
        assert_eq!(core.panel_state().unwrap().sync.status, SyncStatus::Pending);
        let old_rows: i64 = match &*core.inner.store.lock().unwrap() {
            ReadModelStore::Persistent(store) => store
                .connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_latest_outbox WHERE queue_state = 'active'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
            ReadModelStore::Memory => panic!("persistence must remain available"),
        };
        assert_eq!(old_rows, 1);
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
    fn completed_provider_commits_before_peer_refresh_finishes() {
        struct ReleaseOnDrop(Option<Sender<()>>);

        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                if let Some(release) = self.0.take() {
                    let _ = release.send(());
                }
            }
        }

        let database = TestDatabase::new();
        let clock: Arc<dyn Clock> = Arc::new(FixtureClock::new(test_time()));
        let (refresh_adapter, claude_finished, codex_release) =
            crate::providers::test_staggered_observation_coordinator(Arc::clone(&clock));
        let core = NativeCore::open_without_launch(&database.0, clock, refresh_adapter).unwrap();
        let mut codex_release = ReleaseOnDrop(Some(codex_release));
        let notices = core.revision_notices().unwrap();

        core.request_refresh(RefreshSource::Manual).unwrap();
        claude_finished
            .recv_timeout(Duration::from_secs(1))
            .expect("Claude refresh must finish");
        let notice = notices
            .recv_timeout(Duration::from_millis(250))
            .expect("Claude result must commit while Codex remains active");
        let panel = core.panel_state().unwrap();

        assert_eq!(panel.revision, notice.revision);
        assert!(
            core.inner
                .coordinator
                .inbox
                .in_flight
                .load(Ordering::Acquire)
        );
        assert!(matches!(
            panel.provider(CodingProvider::Claude).unwrap().quota,
            ProviderSnapshot::Current { .. }
        ));
        codex_release.0.take().unwrap().send(()).unwrap();
        core.wait_for_refresh_completion().unwrap();
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

    #[derive(Default)]
    struct RecordingProviderCancellationSource {
        cancelled: AtomicU8,
    }

    impl SnapshotRefreshAdapter for RecordingProviderCancellationSource {
        fn cancel_provider(&self, provider: CodingProvider) {
            let bit = match provider {
                CodingProvider::Codex => 1,
                CodingProvider::Claude => 2,
            };
            self.cancelled.fetch_or(bit, Ordering::AcqRel);
        }

        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            _attempt: &RefreshAttempt,
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
            Ok(SnapshotRefreshOutcome::default())
        }
    }

    #[test]
    fn disabling_provider_cancels_only_its_active_adapter_work() {
        let source = Arc::new(RecordingProviderCancellationSource::default());
        let core = NativeCore::with_refresh_adapter(source.clone());

        core.provider_enablement_changed(CodingProvider::Claude, false)
            .unwrap();

        assert_eq!(source.cancelled.load(Ordering::Acquire), 2);
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
            attempt.remaining()?;
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.wait();
            self.release.wait();
            attempt.remaining()?;
            Ok(Some(observed_state(test_time(), 42)).into())
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
            attempt.remaining()?;
            let run = self.runs.fetch_add(1, Ordering::SeqCst);
            if run < 2 {
                self.started.wait();
                self.release.wait();
            }
            attempt.remaining()?;
            let mut state = observed_state(test_time(), 42);
            if run == 0 {
                let stale_usage = state
                    .provider(CodingProvider::Codex)
                    .expect("stale fixture Codex")
                    .usage
                    .clone();
                let stale_claude = state
                    .provider_mut(CodingProvider::Claude)
                    .expect("stale fixture Claude");
                stale_claude.presence = ProviderPresenceStatus::Detected;
                stale_claude.usage = stale_usage;
                state.refresh_combined_usage();
            }
            Ok(Some(state).into())
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
    fn refresh_completion_waits_for_the_native_worker() {
        let source = Arc::new(BlockingRefreshSource::new());
        let core = NativeCore::with_refresh_adapter(source.clone());

        assert!(
            core.request_refresh(RefreshSource::Manual)
                .unwrap()
                .accepted
        );
        source.started.wait();

        let (completed, completion) = mpsc::sync_channel(1);
        let waiting_core = core.clone();
        let waiter = thread::spawn(move || {
            completed
                .send(waiting_core.wait_for_refresh_completion())
                .unwrap();
        });

        assert!(completion.recv_timeout(Duration::from_millis(50)).is_err());
        source.release.wait();
        assert_eq!(
            completion.recv_timeout(Duration::from_secs(1)).unwrap(),
            Ok(())
        );
        waiter.join().unwrap();
        assert_eq!(core.panel_state().unwrap().revision, "2");
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
    fn stale_refresh_commit_preserves_provider_reenable_refresh_status() {
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
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
            Ok(Some(observed_state(test_time(), 42)).into())
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
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
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
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
        let mut connection = Connection::open(database_path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        let mut state = SqliteReadModelStore::read_from(&connection).unwrap();
        let now = test_time();
        state.revision = state
            .revision
            .parse::<u64>()
            .unwrap()
            .checked_add(1)
            .unwrap()
            .to_string();
        state.generated_at = format_time(now);
        state.sync.status = SyncStatus::Pending;

        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 7).unwrap();
        let ranking_day = now.to_offset(time::UtcOffset::UTC).date().to_string();
        let observed_at = u64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(
                &transaction,
                7,
                DailyUsageAggregate {
                    provider,
                    ranking_day: ranking_day.clone(),
                    evidence_basis: match provider {
                        CodingProvider::Codex => SyncEvidenceBasis::ProviderReported,
                        CodingProvider::Claude => SyncEvidenceBasis::LocallyDerived,
                    },
                    coverage: SyncCoverage::Complete,
                    observed_at,
                    observed_tokens: 42,
                    api_equivalent_cost: None,
                    correction_reason: None,
                },
                now,
            )
            .unwrap();
        }
        persist_snapshot(&transaction, &state).unwrap();
        assert_eq!(
            transaction
                .query_row(
                    "SELECT count(*) FROM usage_sync_daily_aggregates",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            transaction
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            2
        );
        process::exit(97);
    }

    #[test]
    fn process_exit_keeps_snapshot_and_both_provider_sync_writes_atomic() {
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
        let cached = relaunched.panel_state().unwrap();

        let connection = Connection::open(&database.0).unwrap();
        let aggregate_count = connection
            .query_row(
                "SELECT count(*) FROM usage_sync_daily_aggregates",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        let outbox_count = connection
            .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(cached.revision, "3");
        assert_eq!((aggregate_count, outbox_count), (0, 0));
        assert_eq!(relaunched.active_usage_sync_generation().unwrap(), None);
        assert_eq!(
            relaunched.menu_bar_headroom().unwrap(),
            crate::quota_headroom::RevisionedOverallQuotaHeadroom {
                revision: 3,
                headroom: expected_headroom(
                    74.0,
                    crate::quota_headroom::HeadroomFreshness::Stale,
                    crate::quota_headroom::HeadroomCompleteness::Incomplete,
                ),
            }
        );
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
    fn migrates_v2_cache_and_marks_the_restored_quota_stale() {
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
        assert_eq!(migrated.revision, "2");
        assert!(matches!(
            &migrated.provider(CodingProvider::Codex).unwrap().quota,
            ProviderSnapshot::Stale { quota_lanes, .. } if quota_lanes.len() == 1
        ));
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
    fn migrates_fixed_v3_cache_to_dynamic_providers_as_stale() {
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

        assert_eq!(migrated.revision, "2");
        let codex = migrated.provider(CodingProvider::Codex).unwrap();
        assert_eq!(codex.display_name, "Codex");
        assert!(matches!(
            &codex.quota,
            ProviderSnapshot::Stale { quota_lanes, .. } if quota_lanes.len() == 1
        ));
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
    fn migrates_v4_cache_to_the_extended_sync_contract() {
        let database = TestDatabase::new();
        let mut legacy = serde_json::to_value(observed_state(test_time(), 42)).unwrap();
        legacy["contractVersion"] = json!(3);
        legacy["sync"] = json!({
            "status": "synced",
            "lastSuccessfulAt": null
        });
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 4),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 3),
                   revision TEXT NOT NULL,
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version) VALUES (?1, 4)",
                [READ_MODEL_SCHEMA_MODULE],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, 4, 3, '1', ?1)",
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
        assert_eq!(migrated.contract_version, CONTRACT_VERSION);
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
        assert_eq!(migrated.sync.status, SyncStatus::Unavailable);

        let connection = Connection::open(&database.0).unwrap();
        let versions = connection
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(
            versions,
            (READ_MODEL_SCHEMA_VERSION, i64::from(CONTRACT_VERSION))
        );
        assert!(read_model_backup_path(&database.0, 4).is_file());
    }

    #[test]
    fn migrates_main_v5_cache_to_the_current_contract() {
        let database = TestDatabase::new();
        let mut previous = serde_json::to_value(observed_state(test_time(), 42)).unwrap();
        previous["contractVersion"] = json!(3);
        previous["sync"] = json!({
            "status": "synced",
            "lastSuccessfulAt": null
        });
        let connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 5),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 3),
                   revision TEXT NOT NULL CHECK (
                     length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
                   ),
                   snapshot_json TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions (module, version) VALUES (?1, 5)",
                [READ_MODEL_SCHEMA_MODULE],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sanitized_desktop_state (
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) VALUES (1, 5, 3, '1', ?1)",
                [previous.to_string()],
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

        assert_eq!(migrated.contract_version, CONTRACT_VERSION);
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
        assert_eq!(migrated.sync.status, SyncStatus::Unavailable);

        let connection = Connection::open(&database.0).unwrap();
        let versions = connection
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(
            versions,
            (READ_MODEL_SCHEMA_VERSION, i64::from(CONTRACT_VERSION))
        );
        let sync_table_count = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'usage_sync_generations'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(sync_table_count, 1);

        let backup = Connection::open(read_model_backup_path(&database.0, 5)).unwrap();
        let backup_versions = backup
            .query_row(
                "SELECT schema_version, contract_version
                 FROM sanitized_desktop_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(backup_versions, (5, 3));
    }

    #[test]
    fn migrates_v6_profile_completion_state_without_losing_usage_sync_rows() {
        let database = TestDatabase::new();
        let initial = observed_state(test_time(), 42);
        let (store, _) = SqliteReadModelStore::open(&database.0, &initial).unwrap();
        drop(store);

        let mut connection = Connection::open(&database.0).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "INSERT INTO usage_sync_generations(active_generation, queue_state)
                   VALUES(1, 'active');
                 INSERT INTO usage_sync_daily_aggregates(
                   active_generation, provider, ranking_day, revision, aggregate_json
                 ) VALUES(1, 'codex', '2026-08-08', 2, '{}');
                 INSERT INTO usage_sync_generation_baselines(
                   active_generation, provider, ranking_day, aggregate_json
                 ) VALUES(1, 'codex', '2026-08-08', '{}');
                 INSERT INTO usage_sync_generation_activations(
                   active_generation, ranking_day, activated_at
                 ) VALUES(1, '2026-08-08', 1786147200000);
                 INSERT INTO usage_sync_latest_outbox(
                   active_generation, provider, ranking_day, revision, snapshot_json,
                   correction_reason, correction_revision, queue_state
                 ) VALUES(
                   1, 'codex', '2026-08-08', 2, '{}',
                   'parser-correction', 2, 'active'
                 );
                 INSERT INTO usage_sync_transfer_day_carryovers(
                   active_generation, provider, ranking_day, carryover_kind
                 ) VALUES(1, 'codex', '2026-08-08', 'pending-segment');
                 INSERT INTO usage_sync_terminal_conflicts(
                   active_generation, provider, ranking_day, revision
                 ) VALUES(1, 'codex', '2026-08-08', 1);
                 INSERT INTO usage_sync_provider_settings_outbox(
                   active_generation, revision, codex_enabled, claude_enabled, delivery_state
                 ) VALUES(1, 2, 1, 0, 'pending');
                 INSERT INTO usage_sync_correction_lineage(
                   provider, ranking_day, source_revision, reason, consumed_generation
                 ) VALUES('codex', '2026-08-08', 2, 'parser-correction', 1);",
            )
            .unwrap();
        transaction.commit().unwrap();

        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 ALTER TABLE usage_sync_generation_activations
                   RENAME TO usage_sync_generation_activations_v7;
                 CREATE TABLE usage_sync_generation_activations (
                   active_generation INTEGER PRIMARY KEY,
                   ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
                   activated_at INTEGER NOT NULL
                     CHECK(activated_at >= 0 AND activated_at <= 9007199254740991),
                   FOREIGN KEY(active_generation)
                     REFERENCES usage_sync_generations(active_generation)
                 ) STRICT;
                 INSERT INTO usage_sync_generation_activations(
                   active_generation, ranking_day, activated_at
                 ) SELECT active_generation, ranking_day, activated_at
                   FROM usage_sync_generation_activations_v7;
                 DROP TABLE usage_sync_generation_activations_v7;
                 ALTER TABLE sanitized_desktop_state
                   RENAME TO sanitized_desktop_state_v7;
                 CREATE TABLE sanitized_desktop_state (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   schema_version INTEGER NOT NULL CHECK (schema_version = 6),
                   contract_version INTEGER NOT NULL CHECK (contract_version = 4),
                   revision TEXT NOT NULL CHECK (
                     length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
                   ),
                   snapshot_json TEXT NOT NULL
                 );
                 INSERT INTO sanitized_desktop_state(
                   singleton, schema_version, contract_version, revision, snapshot_json
                 ) SELECT singleton, 6, contract_version, revision, snapshot_json
                   FROM sanitized_desktop_state_v7;
                 DROP TABLE sanitized_desktop_state_v7;
                 UPDATE touchgrassbar_schema_versions SET version = 6
                 WHERE module = 'sanitized-desktop-state';
                 PRAGMA journal_mode = DELETE;",
            )
            .unwrap();
        drop(connection);

        let (store, migrated) = SqliteReadModelStore::open(&database.0, &initial).unwrap();
        assert_eq!(migrated, initial);
        drop(store);
        let connection = Connection::open(&database.0).unwrap();
        assert_eq!(read_model_schema_version(&connection).unwrap(), 7);
        let retained_tables = [
            "usage_sync_generations",
            "usage_sync_daily_aggregates",
            "usage_sync_generation_baselines",
            "usage_sync_generation_activations",
            "usage_sync_latest_outbox",
            "usage_sync_transfer_day_carryovers",
            "usage_sync_terminal_conflicts",
            "usage_sync_provider_settings_outbox",
            "usage_sync_correction_lineage",
        ];
        for table in retained_tables {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                1,
                "{table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT activated_at, profile_backfill_completed
                     FROM usage_sync_generation_activations WHERE active_generation = 1",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            (1_786_147_200_000, 0)
        );
        assert!(generation_one_profile_backfill_is_pending(&connection).unwrap());
        assert!(read_model_backup_path(&database.0, 6).is_file());
        drop(connection);

        let before = fs::read(&database.0).unwrap();
        let (store, reopened) = SqliteReadModelStore::open(&database.0, &initial).unwrap();
        assert_eq!(reopened, initial);
        drop(store);
        assert_eq!(fs::read(&database.0).unwrap(), before);
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
        wait_for_completed_runs(&source, 1);
        wait_for_idle(&core);

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
        wait_for_idle(&core);

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
        wait_for_idle(&core);
        assert!(core.request_refresh(RefreshSource::Wake).unwrap().accepted);
        assert_eq!(
            notices
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            "6"
        );
        wait_for_completed_runs(&source, 4);
        wait_for_idle(&core);
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
        wait_for_idle(&core);

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
    fn local_usage_catch_up_is_prompt_but_failures_back_off() {
        assert_eq!(
            local_usage_catch_up_delay(false),
            Duration::from_millis(250)
        );
        assert_eq!(local_usage_catch_up_delay(true), Duration::from_secs(60));
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
    fn reset_deadline_keeps_the_last_codex_quota_lane_stale() {
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
        core.wait_for_refresh_completion().unwrap();

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
            ProviderSnapshot::Stale {
                provider: CodingProvider::Codex,
                quota_lanes,
                ..
            } if quota_lanes.len() == 1
                && quota_lanes[0].label == "Weekly limit"
                && quota_lanes[0].remaining == Some(74.0)
        ));
        assert_eq!(
            core.menu_bar_headroom().unwrap().headroom,
            crate::quota_headroom::OverallQuotaHeadroom::Unavailable
        );
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
    fn restart_marks_both_quotas_stale_until_network_recovery_replaces_them() {
        let database = TestDatabase::new();
        let clock = Arc::new(FixtureClock::new(test_time()));
        let reset_at = format_time(test_time() + TimeDuration::minutes(1));
        let mut observed = observed_state(test_time(), 42);
        let ProviderSnapshot::Current { quota_lanes, .. } =
            &mut observed.provider_mut(CodingProvider::Codex).unwrap().quota
        else {
            panic!("Codex fixture quota must be current");
        };
        quota_lanes[0].reset_at = Some(reset_at.clone());
        let claude = observed.provider_mut(CodingProvider::Claude).unwrap();
        claude.presence = ProviderPresenceStatus::Detected;
        claude.quota = ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: format_time(test_time()),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(50.0),
                reset_at: Some(reset_at),
            }],
        };

        let initial_source = Arc::new(ScriptedRefreshSource::new([Ok(Some(observed))]));
        let core =
            NativeCore::open_without_launch(&database.0, clock.clone(), initial_source.clone())
                .unwrap();
        core.request_refresh(RefreshSource::Launch).unwrap();
        wait_for_completed_runs(initial_source.as_ref(), 1);
        core.wait_for_refresh_completion().unwrap();
        drop(core);

        let restored_before_reset = NativeCore::open_without_launch(
            &database.0,
            clock.clone(),
            Arc::new(CachedProjectionRefreshAdapter),
        )
        .unwrap();
        let restored_state = restored_before_reset.panel_state().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            assert!(matches!(
                &restored_state.provider(provider).unwrap().quota,
                ProviderSnapshot::Stale { quota_lanes, .. }
                    if quota_lanes.len() == 1
            ));
        }
        drop(restored_before_reset);

        clock.advance(Duration::from_secs(2 * 60));
        let refreshed_at = test_time() + TimeDuration::minutes(2);
        let mut refreshed = observed_state(refreshed_at, 43);
        let ProviderSnapshot::Current { quota_lanes, .. } =
            &mut refreshed.provider_mut(CodingProvider::Codex).unwrap().quota
        else {
            panic!("refreshed Codex quota must be current");
        };
        quota_lanes[0].remaining = Some(80.0);
        let refreshed_claude = refreshed.provider_mut(CodingProvider::Claude).unwrap();
        refreshed_claude.presence = ProviderPresenceStatus::Detected;
        refreshed_claude.quota = ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: format_time(refreshed_at),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(60.0),
                reset_at: Some(format_time(refreshed_at + TimeDuration::days(7))),
            }],
        };
        let recovery_source = Arc::new(ScriptedRefreshSource::new([Ok(Some(refreshed))]));
        let reopened =
            NativeCore::open_without_launch(&database.0, clock, recovery_source.clone()).unwrap();

        let restored = reopened.panel_state().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            assert!(matches!(
                &restored.provider(provider).unwrap().quota,
                ProviderSnapshot::Stale { quota_lanes, .. }
                    if quota_lanes.len() == 1
            ));
        }

        reopened
            .request_refresh(RefreshSource::NetworkRecovery)
            .unwrap();
        wait_for_completed_runs(recovery_source.as_ref(), 1);
        reopened.wait_for_refresh_completion().unwrap();
        let recovered = reopened.panel_state().unwrap();
        assert!(matches!(
            &recovered.provider(CodingProvider::Codex).unwrap().quota,
            ProviderSnapshot::Current { quota_lanes, .. }
                if quota_lanes[0].remaining == Some(80.0)
        ));
        assert!(matches!(
            &recovered.provider(CodingProvider::Claude).unwrap().quota,
            ProviderSnapshot::Current { quota_lanes, .. }
                if quota_lanes[0].remaining == Some(60.0)
        ));
    }

    #[test]
    fn stale_quota_reset_crossing_requests_one_revision_without_dropping_the_lane() {
        let reset_at = test_time() + TimeDuration::minutes(1);
        let mut snapshot = observed_state(test_time(), 42);
        snapshot.generated_at = format_time(test_time());
        snapshot.provider_mut(CodingProvider::Codex).unwrap().quota = ProviderSnapshot::Stale {
            provider: CodingProvider::Codex,
            observed_at: format_time(test_time()),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(74.0),
                reset_at: Some(format_time(reset_at)),
            }],
        };

        assert_eq!(next_refresh_at(&snapshot, test_time()), reset_at);
        let mut transitioned =
            transition_snapshot_at(&snapshot, test_time() + TimeDuration::minutes(2))
                .expect("the reset crossing must request a revision");
        assert!(matches!(
            &transitioned.provider(CodingProvider::Codex).unwrap().quota,
            ProviderSnapshot::Stale { quota_lanes, .. }
                if quota_lanes.len() == 1
                    && quota_lanes[0].remaining == Some(74.0)
        ));

        transitioned.generated_at = format_time(test_time() + TimeDuration::minutes(2));
        assert!(
            transition_snapshot_at(&transitioned, test_time() + TimeDuration::minutes(3),)
                .is_none()
        );
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
