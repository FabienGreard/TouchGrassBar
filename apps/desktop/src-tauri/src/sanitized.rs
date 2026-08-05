use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::lifecycle::{
    LIFECYCLE_CONTRACT_VERSION, SETTINGS_NAVIGATION_EVENT, bootstrap_state_schema,
    settings_navigation_schema, settings_state_schema,
};

pub const CONTRACT_VERSION: u8 = 1;
pub const REVISION_NOTICE_EVENT: &str = "sanitized-desktop-state-revision";

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDesktopStateV1 {
    pub contract_version: u8,
    pub generated_at: String,
    pub revision: String,
    pub providers: [ProviderSnapshot; 2],
    pub usage: UsageByProvider,
    pub sync: SyncState,
    pub profile: SanitizedProfileOutcome,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    tag = "status"
)]
pub enum SanitizedProfileOutcome {
    NotAuthorized,
    IdentityPending,
    Ready {
        display_name: String,
        touch_grass_id: String,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, JsonSchema, Serialize)]
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

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodingProvider {
    Codex,
    Claude,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
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

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct UsageByProvider {
    pub codex: UsagePeriods,
    pub claude: UsagePeriods,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriods {
    pub today: UsageTotal,
    pub seven_days: UsageTotal,
    pub thirty_days: UsageTotal,
}

#[allow(dead_code)]
#[derive(Clone, Debug, JsonSchema, Serialize)]
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
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageEvidenceBasis {
    ProviderReported,
    LocallyDerived,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageCoverage {
    Complete,
    Partial,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub status: SyncStatus,
    pub last_successful_at: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
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

trait RefreshSource: Send + Sync {
    fn refresh(
        &self,
        cached: SanitizedDesktopStateV1,
    ) -> Result<SanitizedDesktopStateV1, &'static str>;
}

struct CachedProjectionRefreshSource;

impl RefreshSource for CachedProjectionRefreshSource {
    fn refresh(
        &self,
        cached: SanitizedDesktopStateV1,
    ) -> Result<SanitizedDesktopStateV1, &'static str> {
        // Provider observation is not wired yet. Recommitting the cached sanitized
        // projection keeps the unavailable state truthful while exercising the same
        // asynchronous coordinator that future provider sources will use.
        Ok(cached)
    }
}

struct NativeCoreInner {
    state: Mutex<SanitizedDesktopStateV1>,
    revision_subscribers: Mutex<Vec<Sender<RevisionNotice>>>,
    refresh_in_flight: Mutex<bool>,
    refresh_source: Arc<dyn RefreshSource>,
}

impl NativeCoreInner {
    fn run_refresh(&self) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let cached = self
                .state
                .lock()
                .map_err(|_| "native state unavailable")?
                .clone();
            let refreshed = self.refresh_source.refresh(cached)?;
            self.commit_refreshed_snapshot(refreshed)
        }));

        if let Ok(mut in_flight) = self.refresh_in_flight.lock() {
            *in_flight = false;
        }
    }

    fn commit_refreshed_snapshot(
        &self,
        mut refreshed: SanitizedDesktopStateV1,
    ) -> Result<(), &'static str> {
        let notice = {
            let mut state = self.state.lock().map_err(|_| "native state unavailable")?;
            let revision = state
                .revision
                .parse::<u64>()
                .ok()
                .and_then(|revision| revision.checked_add(1))
                .ok_or("native revision unavailable")?;

            refreshed.contract_version = CONTRACT_VERSION;
            refreshed.generated_at = now();
            refreshed.revision = revision.to_string();
            *state = refreshed;

            RevisionNotice {
                revision: revision.to_string(),
            }
        };

        let mut subscribers = self
            .revision_subscribers
            .lock()
            .map_err(|_| "revision notices unavailable")?;
        subscribers.retain(|subscriber| subscriber.send(notice.clone()).is_ok());
        Ok(())
    }
}

#[derive(Clone)]
pub struct NativeCore {
    inner: Arc<NativeCoreInner>,
}

impl NativeCore {
    pub fn unavailable() -> Self {
        Self::with_refresh_source(Arc::new(CachedProjectionRefreshSource))
    }

    fn with_refresh_source(refresh_source: Arc<dyn RefreshSource>) -> Self {
        Self {
            inner: Arc::new(NativeCoreInner {
                state: Mutex::new(unavailable_state(1)),
                revision_subscribers: Mutex::new(Vec::new()),
                refresh_in_flight: Mutex::new(false),
                refresh_source,
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

    pub fn set_profile_outcome(
        &self,
        profile: SanitizedProfileOutcome,
    ) -> Result<(), &'static str> {
        let mut state = self.panel_state()?;
        if state.profile == profile {
            return Ok(());
        }
        state.profile = profile;
        self.inner.commit_refreshed_snapshot(state)
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

    pub fn request_refresh(&self) -> Result<RefreshReceipt, &'static str> {
        {
            let mut in_flight = self
                .inner
                .refresh_in_flight
                .lock()
                .map_err(|_| "refresh coordinator unavailable")?;
            if *in_flight {
                return Ok(RefreshReceipt { accepted: true });
            }
            *in_flight = true;
        }

        let inner = Arc::clone(&self.inner);
        if thread::Builder::new()
            .name("sanitized-state-refresh".to_owned())
            .spawn(move || inner.run_refresh())
            .is_err()
        {
            if let Ok(mut in_flight) = self.inner.refresh_in_flight.lock() {
                *in_flight = false;
            }
            return Err("refresh coordinator unavailable");
        }

        Ok(RefreshReceipt { accepted: true })
    }
}

fn unavailable_periods() -> UsagePeriods {
    UsagePeriods {
        today: UsageTotal::Unavailable,
        seven_days: UsageTotal::Unavailable,
        thirty_days: UsageTotal::Unavailable,
    }
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

pub fn unavailable_state(revision: u64) -> SanitizedDesktopStateV1 {
    SanitizedDesktopStateV1 {
        contract_version: CONTRACT_VERSION,
        generated_at: now(),
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
        profile: SanitizedProfileOutcome::NotAuthorized,
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
        sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use serde_json::{Value, json};

    use super::*;

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
        let core = NativeCore::unavailable();
        let notices = core.revision_notices().unwrap();

        let receipt = core.request_refresh().unwrap();
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();
        let after = core.panel_state().unwrap();

        assert!(receipt.accepted);
        assert_eq!(notice.revision, "2");
        assert_eq!(after.revision, notice.revision);
        assert!(matches!(after.usage.codex.today, UsageTotal::Unavailable));
    }

    #[test]
    fn refresh_drops_closed_notice_receivers_without_rejecting_work() {
        let core = NativeCore::unavailable();
        drop(core.revision_notices().unwrap());
        let live_notices = core.revision_notices().unwrap();

        assert!(core.request_refresh().unwrap().accepted);
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

    impl RefreshSource for BlockingRefreshSource {
        fn refresh(
            &self,
            cached: SanitizedDesktopStateV1,
        ) -> Result<SanitizedDesktopStateV1, &'static str> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.wait();
            self.release.wait();
            Ok(cached)
        }
    }

    #[test]
    fn concurrent_refresh_requests_join_one_in_flight_commit() {
        let source = Arc::new(BlockingRefreshSource::new());
        let core = NativeCore::with_refresh_source(source.clone());
        let notices = core.revision_notices().unwrap();

        assert!(core.request_refresh().unwrap().accepted);
        source.started.wait();

        assert_eq!(core.panel_state().unwrap().revision, "1");
        assert!(core.request_refresh().unwrap().accepted);
        assert_eq!(source.runs.load(Ordering::SeqCst), 1);

        source.release.wait();
        let notice = notices.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(notice.revision, "2");
        assert_eq!(core.panel_state().unwrap().revision, "2");
        assert!(notices.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(source.runs.load(Ordering::SeqCst), 1);
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
