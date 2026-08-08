mod claude;
mod codex;
mod process;
mod registry;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crate::sanitized::{
    Clock, ProviderPresentation, ProviderSnapshot, RefreshAttempt, RefreshFailure, RefreshTrigger,
    SanitizedDesktopStateV3, SnapshotRefreshAdapter, TopModelUsage, UsagePeriods,
};
use time::OffsetDateTime;

pub use registry::{CodingProvider, ProviderPresenceStatus};
pub(crate) use registry::{PROVIDER_REGISTRY, detect_provider_presence, provider_descriptor};

pub(crate) trait ProviderEnablementPolicy: Send + Sync {
    fn is_provider_enabled(&self, provider: CodingProvider) -> bool;
}

struct AllProvidersEnabled;

impl ProviderEnablementPolicy for AllProvidersEnabled {
    fn is_provider_enabled(&self, _provider: CodingProvider) -> bool {
        true
    }
}

pub(crate) fn all_providers_enabled_policy() -> Arc<dyn ProviderEnablementPolicy> {
    Arc::new(AllProvidersEnabled)
}

/// Sanitized output from one deep provider adapter.
/// Provider-native models, token categories, paths, and parser data must stay
/// behind the adapter boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProviderObservation {
    pub(crate) quota: ProviderSnapshot,
    pub(crate) usage: UsagePeriods,
    pub(crate) top_model_usage: Option<TopModelUsage>,
}

pub(crate) fn normalized_model_display_name(canonical: &str) -> Option<String> {
    let mut parts = canonical.split('-').collect::<Vec<_>>();
    if parts.last().is_some_and(|part| {
        part.len() == 8 && part.bytes().all(|character| character.is_ascii_digit())
    }) {
        parts.pop();
    }
    let display = match parts.as_slice() {
        ["gpt", version, variants @ ..] => {
            let mut display = format!("GPT {version}");
            for variant in variants {
                display.push(' ');
                display.push_str(&title_ascii_word(variant)?);
            }
            display
        }
        ["claude", first, rest @ ..] if first.bytes().all(|value| value.is_ascii_digit()) => {
            let family_index = rest
                .iter()
                .position(|part| !part.bytes().all(|value| value.is_ascii_digit()))?;
            let mut version = vec![*first];
            version.extend_from_slice(&rest[..family_index]);
            let family = title_ascii_word(rest[family_index])?;
            format!("Claude {family} {}", version.join("."))
        }
        ["claude", family, version @ ..] if !version.is_empty() => {
            let family = title_ascii_word(family)?;
            if !version
                .iter()
                .all(|part| part.bytes().all(|value| value.is_ascii_digit()))
            {
                return None;
            }
            format!("Claude {family} {}", version.join("."))
        }
        _ => return None,
    };
    (display.len() <= 48).then_some(display)
}

fn title_ascii_word(value: &str) -> Option<String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|character| character.is_ascii_lowercase())
    {
        return None;
    }
    let mut characters = value.chars();
    let mut titled = characters.next()?.to_uppercase().to_string();
    titled.extend(characters);
    Some(titled)
}

pub(crate) fn select_top_model_usage(
    entries: impl IntoIterator<Item = (String, Option<String>, u64)>,
) -> Option<TopModelUsage> {
    let mut totals = BTreeMap::<String, (Option<String>, u64)>::new();
    for (grouping_key, model, tokens) in entries {
        let (stored_model, total) = totals.entry(grouping_key).or_insert((model.clone(), 0));
        if *stored_model != model {
            return None;
        }
        *total = total.checked_add(tokens)?;
    }
    totals
        .into_iter()
        .filter(|(_, (_, tokens))| *tokens > 0)
        .map(|(_, (model, observed_tokens))| TopModelUsage {
            model,
            observed_tokens,
        })
        .reduce(crate::sanitized::preferred_top_model)
}

/// A deep adapter for one coding provider.
///
/// An adapter can read private provider data. It can return only the sanitized
/// quota and usage contract.
pub(crate) trait ProviderObservationAdapter: Send + Sync {
    fn provider(&self) -> CodingProvider;

    fn install_refresh_trigger(&self, _trigger: RefreshTrigger) {}

    fn reset_after_cancellation(&self) {}

    fn refresh(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure>;
}

pub(crate) struct ProviderObservationCoordinator {
    adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
    processes: BTreeMap<CodingProvider, process::ProviderProcessSupervisor>,
    cancellation_generations: BTreeMap<CodingProvider, Arc<AtomicU64>>,
    provider_settings_generation: AtomicU64,
    pending_first_observations: Mutex<BTreeMap<CodingProvider, u64>>,
    completed_first_observations: Mutex<BTreeMap<u64, BTreeMap<CodingProvider, u64>>>,
    enablement: Arc<dyn ProviderEnablementPolicy>,
}

impl ProviderObservationCoordinator {
    #[cfg(test)]
    pub(crate) fn new(adapters: Vec<Arc<dyn ProviderObservationAdapter>>) -> Self {
        Self::with_shared_processes_and_enablement(
            adapters,
            process::ProviderProcessSupervisor::default(),
            all_providers_enabled_policy(),
        )
    }

    #[cfg(test)]
    fn with_processes(
        adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
        processes: process::ProviderProcessSupervisor,
    ) -> Self {
        Self::with_shared_processes_and_enablement(
            adapters,
            processes,
            all_providers_enabled_policy(),
        )
    }

    #[cfg(test)]
    pub(crate) fn with_enablement(
        adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        Self::with_shared_processes_and_enablement(
            adapters,
            process::ProviderProcessSupervisor::default(),
            enablement,
        )
    }

    #[cfg(test)]
    fn with_shared_processes_and_enablement(
        adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
        processes: process::ProviderProcessSupervisor,
        enablement: Arc<dyn ProviderEnablementPolicy>,
    ) -> Self {
        let processes = adapters
            .iter()
            .map(|adapter| (adapter.provider(), processes.clone()))
            .collect();
        Self::with_processes_and_enablement(adapters, processes, enablement)
    }

    fn with_processes_and_enablement(
        adapters: Vec<Arc<dyn ProviderObservationAdapter>>,
        processes: BTreeMap<CodingProvider, process::ProviderProcessSupervisor>,
        enablement: Arc<dyn ProviderEnablementPolicy>,
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
        let cancellation_generations = adapters
            .iter()
            .map(|adapter| (adapter.provider(), Arc::new(AtomicU64::new(0))))
            .collect();
        Self {
            adapters,
            processes,
            cancellation_generations,
            provider_settings_generation: AtomicU64::new(0),
            pending_first_observations: Mutex::new(BTreeMap::new()),
            completed_first_observations: Mutex::new(BTreeMap::new()),
            enablement,
        }
    }

    fn matches_pending_first_observation(
        &self,
        provider: CodingProvider,
        expected_observation_generation: Option<u64>,
        expected_settings_generation: u64,
        attempt: &RefreshAttempt,
    ) -> bool {
        let Some(expected_observation_generation) = expected_observation_generation else {
            return false;
        };
        if !attempt.provider_settings_are_current(expected_settings_generation)
            || self.provider_settings_generation.load(Ordering::Acquire)
                != expected_settings_generation
        {
            return false;
        }
        let pending = self
            .pending_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !attempt.provider_settings_are_current(expected_settings_generation)
            || self.provider_settings_generation.load(Ordering::Acquire)
                != expected_settings_generation
            || pending.get(&provider).copied() != Some(expected_observation_generation)
        {
            return false;
        }
        true
    }

    fn stage_completed_first_observations(
        &self,
        settings_generation: u64,
        attempt: &RefreshAttempt,
        mut completed: BTreeMap<CodingProvider, u64>,
    ) {
        if completed.is_empty()
            || !attempt.provider_settings_are_current(settings_generation)
            || self.provider_settings_generation.load(Ordering::Acquire) != settings_generation
        {
            return;
        }
        let pending = self
            .pending_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut staged = self
            .completed_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !attempt.provider_settings_are_current(settings_generation)
            || self.provider_settings_generation.load(Ordering::Acquire) != settings_generation
        {
            return;
        }
        completed.retain(|provider, generation| pending.get(provider) == Some(generation));
        if !completed.is_empty() {
            staged.insert(settings_generation, completed);
        }
    }

    fn normalize_registry(&self, state: &mut SanitizedDesktopStateV3) {
        state.providers = PROVIDER_REGISTRY
            .iter()
            .filter(|descriptor| self.enablement.is_provider_enabled(descriptor.provider))
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
    enablement: Arc<dyn ProviderEnablementPolicy>,
) -> ProviderObservationCoordinator {
    let codex_processes = process::ProviderProcessSupervisor::default();
    let claude_processes = process::ProviderProcessSupervisor::default();
    let codex: Arc<dyn ProviderObservationAdapter> =
        Arc::new(codex::CodexProviderObservationAdapter::production(
            Arc::clone(&clock),
            database_path.clone(),
            codex_processes.clone(),
        ));
    let claude: Arc<dyn ProviderObservationAdapter> =
        Arc::new(claude::ClaudeProviderObservationAdapter::production(
            clock,
            database_path,
            claude_processes.clone(),
        ));
    ProviderObservationCoordinator::with_processes_and_enablement(
        vec![codex, claude],
        BTreeMap::from([
            (CodingProvider::Codex, codex_processes),
            (CodingProvider::Claude, claude_processes),
        ]),
        enablement,
    )
}

pub(crate) fn debug_codex_usage_pass(
    database_path: &Path,
    codex_home: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    codex::debug_usage_pass(database_path, codex_home, now)
}

pub(crate) fn debug_claude_usage_pass(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    claude::debug_usage_report(database_path, config_root, probe_directory, now)
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
        let cancellation_generations = self
            .cancellation_generations
            .iter()
            .map(|(provider, generation)| {
                (
                    *provider,
                    (Arc::clone(generation), generation.load(Ordering::Acquire)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let expected_settings_generation =
            self.provider_settings_generation.load(Ordering::Acquire);
        let pending_first_observations = self
            .pending_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let previous = cached.clone();
        self.normalize_registry(&mut cached);

        let results = thread::scope(|scope| {
            self.adapters
                .iter()
                .filter_map(|adapter| {
                    let provider = adapter.provider();
                    if !self.enablement.is_provider_enabled(provider) {
                        debug_refresh_event(provider, "disabled");
                        return None;
                    }
                    let presentation = cached.provider(provider)?;
                    let (generation, expected_generation) =
                        cancellation_generations.get(&provider)?.clone();
                    let provider_attempt =
                        attempt.with_provider_cancellation(generation, expected_generation);
                    let worker_attempt = provider_attempt.clone();
                    debug_refresh_event(provider, "started");
                    Some((
                        provider,
                        provider_attempt,
                        pending_first_observations.get(&provider).copied(),
                        scope.spawn(move || adapter.refresh(presentation, &worker_attempt)),
                    ))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(
                    |(provider, provider_attempt, pending_first_observation, handle)| {
                        (
                            provider,
                            provider_attempt,
                            pending_first_observation,
                            handle.join(),
                        )
                    },
                )
                .collect::<Vec<_>>()
        });

        let mut completed_first_observations = BTreeMap::new();

        for (provider, provider_attempt, pending_first_observation, result) in results {
            if attempt.is_cancelled() {
                return Err(RefreshFailure::Cancelled);
            }
            if provider_attempt.is_cancelled() {
                debug_refresh_event(provider, "cancelled");
                continue;
            }
            let completed_first_observation =
                !matches!(&result, Ok(Err(RefreshFailure::Cancelled)))
                    && self.matches_pending_first_observation(
                        provider,
                        pending_first_observation,
                        expected_settings_generation,
                        attempt,
                    );
            if completed_first_observation && let Some(generation) = pending_first_observation {
                completed_first_observations.insert(provider, generation);
            }
            match result {
                Ok(Ok(Some(observation))) => {
                    if observation.quota.provider() != provider {
                        debug_refresh_failure(provider, "invalid_provider");
                        if completed_first_observation
                            && let Some(presentation) = cached.provider_mut(provider)
                        {
                            presentation.finish_first_observation_wait();
                        }
                        continue;
                    }
                    let Some(presentation) = cached.provider_mut(provider) else {
                        continue;
                    };
                    presentation.quota = observation.quota;
                    presentation.usage = observation.usage;
                    presentation.top_model_usage = observation.top_model_usage;
                    debug_refresh_event(provider, "completed");
                }
                Ok(Ok(None)) => {
                    if completed_first_observation
                        && let Some(presentation) = cached.provider_mut(provider)
                    {
                        presentation.finish_first_observation_wait();
                    }
                    debug_refresh_event(provider, "unchanged");
                }
                Ok(Err(RefreshFailure::Cancelled)) => return Err(RefreshFailure::Cancelled),
                Ok(Err(RefreshFailure::DeadlineExceeded)) => {
                    if completed_first_observation
                        && let Some(presentation) = cached.provider_mut(provider)
                    {
                        presentation.finish_first_observation_wait();
                    }
                    debug_refresh_failure(provider, "deadline_exceeded");
                }
                Ok(Err(RefreshFailure::SourceUnavailable)) => {
                    if completed_first_observation
                        && let Some(presentation) = cached.provider_mut(provider)
                    {
                        presentation.finish_first_observation_wait();
                    }
                    debug_refresh_failure(provider, "source_unavailable");
                }
                Err(_) => {
                    if completed_first_observation
                        && let Some(presentation) = cached.provider_mut(provider)
                    {
                        presentation.finish_first_observation_wait();
                    }
                    debug_refresh_failure(provider, "adapter_panicked");
                }
            }
        }

        attempt.remaining()?;
        self.stage_completed_first_observations(
            expected_settings_generation,
            attempt,
            completed_first_observations,
        );
        cached.refresh_combined_usage();
        Ok((cached != previous).then_some(cached))
    }

    fn note_provider_enablement_commit(
        &self,
        provider: CodingProvider,
        enabled: bool,
        waits_for_first_observation: bool,
        settings_generation: u64,
    ) {
        let mut pending = self
            .pending_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut completed = self
            .completed_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enabled && waits_for_first_observation {
            pending.insert(provider, settings_generation);
        } else {
            pending.remove(&provider);
        }
        completed.clear();
        self.provider_settings_generation
            .store(settings_generation, Ordering::Release);
    }

    fn completed_provider_refreshes(&self, settings_generation: u64) -> BTreeSet<CodingProvider> {
        self.completed_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&settings_generation)
            .map(|completed| completed.keys().copied().collect())
            .unwrap_or_default()
    }

    fn acknowledge_provider_refresh(
        &self,
        settings_generation: u64,
        providers: &BTreeSet<CodingProvider>,
    ) {
        let mut pending = self
            .pending_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut completed = self
            .completed_first_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(staged) = completed.get_mut(&settings_generation) else {
            return;
        };
        if self.provider_settings_generation.load(Ordering::Acquire) != settings_generation {
            return;
        }
        for provider in providers {
            let Some(generation) = staged.remove(provider) else {
                continue;
            };
            if pending.get(provider).copied() == Some(generation) {
                pending.remove(provider);
            }
        }
        if staged.is_empty() {
            completed.remove(&settings_generation);
        }
    }

    fn cancel_provider(&self, provider: CodingProvider) {
        if let Some(generation) = self.cancellation_generations.get(&provider) {
            generation.fetch_add(1, Ordering::AcqRel);
        }
        // Stop process I/O before provider cleanup. Codex cleanup can wait for
        // a session lock that an active refresh holds.
        if let Some(processes) = self.processes.get(&provider) {
            let summary = processes.cancel_active();
            debug_process_shutdown(summary.process_count, summary.deadline_count);
        }
        if let Some(adapter) = self
            .adapters
            .iter()
            .find(|adapter| adapter.provider() == provider)
        {
            adapter.reset_after_cancellation();
        }
    }

    fn shutdown(&self) {
        let summary = self.processes.values().fold(
            process::ShutdownSummary::default(),
            |mut summary, processes| {
                let provider = processes.shutdown_all();
                summary.process_count =
                    summary.process_count.saturating_add(provider.process_count);
                summary.deadline_count = summary
                    .deadline_count
                    .saturating_add(provider.deadline_count);
                summary
            },
        );
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
        sync::{
            Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration as StdDuration, Instant},
    };

    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::sanitized::{
        ApiEquivalentCostQuality, UsageCoverage, UsageEvidenceBasis, UsageScanStatus, UsageTotal,
        unavailable_state,
    };

    #[derive(Clone)]
    struct FixedAdapter {
        provider: CodingProvider,
        result: Result<Option<ProviderObservation>, RefreshFailure>,
    }

    struct MutableEnablement {
        claude_enabled: AtomicBool,
    }

    impl ProviderEnablementPolicy for MutableEnablement {
        fn is_provider_enabled(&self, provider: CodingProvider) -> bool {
            provider == CodingProvider::Codex || self.claude_enabled.load(Ordering::Acquire)
        }
    }

    struct CountingAdapter {
        inner: FixedAdapter,
        runs: AtomicUsize,
    }

    impl ProviderObservationAdapter for CountingAdapter {
        fn provider(&self) -> CodingProvider {
            self.inner.provider()
        }

        fn refresh(
            &self,
            cached: &ProviderPresentation,
            attempt: &RefreshAttempt,
        ) -> Result<Option<ProviderObservation>, RefreshFailure> {
            self.runs.fetch_add(1, Ordering::AcqRel);
            self.inner.refresh(cached, attempt)
        }
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

    struct CancellationAwareAdapter {
        provider: CodingProvider,
        started: Option<mpsc::SyncSender<()>>,
        saw_cancellation: AtomicBool,
        runs: AtomicUsize,
    }

    impl ProviderObservationAdapter for CancellationAwareAdapter {
        fn provider(&self) -> CodingProvider {
            self.provider
        }

        fn refresh(
            &self,
            _cached: &ProviderPresentation,
            attempt: &RefreshAttempt,
        ) -> Result<Option<ProviderObservation>, RefreshFailure> {
            let run = self.runs.fetch_add(1, Ordering::AcqRel);
            if self.provider == CodingProvider::Claude && run == 0 {
                self.started
                    .as_ref()
                    .expect("Claude start signal")
                    .send(())
                    .expect("Claude start receiver");
                let deadline = Instant::now() + StdDuration::from_millis(250);
                while Instant::now() < deadline {
                    if attempt.remaining() == Err(RefreshFailure::Cancelled) {
                        self.saw_cancellation.store(true, Ordering::Release);
                        break;
                    }
                    thread::yield_now();
                }
            }
            let tokens = match (self.provider, run) {
                (CodingProvider::Codex, _) => 42,
                (CodingProvider::Claude, 0) => 999,
                (CodingProvider::Claude, _) => 58,
            };
            Ok(Some(ProviderObservation {
                quota: ProviderSnapshot::Unavailable {
                    provider: self.provider,
                    quota_lanes: [],
                },
                usage: usage_with_tokens(tokens),
                top_model_usage: None,
            }))
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
    fn model_labels_and_rankings_are_sanitized_and_deterministic() {
        assert_eq!(
            normalized_model_display_name("gpt-5.6-sol").as_deref(),
            Some("GPT 5.6 Sol")
        );
        assert_eq!(
            normalized_model_display_name("claude-sonnet-4-5-20250929").as_deref(),
            Some("Claude Sonnet 4.5")
        );
        assert_eq!(
            normalized_model_display_name("claude-3-5-haiku-20241022").as_deref(),
            Some("Claude Haiku 3.5")
        );
        assert_eq!(normalized_model_display_name("PRIVATE MODEL"), None);

        let top = select_top_model_usage([
            ("GPT 5.6 Sol".to_owned(), Some("GPT 5.6 Sol".to_owned()), 40),
            ("GPT 5.6 Sol".to_owned(), Some("GPT 5.6 Sol".to_owned()), 20),
            (
                "Claude Sonnet 4.5".to_owned(),
                Some("Claude Sonnet 4.5".to_owned()),
                50,
            ),
        ])
        .unwrap();
        assert_eq!(top.model.as_deref(), Some("GPT 5.6 Sol"));
        assert_eq!(top.observed_tokens, 60);

        let unknown_tie = select_top_model_usage([
            ("GPT 5.6 Sol".to_owned(), Some("GPT 5.6 Sol".to_owned()), 60),
            ("private-model".to_owned(), None, 60),
        ])
        .unwrap();
        assert_eq!(unknown_tie.model, None);

        let distinct_unknowns = select_top_model_usage([
            ("GPT 5.6 Sol".to_owned(), Some("GPT 5.6 Sol".to_owned()), 60),
            ("private-model-a".to_owned(), None, 40),
            ("private-model-b".to_owned(), None, 40),
        ])
        .unwrap();
        assert_eq!(distinct_unknowns.model.as_deref(), Some("GPT 5.6 Sol"));
    }

    #[test]
    fn cancelling_one_provider_attempt_preserves_the_peer_and_allows_a_fresh_attempt() {
        let (started, receiver) = mpsc::sync_channel(1);
        let codex = Arc::new(CancellationAwareAdapter {
            provider: CodingProvider::Codex,
            started: None,
            saw_cancellation: AtomicBool::new(false),
            runs: AtomicUsize::new(0),
        });
        let claude = Arc::new(CancellationAwareAdapter {
            provider: CodingProvider::Claude,
            started: Some(started),
            saw_cancellation: AtomicBool::new(false),
            runs: AtomicUsize::new(0),
        });
        let coordinator = Arc::new(ProviderObservationCoordinator::new(vec![
            codex.clone(),
            claude.clone(),
        ]));
        let worker_coordinator = Arc::clone(&coordinator);
        let worker = thread::spawn(move || {
            worker_coordinator.refresh(unavailable_state(1), &RefreshAttempt::test())
        });
        receiver
            .recv_timeout(StdDuration::from_secs(1))
            .expect("Claude refresh must start");

        coordinator.cancel_provider(CodingProvider::Claude);
        let first = worker
            .join()
            .expect("provider refresh thread")
            .expect("peer refresh must continue")
            .expect("Codex must change the snapshot");

        assert!(claude.saw_cancellation.load(Ordering::Acquire));
        assert!(matches!(
            first
                .provider(CodingProvider::Claude)
                .expect("Claude presentation")
                .usage
                .today,
            UsageTotal::Unavailable
        ));
        assert!(matches!(
            first.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));

        let second = coordinator
            .refresh(first, &RefreshAttempt::test())
            .expect("fresh provider attempt")
            .expect("Claude must add fresh evidence");
        assert!(matches!(
            second
                .provider(CodingProvider::Claude)
                .expect("Claude presentation")
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 58,
                ..
            }
        ));
        assert!(matches!(
            second.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 100,
                ..
            }
        ));
        assert_eq!(codex.runs.load(Ordering::Acquire), 2);
        assert_eq!(claude.runs.load(Ordering::Acquire), 2);
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
                top_model_usage: None,
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

    #[test]
    fn unchanged_refresh_finishes_first_observation_wait() {
        let mut state = unavailable_state(1);
        state
            .provider_mut(CodingProvider::Claude)
            .unwrap()
            .usage
            .scan_status = UsageScanStatus::Indexing;
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Ok(None),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![claude]);
        coordinator.note_provider_enablement_commit(CodingProvider::Claude, true, true, 1);

        let refreshed = coordinator
            .refresh(state, &RefreshAttempt::test())
            .unwrap()
            .expect("first observation wait must finish in the snapshot");

        let claude = refreshed.provider(CodingProvider::Claude).unwrap();
        assert_eq!(claude.usage.scan_status, UsageScanStatus::Unavailable);
        assert!(matches!(
            &claude.quota,
            ProviderSnapshot::Unavailable { quota_lanes, .. } if quota_lanes.is_empty()
        ));
        assert!(matches!(&claude.usage.today, UsageTotal::Unavailable));
    }

    #[test]
    fn unchanged_refresh_preserves_real_indexing_with_cached_observed_usage() {
        let mut state = unavailable_state(1);
        let claude = state.provider_mut(CodingProvider::Claude).unwrap();
        claude.usage = usage_with_tokens(58);
        claude.usage.scan_status = UsageScanStatus::Indexing;
        state.refresh_combined_usage();
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Ok(None),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![claude]);

        let refreshed = coordinator.refresh(state, &RefreshAttempt::test()).unwrap();

        assert!(refreshed.is_none());
    }

    #[test]
    fn unchanged_refresh_preserves_real_indexing_for_older_pending_history() {
        let mut state = unavailable_state(1);
        let claude = state.provider_mut(CodingProvider::Claude).unwrap();
        claude.usage.scan_status = UsageScanStatus::Indexing;
        claude.usage.today_scan_status = UsageScanStatus::Complete;
        claude.usage.seven_day_scan_status = UsageScanStatus::Complete;
        claude.usage.thirty_day_scan_status = UsageScanStatus::Complete;
        state.refresh_combined_usage();
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Ok(None),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![claude]);

        let refreshed = coordinator.refresh(state, &RefreshAttempt::test()).unwrap();

        assert!(refreshed.is_none());
    }

    #[test]
    fn newer_provider_setting_keeps_a_stale_attempt_from_finishing_the_wait() {
        let mut state = unavailable_state(1);
        state
            .provider_mut(CodingProvider::Claude)
            .unwrap()
            .usage
            .scan_status = UsageScanStatus::Indexing;
        state.refresh_combined_usage();
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Ok(None),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![claude]);
        coordinator.note_provider_enablement_commit(CodingProvider::Claude, true, true, 1);
        let settings_generation = Arc::new(AtomicU64::new(1));
        let stale_attempt = RefreshAttempt::test()
            .with_provider_settings_generation(Arc::clone(&settings_generation), 1);
        settings_generation.store(2, Ordering::Release);

        let stale = coordinator.refresh(state.clone(), &stale_attempt).unwrap();

        assert!(stale.is_none());
        coordinator.note_provider_enablement_commit(CodingProvider::Codex, false, false, 2);
        let current_attempt = RefreshAttempt::test()
            .with_provider_settings_generation(Arc::clone(&settings_generation), 2);
        let refreshed = coordinator
            .refresh(state, &current_attempt)
            .unwrap()
            .expect("current provider setting must finish the wait");
        coordinator.acknowledge_provider_refresh(2, &BTreeSet::from([CodingProvider::Claude]));
        assert_eq!(
            refreshed
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Unavailable
        );
    }

    #[test]
    fn provider_setting_between_refresh_and_commit_keeps_the_wait_pending() {
        let mut state = unavailable_state(1);
        state
            .provider_mut(CodingProvider::Claude)
            .unwrap()
            .usage
            .scan_status = UsageScanStatus::Indexing;
        state.refresh_combined_usage();
        let claude = Arc::new(FixedAdapter {
            provider: CodingProvider::Claude,
            result: Ok(None),
        });
        let coordinator = ProviderObservationCoordinator::new(vec![claude]);
        coordinator.note_provider_enablement_commit(CodingProvider::Claude, true, true, 1);
        let settings_generation = Arc::new(AtomicU64::new(1));
        let first_attempt = RefreshAttempt::test()
            .with_provider_settings_generation(Arc::clone(&settings_generation), 1);

        let first = coordinator
            .refresh(state.clone(), &first_attempt)
            .unwrap()
            .expect("first provider refresh must settle the wait candidate");
        assert_eq!(
            first
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .scan_status,
            UsageScanStatus::Unavailable
        );
        assert_eq!(
            coordinator
                .pending_first_observations
                .lock()
                .unwrap()
                .get(&CodingProvider::Claude)
                .copied(),
            Some(1)
        );
        assert_eq!(
            coordinator
                .completed_first_observations
                .lock()
                .unwrap()
                .get(&1)
                .and_then(|providers| providers.get(&CodingProvider::Claude))
                .copied(),
            Some(1)
        );

        settings_generation.store(2, Ordering::Release);
        coordinator.note_provider_enablement_commit(CodingProvider::Codex, false, false, 2);
        coordinator.acknowledge_provider_refresh(1, &BTreeSet::from([CodingProvider::Claude]));
        assert_eq!(
            coordinator
                .pending_first_observations
                .lock()
                .unwrap()
                .get(&CodingProvider::Claude)
                .copied(),
            Some(1)
        );

        let current_attempt = RefreshAttempt::test()
            .with_provider_settings_generation(Arc::clone(&settings_generation), 2);
        coordinator
            .refresh(state.clone(), &current_attempt)
            .unwrap()
            .expect("current provider refresh must settle the wait candidate");
        coordinator.acknowledge_provider_refresh(2, &BTreeSet::from([CodingProvider::Claude]));
        assert!(
            !coordinator
                .pending_first_observations
                .lock()
                .unwrap()
                .contains_key(&CodingProvider::Claude)
        );

        assert!(
            coordinator
                .refresh(state, &current_attempt)
                .unwrap()
                .is_none()
        );
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

    #[test]
    fn combined_usage_adds_codex_and_claude_tokens_and_cost() {
        let mut codex_usage = usage_with_tokens(42);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut codex_usage.today
        else {
            panic!("the fixture must have current usage");
        };
        *api_equivalent_cost_usd = Some(4.2);
        *api_equivalent_cost_basis = Some("openai-fixture".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::Reconciled);
        let mut claude_usage = usage_with_tokens(58);
        let UsageTotal::Current {
            evidence_basis,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &mut claude_usage.today
        else {
            panic!("the fixture must have current usage");
        };
        *evidence_basis = UsageEvidenceBasis::LocallyDerived;
        *api_equivalent_cost_usd = Some(5.8);
        *api_equivalent_cost_basis = Some("anthropic-fixture".to_owned());
        *api_equivalent_cost_quality = Some(ApiEquivalentCostQuality::LocalOnly);
        let adapter = |provider, usage| {
            Arc::new(FixedAdapter {
                provider,
                result: Ok(Some(ProviderObservation {
                    quota: ProviderSnapshot::Unavailable {
                        provider,
                        quota_lanes: [],
                    },
                    usage,
                    top_model_usage: None,
                })),
            })
        };
        let coordinator = ProviderObservationCoordinator::new(vec![
            adapter(CodingProvider::Codex, codex_usage),
            adapter(CodingProvider::Claude, claude_usage),
        ]);

        let refreshed = coordinator
            .refresh(unavailable_state(1), &RefreshAttempt::test())
            .unwrap()
            .expect("both provider results must change the snapshot");

        assert!(matches!(
            &refreshed
                .provider(CodingProvider::Claude)
                .unwrap()
                .usage
                .today,
            UsageTotal::Current {
                observed_tokens: 58,
                ..
            }
        ));
        assert!(matches!(
            &refreshed.combined_usage.today,
            UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::Mixed,
                observed_tokens: 100,
                ..
            }
        ));
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = &refreshed.combined_usage.today
        else {
            panic!("combined usage must be available");
        };
        assert!((api_equivalent_cost_usd.unwrap() - 10.0).abs() < f64::EPSILON);
        assert_eq!(
            *api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::LocalOnly)
        );
        assert_eq!(
            api_equivalent_cost_basis.as_deref(),
            Some("anthropic-fixture + openai-fixture")
        );
    }

    #[test]
    fn disabled_provider_is_not_refreshed_and_enabling_restores_its_adapter() {
        let policy = Arc::new(MutableEnablement {
            claude_enabled: AtomicBool::new(false),
        });
        let adapter = |provider, tokens| {
            Arc::new(CountingAdapter {
                inner: FixedAdapter {
                    provider,
                    result: Ok(Some(ProviderObservation {
                        quota: ProviderSnapshot::Unavailable {
                            provider,
                            quota_lanes: [],
                        },
                        usage: usage_with_tokens(tokens),
                        top_model_usage: Some(TopModelUsage {
                            model: Some(match provider {
                                CodingProvider::Codex => "GPT 5.6 Sol".to_owned(),
                                CodingProvider::Claude => "Claude Sonnet 4.5".to_owned(),
                            }),
                            observed_tokens: tokens,
                        }),
                    })),
                },
                runs: AtomicUsize::new(0),
            })
        };
        let codex = adapter(CodingProvider::Codex, 42);
        let claude = adapter(CodingProvider::Claude, 58);
        let coordinator = ProviderObservationCoordinator::with_enablement(
            vec![codex.clone(), claude.clone()],
            policy.clone(),
        );

        let codex_only = coordinator
            .refresh(unavailable_state(1), &RefreshAttempt::test())
            .unwrap()
            .unwrap();
        assert_eq!(codex.runs.load(Ordering::Acquire), 1);
        assert_eq!(claude.runs.load(Ordering::Acquire), 0);
        assert!(matches!(
            codex_only.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 42,
                ..
            }
        ));
        assert_eq!(
            codex_only
                .top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("GPT 5.6 Sol")
        );

        policy.claude_enabled.store(true, Ordering::Release);
        let both = coordinator
            .refresh(codex_only, &RefreshAttempt::test())
            .unwrap()
            .unwrap();
        assert_eq!(claude.runs.load(Ordering::Acquire), 1);
        assert!(matches!(
            both.combined_usage.today,
            UsageTotal::Current {
                observed_tokens: 100,
                ..
            }
        ));
        assert_eq!(
            both.top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("Claude Sonnet 4.5")
        );
    }
}
