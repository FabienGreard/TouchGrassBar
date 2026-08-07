mod claude;
mod codex;
mod process;
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
    processes: process::ProviderProcessSupervisor,
}

impl ProviderObservationCoordinator {
    #[cfg(test)]
    pub(crate) fn new(adapters: Vec<Arc<dyn ProviderObservationAdapter>>) -> Self {
        Self::with_processes(adapters, process::ProviderProcessSupervisor::default())
    }

    fn with_processes(
        adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
        processes: process::ProviderProcessSupervisor,
    ) -> Self {
        debug_assert_eq!(
            adapters
                .iter()
                .map(|adapter| adapter.provider())
                .collect::<BTreeSet<_>>()
                .len(),
            adapters.len(),
            "each provider can have only one observation adapter"
        );
        Self {
            adapters,
            processes,
        }
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
    let processes = process::ProviderProcessSupervisor::default();
    let codex: Arc<dyn ProviderObservationAdapter> =
        Arc::new(codex::CodexProviderObservationAdapter::production(
            Arc::clone(&clock),
            database_path.clone(),
            processes.clone(),
        ));
    let claude: Arc<dyn ProviderObservationAdapter> =
        Arc::new(claude::ClaudeProviderObservationAdapter::production(
            clock,
            database_path,
            processes.clone(),
        ));
    ProviderObservationCoordinator::with_processes(vec![codex, claude], processes)
}

pub(crate) fn debug_codex_usage_pass(
    database_path: &Path,
    codex_home: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    codex::debug_usage_pass(database_path, codex_home, now)
}

pub(crate) fn debug_live_claude_quota_pass(
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    claude::debug_live_quota_report(probe_directory, now)
}

#[cfg(test)]
pub(crate) fn test_claude_observation_coordinator(
    clock: Arc<dyn Clock>,
) -> ProviderObservationCoordinator {
    let observation = claude::fixture_observation(clock.now());
    let processes = process::ProviderProcessSupervisor::default();
    let claude: Arc<dyn ProviderObservationAdapter> = Arc::new(
        claude::ClaudeProviderObservationAdapter::fixture(clock, observation, processes.clone()),
    );
    ProviderObservationCoordinator::with_processes(vec![claude], processes)
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

    fn shutdown(&self) {
        let summary = self.processes.shutdown_all();
        debug_process_shutdown(summary.process_count, summary.deadline_count);
    }
}

#[cfg(debug_assertions)]
fn debug_process_shutdown(process_count: usize, deadline_count: usize) {
    eprintln!(
        "[TouchGrassBar][provider-observation] process_shutdown process_count={process_count} deadline_count={deadline_count}"
    );
}

#[cfg(not(debug_assertions))]
fn debug_process_shutdown(_process_count: usize, _deadline_count: usize) {}

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
struct DescendantHeldOutputAdapter {
    processes: process::ProviderProcessSupervisor,
    ready: std::sync::mpsc::SyncSender<(libc::pid_t, libc::pid_t)>,
}

#[cfg(test)]
impl ProviderObservationAdapter for DescendantHeldOutputAdapter {
    fn provider(&self) -> CodingProvider {
        CodingProvider::Codex
    }

    fn refresh(
        &self,
        _cached: &ProviderPresentation,
        _attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        let mut command = process::ProviderCommand::new("/bin/sh");
        command.args([
            "-c",
            "/bin/sh -c 'while [ \"$PPID\" -ne 1 ]; do sleep 0.01; done; printf \"orphaned\\n\"; sleep 30' & printf '%s %s\\n' \"$$\" \"$!\"; exit 0",
        ]);
        let child = self
            .processes
            .spawn_piped(
                command,
                process::ProviderOutputMode::Lines {
                    max_line_bytes: 1024,
                    max_buffered_bytes: 4096,
                },
                None,
            )
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        let pids = child
            .receive_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        let pids = std::str::from_utf8(&pids)
            .ok()
            .and_then(|pids| {
                let mut pids = pids.split_whitespace();
                Some((pids.next()?.parse().ok()?, pids.next()?.parse().ok()?))
            })
            .ok_or(RefreshFailure::SourceUnavailable)?;
        let orphaned = child
            .receive_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        if orphaned.as_slice() != b"orphaned" || self.ready.send(pids).is_err() {
            return Err(RefreshFailure::SourceUnavailable);
        }
        match child.receive_timeout(std::time::Duration::from_secs(30)) {
            Err(process::ProviderProcessError::Cancelled) => Err(RefreshFailure::Cancelled),
            _ => Err(RefreshFailure::SourceUnavailable),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_descendant_held_output_refresh_adapter() -> (
    Arc<dyn SnapshotRefreshAdapter>,
    std::sync::mpsc::Receiver<(libc::pid_t, libc::pid_t)>,
) {
    let processes = process::ProviderProcessSupervisor::default();
    let (ready, receiver) = std::sync::mpsc::sync_channel(1);
    let adapter: Arc<dyn ProviderObservationAdapter> = Arc::new(DescendantHeldOutputAdapter {
        processes: processes.clone(),
        ready,
    });
    (
        Arc::new(ProviderObservationCoordinator::with_processes(
            vec![adapter],
            processes,
        )),
        receiver,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Barrier, mpsc},
        time::Duration as StdDuration,
    };

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

    struct BlockingProcessAdapter {
        processes: process::ProviderProcessSupervisor,
        started: Barrier,
    }

    impl ProviderObservationAdapter for BlockingProcessAdapter {
        fn provider(&self) -> CodingProvider {
            CodingProvider::Codex
        }

        fn refresh(
            &self,
            _cached: &ProviderPresentation,
            _attempt: &RefreshAttempt,
        ) -> Result<Option<ProviderObservation>, RefreshFailure> {
            let mut command = process::ProviderCommand::new("/bin/sh");
            command.args(["-c", "sleep 30"]);
            let child = self
                .processes
                .spawn_piped(
                    command,
                    process::ProviderOutputMode::Lines {
                        max_line_bytes: 1024,
                        max_buffered_bytes: 4096,
                    },
                    None,
                )
                .map_err(|_| RefreshFailure::SourceUnavailable)?;
            self.started.wait();
            match child.receive_timeout(StdDuration::from_secs(30)) {
                Err(process::ProviderProcessError::Cancelled) => Err(RefreshFailure::Cancelled),
                _ => Err(RefreshFailure::SourceUnavailable),
            }
        }
    }

    #[test]
    fn coordinator_shutdown_unblocks_a_provider_process_read() {
        let processes = process::ProviderProcessSupervisor::default();
        let adapter = Arc::new(BlockingProcessAdapter {
            processes: processes.clone(),
            started: Barrier::new(2),
        });
        let coordinator = Arc::new(ProviderObservationCoordinator::with_processes(
            vec![adapter.clone()],
            processes,
        ));
        let worker_coordinator = Arc::clone(&coordinator);
        let (complete, completed) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = worker_coordinator.refresh(unavailable_state(1), &RefreshAttempt::test());
            let _ = complete.send(result);
        });
        adapter.started.wait();

        coordinator.shutdown();
        let result = completed
            .recv_timeout(StdDuration::from_secs(2))
            .expect("provider refresh must stop within the shutdown budget");
        worker.join().unwrap();

        assert_eq!(result, Err(RefreshFailure::Cancelled));
    }
}
