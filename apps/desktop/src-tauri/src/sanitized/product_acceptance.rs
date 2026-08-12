use std::{
    collections::VecDeque,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Receiver,
    },
    time::Duration,
};

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::*;
use crate::providers::{
    CodingProvider, ProviderEnablementPolicy, ProviderObservation, ProviderObservationAdapter,
    ProviderObservationCoordinator,
};

const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);

struct FixedClock(OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

struct ScenarioEnablement {
    codex: AtomicBool,
    claude: AtomicBool,
}

impl ScenarioEnablement {
    fn set(&self, provider: CodingProvider, enabled: bool) {
        match provider {
            CodingProvider::Codex => self.codex.store(enabled, Ordering::Release),
            CodingProvider::Claude => self.claude.store(enabled, Ordering::Release),
        }
    }
}

impl ProviderEnablementPolicy for ScenarioEnablement {
    fn is_provider_enabled(&self, provider: CodingProvider) -> bool {
        match provider {
            CodingProvider::Codex => self.codex.load(Ordering::Acquire),
            CodingProvider::Claude => self.claude.load(Ordering::Acquire),
        }
    }
}

struct ScenarioAdapter {
    provider: CodingProvider,
    results: Mutex<VecDeque<Option<ProviderObservation>>>,
    next_gate: Mutex<Option<Arc<ScenarioRefreshGate>>>,
    manual_runs: AtomicUsize,
}

struct ScenarioRefreshGate {
    started: Barrier,
    release: Barrier,
}

impl ScenarioRefreshGate {
    fn for_two_providers() -> Self {
        Self {
            started: Barrier::new(3),
            release: Barrier::new(3),
        }
    }

    fn wait_until_started(&self) {
        self.started.wait();
    }

    fn release(&self) {
        self.release.wait();
    }
}

impl ScenarioAdapter {
    fn new(provider: CodingProvider) -> Self {
        Self {
            provider,
            results: Mutex::new(VecDeque::new()),
            next_gate: Mutex::new(None),
            manual_runs: AtomicUsize::new(0),
        }
    }

    fn push(&self, result: Option<ProviderObservation>) {
        self.results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(result);
    }

    fn block_next(&self, gate: Arc<ScenarioRefreshGate>) {
        *self
            .next_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
    }

    fn manual_runs(&self) -> usize {
        self.manual_runs.load(Ordering::Acquire)
    }
}

impl ProviderObservationAdapter for ScenarioAdapter {
    fn provider(&self) -> CodingProvider {
        self.provider
    }

    fn refresh(
        &self,
        _cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        attempt.remaining()?;
        if !attempt.is_manual() {
            return Ok(None);
        }
        self.manual_runs.fetch_add(1, Ordering::AcqRel);
        let result = self
            .results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .flatten();
        if let Some(gate) = self
            .next_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            gate.started.wait();
            gate.release.wait();
            attempt.remaining()?;
        }
        Ok(result)
    }
}

struct ProductScenario {
    core: NativeCore,
    notices: Receiver<RevisionNotice>,
    enablement: Arc<ScenarioEnablement>,
    codex: Arc<ScenarioAdapter>,
    claude: Arc<ScenarioAdapter>,
}

impl ProductScenario {
    fn unavailable(now: OffsetDateTime) -> Self {
        Self::with_state(unavailable_state_at(1, now), now)
    }

    fn with_state(state: SanitizedDesktopStateV3, now: OffsetDateTime) -> Self {
        let enablement = Arc::new(ScenarioEnablement {
            codex: AtomicBool::new(true),
            claude: AtomicBool::new(true),
        });
        let codex = Arc::new(ScenarioAdapter::new(CodingProvider::Codex));
        let claude = Arc::new(ScenarioAdapter::new(CodingProvider::Claude));
        let policy: Arc<dyn ProviderEnablementPolicy> = enablement.clone();
        let coordinator = Arc::new(ProviderObservationCoordinator::with_enablement(
            vec![codex.clone(), claude.clone()],
            policy.clone(),
        ));
        let core = NativeCore::with_components(
            state,
            ReadModelStore::Memory,
            Arc::new(FixedClock(now)),
            coordinator,
            policy,
        );
        let notices = core.revision_notices().expect("revision stream");
        Self {
            core,
            notices,
            enablement,
            codex,
            claude,
        }
    }

    fn panel(&self) -> SanitizedDesktopStateV3 {
        self.core.panel_state().expect("sanitized panel state")
    }

    fn refresh(
        &self,
        codex: Option<ProviderObservation>,
        claude: Option<ProviderObservation>,
    ) -> SanitizedDesktopStateV3 {
        self.codex.push(codex);
        self.claude.push(claude);
        self.core
            .request_refresh(RefreshSource::Manual)
            .expect("manual refresh request");
        self.expect_refresh_completion();
        self.panel()
    }

    fn set_enabled(&self, provider: CodingProvider, enabled: bool) -> SanitizedDesktopStateV3 {
        self.enablement.set(provider, enabled);
        self.core
            .provider_enablement_changed(provider, enabled)
            .expect("provider setting projection");
        self.expect_revision();
        self.panel()
    }

    fn set_enabled_and_request_refresh(
        &self,
        provider: CodingProvider,
        enabled: bool,
    ) -> SanitizedDesktopStateV3 {
        let panel = self.set_enabled(provider, enabled);
        self.core
            .request_provider_refresh()
            .expect("provider refresh request");
        panel
    }

    fn expect_revision(&self) {
        self.notices
            .recv_timeout(NOTICE_TIMEOUT)
            .expect("native revision before timeout");
    }

    fn expect_refresh_completion(&self) {
        self.expect_revision();
        self.core
            .wait_for_refresh_completion()
            .expect("native refresh completion");
        while self.notices.try_recv().is_ok() {}
    }

    fn wait_for_manual_runs(&self, expected: usize) {
        let deadline = std::time::Instant::now() + NOTICE_TIMEOUT;
        while self.codex.manual_runs() < expected || self.claude.manual_runs() < expected {
            assert!(
                std::time::Instant::now() < deadline,
                "provider refresh did not finish before timeout"
            );
            std::thread::yield_now();
        }
        while self
            .core
            .inner
            .coordinator
            .inbox
            .in_flight
            .load(Ordering::Acquire)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "native refresh did not become idle before timeout"
            );
            std::thread::yield_now();
        }
    }
}

fn test_time() -> OffsetDateTime {
    OffsetDateTime::parse("2026-08-08T12:00:00Z", &Rfc3339).expect("fixture timestamp")
}

fn unavailable_observation(provider: CodingProvider) -> ProviderObservation {
    ProviderObservation {
        quota: ProviderSnapshot::Unavailable {
            provider,
            quota_lanes: [],
        },
        usage: UsagePeriods {
            scan_status: UsageScanStatus::Indexing,
            today_scan_status: UsageScanStatus::Indexing,
            seven_day_scan_status: UsageScanStatus::Indexing,
            thirty_day_scan_status: UsageScanStatus::Indexing,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        },
        top_model_usage: None,
        correction: None,
    }
}

fn observed_total(
    provider: CodingProvider,
    now: OffsetDateTime,
    tokens: u64,
    cost: f64,
) -> UsageTotal {
    UsageTotal::Current {
        evidence_basis: match provider {
            CodingProvider::Codex => UsageEvidenceBasis::ProviderReported,
            CodingProvider::Claude => UsageEvidenceBasis::LocallyDerived,
        },
        coverage: UsageCoverage::Complete,
        observed_at: format_time(now),
        observed_tokens: tokens,
        api_equivalent_cost_usd: Some(cost),
        trend_percent: None,
        trend_previous_tokens: None,
        api_equivalent_cost_basis: Some(match provider {
            CodingProvider::Codex => "openai-fixture-v1".to_owned(),
            CodingProvider::Claude => "anthropic-fixture-v1".to_owned(),
        }),
        api_equivalent_cost_quality: Some(match provider {
            CodingProvider::Codex => ApiEquivalentCostQuality::Reconciled,
            CodingProvider::Claude => ApiEquivalentCostQuality::LocalOnly,
        }),
        api_equivalent_cost_coverage_percent: None,
    }
}

fn observed(
    provider: CodingProvider,
    now: OffsetDateTime,
    tokens: [u64; 3],
    costs: [f64; 3],
) -> ProviderObservation {
    ProviderObservation {
        quota: ProviderSnapshot::Current {
            provider,
            observed_at: format_time(now),
            quota_lanes: vec![QuotaLane {
                label: "Weekly limit".to_owned(),
                unit: "percent".to_owned(),
                allowance: Some(100.0),
                remaining: Some(match provider {
                    CodingProvider::Codex => 40.0,
                    CodingProvider::Claude => 60.0,
                }),
                reset_at: None,
            }],
        },
        usage: UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: observed_total(provider, now, tokens[0], costs[0]),
            seven_days: observed_total(provider, now, tokens[1], costs[1]),
            thirty_days: observed_total(provider, now, tokens[2], costs[2]),
        },
        top_model_usage: None,
        correction: None,
    }
}

fn assert_usage(usage: &UsagePeriods, tokens: [u64; 3], costs: [f64; 3]) {
    for (total, expected_tokens, expected_cost) in [
        (&usage.today, tokens[0], costs[0]),
        (&usage.seven_days, tokens[1], costs[1]),
        (&usage.thirty_days, tokens[2], costs[2]),
    ] {
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = total
        else {
            panic!("period usage must be current");
        };
        assert_eq!(*observed_tokens, expected_tokens);
        assert_eq!(*api_equivalent_cost_usd, Some(expected_cost));
    }
}

#[test]
fn observed_usage_and_provider_enablement_follow_the_product_flow() {
    let now = test_time();
    let scenario = ProductScenario::unavailable(now);

    let indexing = scenario.refresh(
        Some(unavailable_observation(CodingProvider::Codex)),
        Some(unavailable_observation(CodingProvider::Claude)),
    );
    assert_eq!(
        indexing.combined_usage.scan_status,
        UsageScanStatus::Indexing
    );
    assert!(matches!(
        indexing.combined_usage.today,
        UsageTotal::Unavailable
    ));

    let claude_only = scenario.refresh(
        None,
        Some(observed(
            CodingProvider::Claude,
            now,
            [60, 160, 600],
            [6.0, 16.0, 60.0],
        )),
    );
    assert_usage(
        &claude_only.combined_usage,
        [60, 160, 600],
        [6.0, 16.0, 60.0],
    );
    assert_eq!(
        claude_only.combined_usage.scan_status,
        UsageScanStatus::Complete
    );

    let both = scenario.refresh(
        Some(observed(
            CodingProvider::Codex,
            now,
            [40, 140, 400],
            [4.0, 14.0, 40.0],
        )),
        None,
    );
    assert_usage(&both.combined_usage, [100, 300, 1_000], [10.0, 30.0, 100.0]);

    let codex_only = scenario.set_enabled(CodingProvider::Claude, false);
    assert!(matches!(
        codex_only
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .today,
        UsageTotal::Unavailable
    ));
    assert_usage(
        &codex_only.combined_usage,
        [40, 140, 400],
        [4.0, 14.0, 40.0],
    );

    let reenabled = scenario.set_enabled(CodingProvider::Claude, true);
    let claude = reenabled
        .provider(CodingProvider::Claude)
        .expect("Claude panel row");
    assert_eq!(claude.usage.scan_status, UsageScanStatus::Complete);
    assert!(matches!(
        &claude.quota,
        ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            quota_lanes,
            ..
        } if quota_lanes.len() == 1 && quota_lanes[0].remaining == Some(60.0)
    ));
    assert_usage(&claude.usage, [60, 160, 600], [6.0, 16.0, 60.0]);
    assert_usage(
        &reenabled.combined_usage,
        [100, 300, 1_000],
        [10.0, 30.0, 100.0],
    );

    let refreshed = scenario.refresh(
        None,
        Some(observed(
            CodingProvider::Claude,
            now,
            [70, 170, 700],
            [7.0, 17.0, 70.0],
        )),
    );
    assert_usage(
        &refreshed.combined_usage,
        [110, 310, 1_100],
        [11.0, 31.0, 110.0],
    );
    assert_eq!(
        refreshed
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Complete
    );
}

#[test]
fn reenabling_a_provider_without_history_keeps_the_valid_peer_combined_value() {
    let now = test_time();
    let scenario = ProductScenario::unavailable(now);

    scenario.set_enabled(CodingProvider::Claude, false);
    let codex_only = scenario.refresh(
        Some(observed(
            CodingProvider::Codex,
            now,
            [40, 140, 400],
            [4.0, 14.0, 40.0],
        )),
        None,
    );
    assert_usage(
        &codex_only.combined_usage,
        [40, 140, 400],
        [4.0, 14.0, 40.0],
    );

    let reenabled = scenario.set_enabled(CodingProvider::Claude, true);
    let claude = reenabled
        .provider(CodingProvider::Claude)
        .expect("Claude panel row");
    assert_eq!(claude.usage.scan_status, UsageScanStatus::Indexing);
    assert!(matches!(claude.usage.today, UsageTotal::Unavailable));
    assert_eq!(
        reenabled.combined_usage.scan_status,
        UsageScanStatus::Complete
    );
    assert_eq!(
        reenabled.combined_usage.today_scan_status,
        UsageScanStatus::Complete
    );
    assert_usage(&reenabled.combined_usage, [40, 140, 400], [4.0, 14.0, 40.0]);
}

#[test]
fn unchanged_refresh_finishes_only_the_matching_no_history_reenable_wait() {
    let scenario = ProductScenario::unavailable(test_time());

    scenario.set_enabled(CodingProvider::Claude, false);
    let loading = scenario.set_enabled_and_request_refresh(CodingProvider::Claude, true);
    assert_eq!(
        loading
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );

    scenario.expect_refresh_completion();
    let settled = scenario.panel();
    assert_eq!(
        settled
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Unavailable
    );
}

#[test]
fn repeated_reenable_before_the_first_refresh_still_finishes_the_loading_state() {
    let scenario = ProductScenario::unavailable(test_time());

    scenario.set_enabled(CodingProvider::Claude, false);
    let first_enable = scenario.set_enabled(CodingProvider::Claude, true);
    assert_eq!(
        first_enable
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );
    scenario.set_enabled(CodingProvider::Claude, false);
    let second_enable = scenario.set_enabled_and_request_refresh(CodingProvider::Claude, true);
    assert_eq!(
        second_enable
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );

    scenario.expect_refresh_completion();
    assert_eq!(
        scenario
            .panel()
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Unavailable
    );
}

#[test]
fn a_provider_filtered_before_commit_keeps_its_first_observation_wait() {
    let scenario = ProductScenario::unavailable(test_time());
    scenario.set_enabled(CodingProvider::Claude, false);
    scenario.set_enabled(CodingProvider::Claude, true);
    let gate = Arc::new(ScenarioRefreshGate::for_two_providers());
    scenario.codex.block_next(gate.clone());
    scenario.claude.block_next(gate.clone());

    scenario
        .core
        .request_provider_refresh()
        .expect("blocked provider refresh request");
    gate.wait_until_started();
    scenario.enablement.set(CodingProvider::Claude, false);
    gate.release();
    scenario.wait_for_manual_runs(1);

    scenario.enablement.set(CodingProvider::Claude, true);
    assert_eq!(
        scenario
            .panel()
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );
}

#[test]
fn reenable_keeps_a_real_older_history_index_active_after_an_unchanged_refresh() {
    let now = test_time();
    let mut state = unavailable_state_at(1, now);
    let claude = state
        .provider_mut(CodingProvider::Claude)
        .expect("Claude panel row");
    claude.usage.scan_status = UsageScanStatus::Indexing;
    claude.usage.today_scan_status = UsageScanStatus::Complete;
    claude.usage.seven_day_scan_status = UsageScanStatus::Complete;
    claude.usage.thirty_day_scan_status = UsageScanStatus::Complete;
    state.refresh_combined_usage();
    let scenario = ProductScenario::with_state(state, now);

    scenario.set_enabled(CodingProvider::Claude, false);
    let reenabled = scenario.set_enabled(CodingProvider::Claude, true);
    assert_eq!(
        reenabled
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );

    scenario
        .core
        .request_provider_refresh()
        .expect("provider refresh request");
    scenario.wait_for_manual_runs(1);

    assert_eq!(
        scenario
            .panel()
            .provider(CodingProvider::Claude)
            .expect("Claude panel row")
            .usage
            .scan_status,
        UsageScanStatus::Indexing
    );
}

#[test]
fn enabling_both_providers_restores_both_contributions() {
    let now = test_time();
    let scenario = ProductScenario::unavailable(now);
    scenario.refresh(
        Some(observed(
            CodingProvider::Codex,
            now,
            [40, 140, 400],
            [4.0, 14.0, 40.0],
        )),
        Some(observed(
            CodingProvider::Claude,
            now,
            [60, 160, 600],
            [6.0, 16.0, 60.0],
        )),
    );

    scenario.set_enabled(CodingProvider::Codex, false);
    let disabled = scenario.set_enabled(CodingProvider::Claude, false);
    assert!(matches!(
        disabled.combined_usage.today,
        UsageTotal::Unavailable
    ));

    scenario.set_enabled(CodingProvider::Codex, true);
    let restored = scenario.set_enabled(CodingProvider::Claude, true);
    assert_usage(
        &restored.combined_usage,
        [100, 300, 1_000],
        [10.0, 30.0, 100.0],
    );
    for provider in &restored.providers {
        assert_eq!(provider.usage.scan_status, UsageScanStatus::Complete);
        assert!(matches!(provider.quota, ProviderSnapshot::Current { .. }));
    }

    let refreshed = scenario.refresh(
        Some(observed(
            CodingProvider::Codex,
            now,
            [45, 145, 405],
            [4.5, 14.5, 40.5],
        )),
        Some(observed(
            CodingProvider::Claude,
            now,
            [65, 165, 605],
            [6.5, 16.5, 60.5],
        )),
    );
    assert_usage(
        &refreshed.combined_usage,
        [110, 310, 1_010],
        [11.0, 31.0, 101.0],
    );
}

#[test]
fn provider_toggles_during_a_refresh_commit_both_follow_up_contributions_once() {
    let now = test_time();
    let scenario = ProductScenario::unavailable(now);
    scenario.refresh(
        Some(observed(
            CodingProvider::Codex,
            now,
            [40, 140, 400],
            [4.0, 14.0, 40.0],
        )),
        Some(observed(
            CodingProvider::Claude,
            now,
            [60, 160, 600],
            [6.0, 16.0, 60.0],
        )),
    );
    let codex_runs_before_race = scenario.codex.manual_runs();
    let claude_runs_before_race = scenario.claude.manual_runs();

    scenario.codex.push(Some(observed(
        CodingProvider::Codex,
        now,
        [41, 141, 401],
        [4.1, 14.1, 40.1],
    )));
    scenario.claude.push(Some(observed(
        CodingProvider::Claude,
        now,
        [61, 161, 601],
        [6.1, 16.1, 60.1],
    )));
    scenario.codex.push(Some(observed(
        CodingProvider::Codex,
        now,
        [45, 145, 405],
        [4.5, 14.5, 40.5],
    )));
    scenario.claude.push(Some(observed(
        CodingProvider::Claude,
        now,
        [65, 165, 605],
        [6.5, 16.5, 60.5],
    )));
    let gate = Arc::new(ScenarioRefreshGate::for_two_providers());
    scenario.codex.block_next(gate.clone());
    scenario.claude.block_next(gate.clone());

    scenario
        .core
        .request_provider_refresh()
        .expect("blocked provider refresh request");
    gate.wait_until_started();

    scenario.set_enabled_and_request_refresh(CodingProvider::Codex, false);
    scenario.set_enabled_and_request_refresh(CodingProvider::Claude, false);
    scenario.set_enabled_and_request_refresh(CodingProvider::Codex, true);
    let reenabled = scenario.set_enabled_and_request_refresh(CodingProvider::Claude, true);
    assert_usage(
        &reenabled.combined_usage,
        [100, 300, 1_000],
        [10.0, 30.0, 100.0],
    );

    gate.release();
    scenario.expect_refresh_completion();
    let refreshed = scenario.panel();

    assert_usage(
        &refreshed.combined_usage,
        [110, 310, 1_010],
        [11.0, 31.0, 101.0],
    );
    assert_eq!(scenario.codex.manual_runs(), codex_runs_before_race + 2);
    assert_eq!(scenario.claude.manual_runs(), claude_runs_before_race + 2);
}
