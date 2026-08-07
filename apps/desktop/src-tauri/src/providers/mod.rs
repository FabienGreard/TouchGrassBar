mod claude;
mod codex;
mod registry;

use std::{collections::BTreeSet, path::Path, sync::Arc, thread};

use crate::sanitized::{
    Clock, ProviderPresentation, ProviderSnapshot, RefreshAttempt, RefreshFailure, RefreshTrigger,
    SanitizedDesktopStateV3, SnapshotRefreshAdapter, UsagePeriods,
};
use time::OffsetDateTime;

pub use registry::{CodingProvider, ProviderPresenceStatus};
pub(crate) use registry::{PROVIDER_REGISTRY, detect_provider_presence, provider_descriptor};

/// Sanitized output from one deep provider adapter.
/// Provider-native models, token categories, paths, and parser data must stay
/// behind the adapter boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProviderObservation {
    pub(crate) quota: ProviderSnapshot,
    pub(crate) usage: UsagePeriods,
}

/// A deep adapter for one coding provider.
///
/// An adapter can read private provider data. It can return only the sanitized
/// quota and usage contract.
pub(crate) trait ProviderObservationAdapter: Send + Sync {
    fn provider(&self) -> CodingProvider;

    fn install_refresh_trigger(&self, _trigger: RefreshTrigger) {}

    fn refresh(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure>;
}

pub(crate) struct ProviderObservationCoordinator {
    adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
}

impl ProviderObservationCoordinator {
    pub(crate) fn new(adapters: Vec<Arc<dyn ProviderObservationAdapter>>) -> Self {
        debug_assert_eq!(
            adapters
                .iter()
                .map(|adapter| adapter.provider())
                .collect::<BTreeSet<_>>()
                .len(),
            adapters.len(),
            "each provider can have only one observation adapter"
        );
        Self { adapters }
    }

    fn normalize_registry(&self, state: &mut SanitizedDesktopStateV3) {
        state.providers = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| {
                let mut presentation = state
                    .provider(descriptor.provider)
                    .cloned()
                    .unwrap_or_else(|| ProviderPresentation::unavailable(descriptor.provider));
                presentation.display_name = descriptor.display_name.to_owned();
                presentation.presence = detect_provider_presence(descriptor.provider);
                presentation
            })
            .collect();
    }
}

pub(crate) fn production_observation_coordinator(
    clock: Arc<dyn Clock>,
    database_path: Option<std::path::PathBuf>,
) -> ProviderObservationCoordinator {
    let codex: Arc<dyn ProviderObservationAdapter> =
        Arc::new(codex::CodexProviderObservationAdapter::production(
            Arc::clone(&clock),
            database_path.clone(),
        ));
    let claude: Arc<dyn ProviderObservationAdapter> = Arc::new(
        claude::ClaudeProviderObservationAdapter::production(clock, database_path),
    );
    ProviderObservationCoordinator::new(vec![codex, claude])
}

pub(crate) fn run_claude_status_line_from_args() -> Option<i32> {
    claude::run_status_line_from_args()
}

#[cfg(debug_assertions)]
pub(crate) fn report_claude_status_line_setup() {
    eprintln!("[TouchGrassBar][claude-quota] bridge_setup_skipped reason=debug_build");
}

#[cfg(not(debug_assertions))]
pub(crate) fn configure_claude_status_line(database_path: &Path) -> Result<(), ()> {
    if detect_provider_presence(CodingProvider::Claude) != ProviderPresenceStatus::Detected {
        eprintln!(
            "[TouchGrassBar][claude-quota] bridge_setup_skipped reason=provider_not_detected"
        );
        return Ok(());
    }
    let result = claude::configure_production_status_line(database_path);
    match result {
        Ok(()) => eprintln!("[TouchGrassBar][claude-quota] bridge_setup_completed"),
        Err(()) => eprintln!("[TouchGrassBar][claude-quota] bridge_setup_failed"),
    }
    result
}

pub(crate) fn debug_codex_usage_pass(
    database_path: &Path,
    codex_home: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    codex::debug_usage_pass(database_path, codex_home, now)
}

pub(crate) fn seed_claude_debug_fixture(
    database_path: &Path,
    now: OffsetDateTime,
) -> Result<(), ()> {
    claude::seed_debug_fixture(database_path, now)
}

pub(crate) fn debug_claude_quota_pass(
    database_path: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    claude::debug_quota_report(database_path, now)
}

#[cfg(test)]
pub(crate) fn test_claude_observation_coordinator(
    clock: Arc<dyn Clock>,
    database_path: std::path::PathBuf,
) -> ProviderObservationCoordinator {
    let claude: Arc<dyn ProviderObservationAdapter> = Arc::new(
        claude::ClaudeProviderObservationAdapter::production(clock, Some(database_path)),
    );
    ProviderObservationCoordinator::new(vec![claude])
}

impl SnapshotRefreshAdapter for ProviderObservationCoordinator {
    fn install_refresh_trigger(&self, trigger: RefreshTrigger) {
        for adapter in &self.adapters {
            adapter.install_refresh_trigger(Arc::clone(&trigger));
        }
    }

    fn refresh(
        &self,
        mut cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
    ) -> Result<Option<SanitizedDesktopStateV3>, RefreshFailure> {
        attempt.remaining()?;
        let previous = cached.clone();
        self.normalize_registry(&mut cached);

        let results = thread::scope(|scope| {
            self.adapters
                .iter()
                .filter_map(|adapter| {
                    let provider = adapter.provider();
                    let presentation = cached.provider(provider)?;
                    let attempt = attempt.clone();
                    debug_refresh_event(provider, "started");
                    Some((
                        provider,
                        scope.spawn(move || adapter.refresh(presentation, &attempt)),
                    ))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(provider, handle)| (provider, handle.join()))
                .collect::<Vec<_>>()
        });

        for (provider, result) in results {
            match result {
                Ok(Ok(Some(observation))) => {
                    if observation.quota.provider() != provider {
                        debug_refresh_failure(provider, "invalid_provider");
                        continue;
                    }
                    let Some(presentation) = cached.provider_mut(provider) else {
                        continue;
                    };
                    presentation.quota = observation.quota;
                    presentation.usage = observation.usage;
                    debug_refresh_event(provider, "completed");
                }
                Ok(Ok(None)) => debug_refresh_event(provider, "unchanged"),
                Ok(Err(RefreshFailure::Cancelled)) => return Err(RefreshFailure::Cancelled),
                Ok(Err(RefreshFailure::DeadlineExceeded)) => {
                    debug_refresh_failure(provider, "deadline_exceeded");
                }
                Ok(Err(RefreshFailure::SourceUnavailable)) => {
                    debug_refresh_failure(provider, "source_unavailable");
                }
                Err(_) => debug_refresh_failure(provider, "adapter_panicked"),
            }
        }

        attempt.remaining()?;
        cached.refresh_combined_usage();
        Ok((cached != previous).then_some(cached))
    }
}

#[cfg(debug_assertions)]
fn debug_refresh_failure(provider: CodingProvider, reason: &str) {
    let provider = provider_descriptor(provider).display_name.to_lowercase();
    eprintln!(
        "[TouchGrassBar][provider-observation] refresh_failed provider={provider} reason={reason}"
    );
}

#[cfg(not(debug_assertions))]
fn debug_refresh_failure(_provider: CodingProvider, _reason: &str) {}

#[cfg(debug_assertions)]
fn debug_refresh_event(provider: CodingProvider, status: &str) {
    let provider = provider_descriptor(provider).display_name.to_lowercase();
    eprintln!("[TouchGrassBar][provider-observation] refresh_{status} provider={provider}");
}

#[cfg(not(debug_assertions))]
fn debug_refresh_event(_provider: CodingProvider, _status: &str) {}

#[cfg(test)]
mod tests {
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::sanitized::{
        UsageCoverage, UsageEvidenceBasis, UsageScanStatus, UsageTotal, unavailable_state,
    };

    #[derive(Clone)]
    struct FixedAdapter {
        provider: CodingProvider,
        result: Result<Option<ProviderObservation>, RefreshFailure>,
    }

    impl ProviderObservationAdapter for FixedAdapter {
        fn provider(&self) -> CodingProvider {
            self.provider
        }

        fn refresh(
            &self,
            _cached: &ProviderPresentation,
            _attempt: &RefreshAttempt,
        ) -> Result<Option<ProviderObservation>, RefreshFailure> {
            self.result.clone()
        }
    }

    fn usage_with_tokens(tokens: u64) -> UsagePeriods {
        let observed_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at,
                observed_tokens: tokens,
                api_equivalent_cost_usd: None,
                trend_percent: None,
                trend_previous_tokens: None,
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        }
    }

    #[test]
    fn one_provider_failure_does_not_discard_another_provider_result() {
        let codex = Arc::new(FixedAdapter {
            provider: CodingProvider::Codex,
            result: Ok(Some(ProviderObservation {
                quota: ProviderSnapshot::Unavailable {
                    provider: CodingProvider::Codex,
                    quota_lanes: [],
                },
                usage: usage_with_tokens(42),
            })),
        });
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Err(RefreshFailure::SourceUnavailable),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![codex, claude]);

        let refreshed = coordinator
            .refresh(unavailable_state(1), &RefreshAttempt::test())
            .unwrap()
            .expect("Codex result must change the snapshot");

        assert!(matches!(
            &refreshed
                .provider(CodingProvider::Codex)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        assert!(matches!(
            &refreshed
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .today,
            UsageTotal::Unavailable
        ));
        assert!(matches!(
            &refreshed.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
    }
}
