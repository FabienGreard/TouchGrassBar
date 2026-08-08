//! Bounded Pending Usage Snapshot synchronization runtime.

use std::{
    io,
    sync::{
        Arc, Condvar, Mutex, Weak,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use time::OffsetDateTime;

use crate::{
    profile::{ActiveSyncCredentials, ProfileCoordinator, Secret},
    sanitized::NativeCore,
    updater::OnlineFeatureGate,
};

use super::{
    PendingUsageBatch, UsageSyncAcknowledgement,
    transport::{HttpUsageSyncTransport, UsageSyncTransportOutcome},
};

const WORKER_INTERVAL: Duration = Duration::from_secs(300);
const UPDATE_PAUSE_TIMEOUT: Duration = Duration::from_secs(40);

/// The complete local result of one protected delivery attempt.
///
/// Active Mac rejection has a separate operation because it can occur before
/// the runtime has a batch.
pub(crate) enum UsageSyncAttemptResult {
    Committed(Vec<UsageSyncAcknowledgement>),
    Offline,
    Deferred,
}

/// The atomic local-state seam used by the delivery state machine.
///
/// The production Adapter keeps the Sanitized Desktop State and outbox in one
/// SQLite transaction. Test Adapters can drive the same state machine without
/// Profile or network access.
trait PendingUsageSnapshotState: Send + Sync {
    fn prepare(
        &self,
        active_mac_generation: u64,
    ) -> Result<Option<PendingUsageBatch>, &'static str>;

    fn finish(
        &self,
        batch: &PendingUsageBatch,
        result: UsageSyncAttemptResult,
    ) -> Result<(), &'static str>;

    fn reject_authority(&self) -> Result<(), &'static str>;

    fn install_usage_sync_request(
        &self,
        request: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), &'static str>;

    fn clear_request(&self);
}

struct NativePendingUsageSnapshotState {
    core: NativeCore,
}

impl PendingUsageSnapshotState for NativePendingUsageSnapshotState {
    fn prepare(
        &self,
        active_mac_generation: u64,
    ) -> Result<Option<PendingUsageBatch>, &'static str> {
        self.core.prepare_usage_sync_attempt(active_mac_generation)
    }

    fn finish(
        &self,
        batch: &PendingUsageBatch,
        result: UsageSyncAttemptResult,
    ) -> Result<(), &'static str> {
        self.core.finish_usage_sync_attempt(batch, result)
    }

    fn reject_authority(&self) -> Result<(), &'static str> {
        self.core.reject_active_usage_sync_authority()
    }

    fn install_usage_sync_request(
        &self,
        request: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), &'static str> {
        self.core.install_usage_sync_request(request)
    }

    fn clear_request(&self) {
        self.core.clear_usage_sync_request();
    }
}

pub(crate) struct SynchronizationEnvironment {
    state: Arc<dyn PendingUsageSnapshotState>,
    online_gate: OnlineFeatureGate,
    authority: Arc<dyn ActiveMacAuthoritySource>,
    delivery: Arc<dyn PendingUsageSnapshotDelivery>,
    clock: Arc<dyn SynchronizationClock>,
    retry_interval: Duration,
}

impl SynchronizationEnvironment {
    pub(crate) fn production(
        core: NativeCore,
        profile: Arc<Mutex<ProfileCoordinator>>,
        online_gate: OnlineFeatureGate,
    ) -> Self {
        Self {
            state: Arc::new(NativePendingUsageSnapshotState { core }),
            online_gate,
            authority: Arc::new(ProfileActiveMacAuthority { profile }),
            delivery: Arc::new(HttpPendingUsageSnapshotDelivery {
                transport: HttpUsageSyncTransport::from_build_configuration(),
            }),
            clock: Arc::new(SystemSynchronizationClock),
            retry_interval: WORKER_INTERVAL,
        }
    }

    /// Build a debug environment that never reads Profile or calls Convex.
    #[cfg(debug_assertions)]
    pub(crate) fn no_io(core: NativeCore, online_gate: OnlineFeatureGate) -> Self {
        Self {
            state: Arc::new(NativePendingUsageSnapshotState { core }),
            online_gate,
            authority: Arc::new(NoActiveMacAuthority),
            delivery: Arc::new(NoPendingUsageSnapshotDelivery),
            clock: Arc::new(SystemSynchronizationClock),
            retry_interval: WORKER_INTERVAL,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PendingUsageSynchronization {
    inner: Arc<RuntimeInner>,
}

impl PendingUsageSynchronization {
    pub(crate) fn start(environment: SynchronizationEnvironment) -> io::Result<Self> {
        let (wake, requests) = mpsc::sync_channel(1);
        let inner = Arc::new(RuntimeInner {
            environment,
            admission: SynchronizationAdmission::default(),
            wake,
            worker: Mutex::new(None),
        });

        let worker = Arc::downgrade(&inner);
        let retry_interval = inner.environment.retry_interval;
        let worker = thread::Builder::new()
            .name("pending-usage-synchronization".to_owned())
            .spawn(move || run_worker(worker, requests, retry_interval))?;
        let mut worker_slot = inner
            .worker
            .lock()
            .map_err(|_| io::Error::other("usage synchronization unavailable"))?;
        *worker_slot = Some(worker);
        drop(worker_slot);

        let callback_runtime = Arc::downgrade(&inner);
        let install_result =
            inner
                .environment
                .state
                .install_usage_sync_request(Arc::new(move || {
                    if let Some(runtime) = callback_runtime.upgrade() {
                        runtime.request();
                    }
                }));
        if let Err(error) = install_result {
            inner.shutdown();
            return Err(io::Error::other(error));
        }

        Ok(Self { inner })
    }

    /// Request one synchronization pass or join the current pass.
    pub(crate) fn request(&self) {
        self.inner.request();
    }

    /// Close admission and wait a bounded time for the active pass.
    pub(crate) fn pause_for_update(&self) -> Result<SynchronizationPause<'_>, ()> {
        self.inner.pause_for_update()?;
        Ok(SynchronizationPause {
            runtime: self,
            resume_on_drop: true,
        })
    }

    /// Stop new work. Pending Usage Snapshots stay durable for the next run.
    pub(crate) fn shutdown(&self) {
        self.inner.shutdown();
    }
}

pub(crate) struct SynchronizationPause<'a> {
    runtime: &'a PendingUsageSynchronization,
    resume_on_drop: bool,
}

impl SynchronizationPause<'_> {
    pub(crate) fn keep_paused(mut self) {
        self.resume_on_drop = false;
    }
}

impl Drop for SynchronizationPause<'_> {
    fn drop(&mut self) {
        if self.resume_on_drop {
            self.runtime.inner.resume_after_update();
        }
    }
}

struct RuntimeInner {
    environment: SynchronizationEnvironment,
    admission: SynchronizationAdmission,
    wake: SyncSender<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl RuntimeInner {
    fn request(&self) {
        if self.admission.request() == RequestAdmission::Wake {
            self.wake();
        }
    }

    fn wake(&self) {
        let _ = self.wake.try_send(());
    }

    fn run_attempt(self: &Arc<Self>) {
        if self.environment.online_gate.is_paused() {
            return;
        }
        let Some(_attempt) = self.start_attempt() else {
            return;
        };
        if self.environment.online_gate.is_paused() {
            return;
        }

        let authority = match self.environment.authority.acquire() {
            ActiveMacAuthorityOutcome::Ready(authority) => authority,
            ActiveMacAuthorityOutcome::Rejected => {
                let _ = self.environment.state.reject_authority();
                return;
            }
            ActiveMacAuthorityOutcome::Unavailable => return,
        };
        let batch = match self
            .environment
            .state
            .prepare(authority.active_mac_generation)
        {
            Ok(Some(batch)) => batch,
            Ok(None) | Err(_) => return,
        };
        match self
            .environment
            .delivery
            .deliver(&authority, &batch, self.environment.clock.now())
        {
            PendingUsageSnapshotDeliveryOutcome::Committed(acknowledgements) => {
                let _ = self
                    .environment
                    .state
                    .finish(&batch, UsageSyncAttemptResult::Committed(acknowledgements));
            }
            PendingUsageSnapshotDeliveryOutcome::Offline => {
                let _ = self
                    .environment
                    .state
                    .finish(&batch, UsageSyncAttemptResult::Offline);
            }
            PendingUsageSnapshotDeliveryOutcome::Deferred => {
                let _ = self
                    .environment
                    .state
                    .finish(&batch, UsageSyncAttemptResult::Deferred);
            }
            PendingUsageSnapshotDeliveryOutcome::AuthorityRejected => {
                let _ = self.environment.state.reject_authority();
            }
        }
    }

    fn start_attempt(self: &Arc<Self>) -> Option<AttemptGuard> {
        self.admission
            .start()
            .then(|| AttemptGuard(Arc::downgrade(self)))
    }

    fn pause_for_update(&self) -> Result<(), ()> {
        if self.admission.pause(UPDATE_PAUSE_TIMEOUT).is_err() {
            self.request();
            return Err(());
        }
        Ok(())
    }

    fn resume_after_update(&self) {
        if self.admission.resume() {
            self.wake();
        }
    }

    fn shutdown(&self) {
        if self.admission.stop() {
            self.environment.state.clear_request();
            self.wake();
        }
        self.join_worker();
    }

    fn join_worker(&self) {
        let worker = self.worker.lock().ok().and_then(|mut worker| worker.take());
        let Some(worker) = worker else {
            return;
        };
        if worker.thread().id() != thread::current().id() {
            let _ = worker.join();
        }
    }
}

struct AttemptGuard(Weak<RuntimeInner>);

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let Some(runtime) = self.0.upgrade() else {
            return;
        };
        if runtime.admission.finish() {
            runtime.wake();
        }
    }
}

fn run_worker(runtime: Weak<RuntimeInner>, requests: Receiver<()>, retry_interval: Duration) {
    while let Ok(()) | Err(RecvTimeoutError::Timeout) = requests.recv_timeout(retry_interval) {
        let Some(runtime) = runtime.upgrade() else {
            break;
        };
        if runtime.admission.is_stopped() {
            break;
        }
        runtime.run_attempt();
    }
}

trait SynchronizationClock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

struct SystemSynchronizationClock;

impl SynchronizationClock for SystemSynchronizationClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestAdmission {
    Wake,
    Coalesced,
    Stopped,
}

#[derive(Default)]
struct SynchronizationAdmission {
    state: Mutex<SynchronizationAdmissionState>,
    idle: Condvar,
}

#[derive(Default)]
struct SynchronizationAdmissionState {
    in_flight: bool,
    pause_depth: usize,
    rerun: bool,
    stopped: bool,
}

impl SynchronizationAdmission {
    fn request(&self) -> RequestAdmission {
        let Ok(mut state) = self.state.lock() else {
            return RequestAdmission::Stopped;
        };
        if state.stopped {
            return RequestAdmission::Stopped;
        }
        if state.in_flight || state.pause_depth > 0 {
            state.rerun = true;
            return RequestAdmission::Coalesced;
        }
        RequestAdmission::Wake
    }

    fn start(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.stopped || state.in_flight || state.pause_depth > 0 {
            if !state.stopped {
                state.rerun = true;
            }
            return false;
        }
        state.in_flight = true;
        state.rerun = false;
        true
    }

    /// Finish one pass and report whether its one sticky rerun should wake.
    fn finish(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if !state.in_flight {
            return false;
        }
        state.in_flight = false;
        self.idle.notify_all();
        if state.rerun && state.pause_depth == 0 && !state.stopped {
            state.rerun = false;
            return true;
        }
        false
    }

    fn pause(&self, timeout: Duration) -> Result<(), ()> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().map_err(|_| ())?;
        if state.stopped {
            return Err(());
        }
        state.pause_depth = state.pause_depth.checked_add(1).ok_or(())?;
        state.rerun = true;
        while state.in_flight && !state.stopped {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.pause_depth = state.pause_depth.saturating_sub(1);
                return Err(());
            }
            let (next, wait) = self.idle.wait_timeout(state, remaining).map_err(|_| ())?;
            state = next;
            if wait.timed_out() && state.in_flight {
                state.pause_depth = state.pause_depth.saturating_sub(1);
                return Err(());
            }
        }
        if state.stopped {
            state.pause_depth = state.pause_depth.saturating_sub(1);
            return Err(());
        }
        Ok(())
    }

    fn resume(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.pause_depth == 0 {
            return false;
        }
        state.pause_depth -= 1;
        if state.pause_depth == 0 && state.rerun && !state.stopped {
            state.rerun = false;
            return true;
        }
        false
    }

    fn stop(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.stopped {
            return false;
        }
        state.stopped = true;
        state.rerun = false;
        self.idle.notify_all();
        true
    }

    fn is_stopped(&self) -> bool {
        self.state.lock().map_or(true, |state| state.stopped)
    }
}

trait ActiveMacAuthoritySource: Send + Sync {
    fn acquire(&self) -> ActiveMacAuthorityOutcome;
}

struct ActiveMacAuthority {
    active_mac_generation: u64,
    installation_credential: Secret,
    session: Secret,
}

enum ActiveMacAuthorityOutcome {
    Ready(ActiveMacAuthority),
    Rejected,
    Unavailable,
}

struct ProfileActiveMacAuthority {
    profile: Arc<Mutex<ProfileCoordinator>>,
}

impl ActiveMacAuthoritySource for ProfileActiveMacAuthority {
    fn acquire(&self) -> ActiveMacAuthorityOutcome {
        let Ok(profile) = self.profile.lock() else {
            return ActiveMacAuthorityOutcome::Unavailable;
        };
        match profile.active_sync_credentials() {
            Ok(Some(credentials)) => ActiveMacAuthorityOutcome::Ready(credentials.into()),
            Ok(None) => ActiveMacAuthorityOutcome::Unavailable,
            Err(error) if error.is_authority_rejected() => ActiveMacAuthorityOutcome::Rejected,
            Err(_) => ActiveMacAuthorityOutcome::Unavailable,
        }
    }
}

impl From<ActiveSyncCredentials> for ActiveMacAuthority {
    fn from(credentials: ActiveSyncCredentials) -> Self {
        Self {
            active_mac_generation: credentials.active_mac_generation,
            installation_credential: credentials.installation_credential,
            session: credentials.session,
        }
    }
}

trait PendingUsageSnapshotDelivery: Send + Sync {
    fn deliver(
        &self,
        authority: &ActiveMacAuthority,
        batch: &PendingUsageBatch,
        now: OffsetDateTime,
    ) -> PendingUsageSnapshotDeliveryOutcome;
}

enum PendingUsageSnapshotDeliveryOutcome {
    Committed(Vec<UsageSyncAcknowledgement>),
    Offline,
    AuthorityRejected,
    Deferred,
}

struct HttpPendingUsageSnapshotDelivery {
    transport: HttpUsageSyncTransport,
}

impl PendingUsageSnapshotDelivery for HttpPendingUsageSnapshotDelivery {
    fn deliver(
        &self,
        authority: &ActiveMacAuthority,
        batch: &PendingUsageBatch,
        now: OffsetDateTime,
    ) -> PendingUsageSnapshotDeliveryOutcome {
        match self.transport.send(
            &authority.session,
            &authority.installation_credential,
            batch,
            now,
        ) {
            UsageSyncTransportOutcome::Committed(acknowledgements) => {
                PendingUsageSnapshotDeliveryOutcome::Committed(acknowledgements)
            }
            UsageSyncTransportOutcome::Offline => PendingUsageSnapshotDeliveryOutcome::Offline,
            UsageSyncTransportOutcome::AuthorityRejected => {
                PendingUsageSnapshotDeliveryOutcome::AuthorityRejected
            }
            UsageSyncTransportOutcome::Deferred => PendingUsageSnapshotDeliveryOutcome::Deferred,
        }
    }
}

#[cfg(debug_assertions)]
struct NoActiveMacAuthority;

#[cfg(debug_assertions)]
impl ActiveMacAuthoritySource for NoActiveMacAuthority {
    fn acquire(&self) -> ActiveMacAuthorityOutcome {
        ActiveMacAuthorityOutcome::Unavailable
    }
}

#[cfg(debug_assertions)]
struct NoPendingUsageSnapshotDelivery;

#[cfg(debug_assertions)]
impl PendingUsageSnapshotDelivery for NoPendingUsageSnapshotDelivery {
    fn deliver(
        &self,
        _authority: &ActiveMacAuthority,
        _batch: &PendingUsageBatch,
        _now: OffsetDateTime,
    ) -> PendingUsageSnapshotDeliveryOutcome {
        PendingUsageSnapshotDeliveryOutcome::Deferred
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use time::format_description::well_known::Rfc3339;

    use crate::sanitized::{
        CONTRACT_VERSION, Clock, CodingProvider, ProviderPresenceStatus, ProviderPresentation,
        ProviderSnapshot, RefreshAttempt, RefreshFailure, SanitizedDesktopStateV3,
        SanitizedProfileOutcome, SnapshotRefreshAdapter, SnapshotRefreshOutcome, SyncState,
        SyncStatus, UsageCoverage, UsageEvidenceBasis, UsagePeriods, UsageScanStatus, UsageTotal,
    };

    use super::*;

    struct FixedSynchronizationClock(OffsetDateTime);

    impl SynchronizationClock for FixedSynchronizationClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    impl Clock for FixedSynchronizationClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    static NEXT_DATABASE: AtomicUsize = AtomicUsize::new(1);

    struct TestDatabase(PathBuf);

    impl TestDatabase {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(env::temp_dir().join(format!(
                "touchgrassbar-sync-runtime-{}-{timestamp}-{}.sqlite3",
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
            ] {
                let _ = fs::remove_file(path);
            }
        }
    }

    struct OneObservation(Mutex<Option<SanitizedDesktopStateV3>>);

    impl SnapshotRefreshAdapter for OneObservation {
        fn refresh(
            &self,
            _cached: SanitizedDesktopStateV3,
            _attempt: &RefreshAttempt,
        ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
            Ok(self
                .0
                .lock()
                .ok()
                .and_then(|mut snapshot| snapshot.take())
                .into())
        }
    }

    struct ReadyAuthority;

    impl ActiveMacAuthoritySource for ReadyAuthority {
        fn acquire(&self) -> ActiveMacAuthorityOutcome {
            ActiveMacAuthorityOutcome::Ready(ActiveMacAuthority {
                active_mac_generation: 7,
                installation_credential: Secret::test_only(),
                session: Secret::test_only(),
            })
        }
    }

    struct CommittingDelivery(SyncSender<(CodingProvider, u64)>);

    impl PendingUsageSnapshotDelivery for CommittingDelivery {
        fn deliver(
            &self,
            _authority: &ActiveMacAuthority,
            batch: &PendingUsageBatch,
            _now: OffsetDateTime,
        ) -> PendingUsageSnapshotDeliveryOutcome {
            let acknowledgements = batch
                .snapshots()
                .iter()
                .map(|snapshot| {
                    let _ = self.0.send((snapshot.provider, snapshot.revision));
                    UsageSyncAcknowledgement {
                        provider: snapshot.provider,
                        ranking_day: snapshot.ranking_day.clone(),
                        revision: snapshot.revision,
                        outcome: super::super::AcknowledgementOutcome::Committed,
                    }
                })
                .collect();
            PendingUsageSnapshotDeliveryOutcome::Committed(acknowledgements)
        }
    }

    fn observed_state(now: OffsetDateTime) -> SanitizedDesktopStateV3 {
        let observed_at = now.format(&Rfc3339).unwrap();
        let unavailable_usage = UsagePeriods {
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let codex_usage = UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens: 42,
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
        SanitizedDesktopStateV3 {
            contract_version: CONTRACT_VERSION,
            generated_at: observed_at,
            revision: "1".to_owned(),
            providers: vec![
                ProviderPresentation {
                    provider: CodingProvider::Codex,
                    display_name: "Codex".to_owned(),
                    presence: ProviderPresenceStatus::Detected,
                    quota: ProviderSnapshot::Unavailable {
                        provider: CodingProvider::Codex,
                        quota_lanes: [],
                    },
                    usage: codex_usage.clone(),
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
                    usage: unavailable_usage,
                    top_model_usage: None,
                },
            ],
            top_model_usage: None,
            combined_usage: codex_usage,
            sync: SyncState {
                status: SyncStatus::Unavailable,
                last_successful_at: None,
            },
            profile: SanitizedProfileOutcome::NotAuthorized,
        }
    }

    struct SignallingAuthority {
        calls: SyncSender<usize>,
        count: AtomicUsize,
        release_first: Mutex<Option<Receiver<()>>>,
    }

    impl ActiveMacAuthoritySource for SignallingAuthority {
        fn acquire(&self) -> ActiveMacAuthorityOutcome {
            let call = self.count.fetch_add(1, Ordering::AcqRel);
            let _ = self.calls.send(call);
            if call == 0
                && let Some(release) = self
                    .release_first
                    .lock()
                    .ok()
                    .and_then(|mut release| release.take())
            {
                let _ = release.recv();
            }
            ActiveMacAuthorityOutcome::Unavailable
        }
    }

    fn test_environment(
        authority: Arc<dyn ActiveMacAuthoritySource>,
    ) -> SynchronizationEnvironment {
        SynchronizationEnvironment {
            state: Arc::new(NativePendingUsageSnapshotState {
                core: NativeCore::no_io_unavailable(),
            }),
            online_gate: OnlineFeatureGate::default(),
            authority,
            delivery: Arc::new(NoPendingUsageSnapshotDelivery),
            clock: Arc::new(FixedSynchronizationClock(OffsetDateTime::UNIX_EPOCH)),
            retry_interval: Duration::from_secs(60),
        }
    }

    #[test]
    fn one_request_starts_one_module_attempt() {
        let (calls, observed_calls) = mpsc::sync_channel(4);
        let runtime =
            PendingUsageSynchronization::start(test_environment(Arc::new(SignallingAuthority {
                calls,
                count: AtomicUsize::new(0),
                release_first: Mutex::new(None),
            })))
            .unwrap();

        assert!(observed_calls.try_recv().is_err());
        runtime.request();
        assert_eq!(observed_calls.recv_timeout(Duration::from_secs(1)), Ok(0));
        assert!(
            observed_calls
                .recv_timeout(Duration::from_millis(25))
                .is_err()
        );

        runtime.shutdown();
        runtime.shutdown();
    }

    #[test]
    fn requests_during_a_module_attempt_make_one_rerun() {
        let (calls, observed_calls) = mpsc::sync_channel(4);
        let (release_first, first_release) = mpsc::sync_channel(1);
        let runtime =
            PendingUsageSynchronization::start(test_environment(Arc::new(SignallingAuthority {
                calls,
                count: AtomicUsize::new(0),
                release_first: Mutex::new(Some(first_release)),
            })))
            .unwrap();

        runtime.request();
        assert_eq!(observed_calls.recv_timeout(Duration::from_secs(1)), Ok(0));
        runtime.request();
        runtime.request();
        release_first.send(()).unwrap();
        assert_eq!(observed_calls.recv_timeout(Duration::from_secs(1)), Ok(1));
        assert!(
            observed_calls
                .recv_timeout(Duration::from_millis(25))
                .is_err()
        );

        runtime.shutdown();
    }

    #[test]
    fn request_commits_a_real_sqlite_outbox_through_test_adapters() {
        let now = OffsetDateTime::from_unix_timestamp(1_775_908_800).unwrap();
        let database = TestDatabase::new();
        let clock = Arc::new(FixedSynchronizationClock(now));
        let core = NativeCore::open_with(
            &database.0,
            clock.clone(),
            Arc::new(OneObservation(Mutex::new(Some(observed_state(now))))),
        )
        .unwrap();
        core.wait_for_refresh_completion().unwrap();
        let (delivered, observed_delivery) = mpsc::sync_channel(2);
        let runtime = PendingUsageSynchronization::start(SynchronizationEnvironment {
            state: Arc::new(NativePendingUsageSnapshotState { core: core.clone() }),
            online_gate: OnlineFeatureGate::default(),
            authority: Arc::new(ReadyAuthority),
            delivery: Arc::new(CommittingDelivery(delivered)),
            clock,
            retry_interval: Duration::from_secs(60),
        })
        .unwrap();

        runtime.request();

        assert_eq!(
            observed_delivery.recv_timeout(Duration::from_secs(1)),
            Ok((CodingProvider::Codex, 1))
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while core.panel_state().unwrap().sync.status != SyncStatus::Synced {
            assert!(Instant::now() < deadline, "synchronization did not commit");
            thread::yield_now();
        }
        assert!(core.pending_usage_sync_batch(7).unwrap().is_none());

        runtime.shutdown();
        core.shutdown();
    }

    #[test]
    fn active_requests_have_one_sticky_rerun() {
        let admission = SynchronizationAdmission::default();

        assert!(admission.start());
        assert_eq!(admission.request(), RequestAdmission::Coalesced);
        assert_eq!(admission.request(), RequestAdmission::Coalesced);
        assert!(admission.finish());

        assert!(admission.start());
        assert!(!admission.finish());
    }

    #[test]
    fn paused_requests_wake_once_after_resume() {
        let admission = SynchronizationAdmission::default();

        admission.pause(Duration::from_secs(1)).unwrap();
        assert_eq!(admission.request(), RequestAdmission::Coalesced);
        assert_eq!(admission.request(), RequestAdmission::Coalesced);
        assert!(admission.resume());
        assert!(!admission.resume());
    }

    #[test]
    fn pause_wait_is_bounded_and_keeps_the_rerun() {
        let admission = SynchronizationAdmission::default();
        assert!(admission.start());

        let started_at = Instant::now();
        assert_eq!(admission.pause(Duration::from_millis(1)), Err(()));
        assert!(started_at.elapsed() < Duration::from_secs(1));
        assert!(admission.finish());
    }

    #[test]
    fn stop_is_idempotent_and_closes_admission() {
        let admission = SynchronizationAdmission::default();

        assert!(admission.stop());
        assert!(!admission.stop());
        assert_eq!(admission.request(), RequestAdmission::Stopped);
        assert!(!admission.start());
    }
}
