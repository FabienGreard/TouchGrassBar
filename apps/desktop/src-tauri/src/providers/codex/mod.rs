mod usage;

pub(crate) use usage::{
    USAGE_INDEX_SCHEMA_MODULE, USAGE_INDEX_SCHEMA_VERSION,
    prepare_database as prepare_usage_database, usage_index_schema_version,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};

use serde::Deserialize;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use self::usage::{
    AccountUsageObservation, CachedAccountUsageObservation, load_cached_account_usage,
    parse_account_usage, project_usage_periods_with_account_time, scan_local_usage,
    store_cached_account_usage,
};
use super::{ProviderObservation, ProviderObservationAdapter};
use crate::daily_usage_aggregate::preserve_best_known_costs;
use crate::providers::process::{
    ProviderCommand, ProviderOutputMode, ProviderProcess, ProviderProcessError,
    ProviderProcessSupervisor,
};
use crate::sanitized::{
    Clock, CodingProvider, ProviderPresentation, ProviderSnapshot, QuotaLane, RefreshAttempt,
    RefreshFailure, RefreshTrigger, TopModelUsage, UsagePeriods,
};

const INITIALIZE_REQUEST_ID: i64 = 1;
const DEFAULT_LIMIT_ID: &str = "codex";
const IGNORED_CODEX_LIMIT_NAME: &str = "GPT-5.3-Codex-Spark";
const ACCOUNT_USAGE_REFRESH_MINUTES: i64 = 30;
const MAX_APP_SERVER_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_APP_SERVER_BUFFERED_BYTES: usize = 4 * MAX_APP_SERVER_MESSAGE_BYTES;
const PROCESS_POLL_INTERVAL: StdDuration = StdDuration::from_millis(100);

pub(super) fn debug_usage_pass(
    database_path: &Path,
    codex_home: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    usage::debug_usage_pass(database_path, codex_home, now)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct UsedPercent(u8);

impl UsedPercent {
    fn parse(value: i32) -> Result<Self, ()> {
        u8::try_from(value)
            .ok()
            .filter(|value| *value <= 100)
            .map(Self)
            .ok_or(())
    }

    fn remaining(self) -> f64 {
        f64::from(100 - self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct UnixResetAt(i64);

impl UnixResetAt {
    fn parse(value: i64) -> Result<Self, ()> {
        (value > 0).then_some(Self(value)).ok_or(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowDurationMinutes(i64);

impl WindowDurationMinutes {
    fn parse(value: i64) -> Result<Self, ()> {
        (value > 0).then_some(Self(value)).ok_or(())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RateLimitWindow {
    reset_at: UnixResetAt,
    used_percent: UsedPercent,
    duration: WindowDurationMinutes,
}

#[derive(Clone, Debug, PartialEq)]
struct RateLimitBucket {
    name: Option<String>,
    primary: Option<RateLimitWindow>,
    secondary: Option<RateLimitWindow>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexQuotaObservation {
    buckets: BTreeMap<String, RateLimitBucket>,
    ignored_limit_ids: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FullRateLimitsResponse {
    rate_limits: RawRateLimitSnapshot,
    #[serde(default)]
    rate_limits_by_limit_id: Option<BTreeMap<String, RawRateLimitSnapshot>>,
    #[serde(default)]
    rate_limit_reset_credits: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SparseRateLimitsNotification {
    rate_limits: RawRateLimitSnapshot,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRateLimitSnapshot {
    #[serde(default)]
    credits: Option<Value>,
    #[serde(default)]
    individual_limit: Option<Value>,
    #[serde(default)]
    limit_id: Option<String>,
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    plan_type: Option<Value>,
    #[serde(default)]
    primary: Option<RawRateLimitWindow>,
    #[serde(default)]
    rate_limit_reached_type: Option<Value>,
    #[serde(default)]
    secondary: Option<RawRateLimitWindow>,
    #[serde(default)]
    spend_control_reached: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRateLimitWindow {
    #[serde(default)]
    resets_at: Option<i64>,
    used_percent: i32,
    #[serde(default)]
    window_duration_mins: Option<i64>,
}

impl CodexQuotaObservation {
    fn from_full_read(payload: &str) -> Result<Self, ()> {
        let response: FullRateLimitsResponse = serde_json::from_str(payload).map_err(|_| ())?;
        let _ = response.rate_limit_reset_credits;
        let snapshots = match response.rate_limits_by_limit_id {
            Some(snapshots) => snapshots,
            None => BTreeMap::from([(DEFAULT_LIMIT_ID.to_owned(), response.rate_limits)]),
        };
        let mut buckets = BTreeMap::new();
        let mut ignored_limit_ids = BTreeSet::new();
        for (limit_id, snapshot) in snapshots {
            if snapshot
                .limit_id
                .as_deref()
                .is_some_and(|id| id != limit_id)
            {
                return Err(());
            }
            if snapshot
                .limit_name
                .as_deref()
                .is_some_and(is_ignored_codex_limit)
            {
                debug_event("quota_ignored class=codex_spark");
                ignored_limit_ids.insert(limit_id);
                continue;
            }
            if let Some(bucket) = complete_bucket(snapshot)? {
                buckets.insert(limit_id, bucket);
            }
        }
        if buckets.is_empty() && ignored_limit_ids.is_empty() {
            return Err(());
        }
        Ok(Self {
            buckets,
            ignored_limit_ids,
        })
    }

    fn merge_sparse(&mut self, payload: &str) -> Result<bool, ()> {
        let notification: SparseRateLimitsNotification =
            serde_json::from_str(payload).map_err(|_| ())?;
        validate_sparse_snapshot(&notification.rate_limits)?;
        let limit_id = notification
            .rate_limits
            .limit_id
            .clone()
            .unwrap_or_else(|| DEFAULT_LIMIT_ID.to_owned());
        if notification
            .rate_limits
            .limit_name
            .as_deref()
            .is_some_and(is_ignored_codex_limit)
        {
            self.ignored_limit_ids.insert(limit_id);
            debug_event("quota_update_ignored class=codex_spark");
            return Ok(false);
        }
        if self.ignored_limit_ids.contains(&limit_id) {
            debug_event("quota_update_ignored class=codex_spark");
            return Ok(false);
        }
        let current = self.buckets.get_mut(&limit_id).ok_or(())?;
        let mut changed = false;
        if let Some(name) = notification.rate_limits.limit_name {
            current.name = nonempty_name(name)?;
            changed = true;
        }
        if let Some(primary) = notification.rate_limits.primary {
            changed |= update_window(&mut current.primary, primary)?;
        }
        if let Some(secondary) = notification.rate_limits.secondary {
            changed |= update_window(&mut current.secondary, secondary)?;
        }
        Ok(changed)
    }

    fn sanitized_snapshot(&self, observed_at: OffsetDateTime) -> Result<ProviderSnapshot, ()> {
        let mut quota_lanes = Vec::new();
        for (limit_id, bucket) in self.ordered_buckets() {
            append_bucket_lanes(&mut quota_lanes, limit_id, bucket)?;
        }
        if quota_lanes.is_empty() {
            return Ok(ProviderSnapshot::Unavailable {
                provider: CodingProvider::Codex,
                quota_lanes: [],
            });
        }
        Ok(ProviderSnapshot::Current {
            provider: CodingProvider::Codex,
            observed_at: format_time(observed_at)?,
            quota_lanes,
        })
    }

    fn ordered_buckets(&self) -> impl Iterator<Item = (&str, &RateLimitBucket)> {
        self.buckets
            .get(DEFAULT_LIMIT_ID)
            .map(|bucket| (DEFAULT_LIMIT_ID, bucket))
            .into_iter()
            .chain(
                self.buckets
                    .iter()
                    .filter(|(limit_id, _)| limit_id.as_str() != DEFAULT_LIMIT_ID)
                    .map(|(limit_id, bucket)| (limit_id.as_str(), bucket)),
            )
    }
}

fn complete_bucket(snapshot: RawRateLimitSnapshot) -> Result<Option<RateLimitBucket>, ()> {
    let primary = snapshot.primary.map(complete_window).transpose()?;
    let secondary = snapshot.secondary.map(complete_window).transpose()?;
    if primary.is_none() && secondary.is_none() {
        return Ok(None);
    }
    let name = snapshot
        .limit_name
        .map(nonempty_name)
        .transpose()?
        .flatten();
    let _ = (
        snapshot.credits,
        snapshot.individual_limit,
        snapshot.plan_type,
        snapshot.rate_limit_reached_type,
        snapshot.spend_control_reached,
    );
    Ok(Some(RateLimitBucket {
        name,
        primary,
        secondary,
    }))
}

fn validate_sparse_snapshot(snapshot: &RawRateLimitSnapshot) -> Result<(), ()> {
    if snapshot.limit_id.as_deref().is_some_and(str::is_empty) {
        return Err(());
    }
    if let Some(name) = &snapshot.limit_name {
        nonempty_name(name.clone())?;
    }
    for window in [&snapshot.primary, &snapshot.secondary]
        .into_iter()
        .flatten()
    {
        UsedPercent::parse(window.used_percent)?;
        if let Some(reset_at) = window.resets_at {
            UnixResetAt::parse(reset_at)?;
        }
        if let Some(duration) = window.window_duration_mins {
            WindowDurationMinutes::parse(duration)?;
        }
    }
    Ok(())
}

fn nonempty_name(name: String) -> Result<Option<String>, ()> {
    let name = normalized_limit_name(&name);
    let name = name.chars().take(64).collect::<String>();
    (!name.is_empty()).then_some(Some(name)).ok_or(())
}

fn normalized_limit_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_ignored_codex_limit(name: &str) -> bool {
    normalized_limit_name(name) == IGNORED_CODEX_LIMIT_NAME
}

fn complete_window(window: RawRateLimitWindow) -> Result<RateLimitWindow, ()> {
    Ok(RateLimitWindow {
        reset_at: UnixResetAt::parse(window.resets_at.ok_or(())?)?,
        used_percent: UsedPercent::parse(window.used_percent)?,
        duration: WindowDurationMinutes::parse(window.window_duration_mins.ok_or(())?)?,
    })
}

fn merge_window(current: &mut RateLimitWindow, update: RawRateLimitWindow) -> Result<(), ()> {
    current.used_percent = UsedPercent::parse(update.used_percent)?;
    if let Some(reset_at) = update.resets_at {
        current.reset_at = UnixResetAt::parse(reset_at)?;
    }
    if let Some(duration) = update.window_duration_mins {
        current.duration = WindowDurationMinutes::parse(duration)?;
    }
    Ok(())
}

fn update_window(
    current: &mut Option<RateLimitWindow>,
    update: RawRateLimitWindow,
) -> Result<bool, ()> {
    if let Some(current) = current {
        merge_window(current, update)?;
        return Ok(true);
    }
    if update.resets_at.is_none() || update.window_duration_mins.is_none() {
        return Ok(false);
    }
    *current = Some(complete_window(update)?);
    Ok(true)
}

fn window_label(duration: WindowDurationMinutes) -> String {
    match duration.0 {
        300 => "5-hour limit".to_owned(),
        10_080 => "Weekly limit".to_owned(),
        minutes if minutes % 1_440 == 0 => format!("{}-day limit", minutes / 1_440),
        minutes if minutes % 60 == 0 => format!("{}-hour limit", minutes / 60),
        minutes => format!("{minutes}-minute limit"),
    }
}

fn append_bucket_lanes(
    lanes: &mut Vec<QuotaLane>,
    limit_id: &str,
    bucket: &RateLimitBucket,
) -> Result<(), ()> {
    for window in [&bucket.primary, &bucket.secondary].into_iter().flatten() {
        lanes.push(sanitized_lane(
            window,
            sanitized_lane_label(limit_id, bucket, window),
        )?);
    }
    Ok(())
}

fn sanitized_lane_label(
    limit_id: &str,
    bucket: &RateLimitBucket,
    window: &RateLimitWindow,
) -> String {
    let window_label = window_label(window.duration);
    if limit_id == DEFAULT_LIMIT_ID {
        window_label
    } else {
        format!(
            "{} {window_label}",
            bucket.name.as_deref().unwrap_or("Additional")
        )
    }
}

fn sanitized_lane(window: &RateLimitWindow, label: String) -> Result<QuotaLane, ()> {
    let reset_at = OffsetDateTime::from_unix_timestamp(window.reset_at.0).map_err(|_| ())?;
    // The app-server contract supplies used percent. A percent lane therefore
    // has an allowance of 100 and the exact complement as its remaining value.
    Ok(QuotaLane {
        label,
        unit: "percent".to_owned(),
        allowance: Some(100.0),
        remaining: Some(window.used_percent.remaining()),
        reset_at: Some(format_time(reset_at)?),
    })
}

fn format_time(now: OffsetDateTime) -> Result<String, ()> {
    now.format(&Rfc3339).map_err(|_| ())
}

#[cfg(debug_assertions)]
fn debug_event(event: &str) {
    eprintln!("[TouchGrassBar][codex-quota] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_event(_event: &str) {}

#[cfg(debug_assertions)]
fn debug_usage_event(event: &str) {
    eprintln!("[TouchGrassBar][codex-usage] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_usage_event(_event: &str) {}

#[cfg(debug_assertions)]
fn debug_observation_summary(observation: &CodexQuotaObservation) -> String {
    let mut labels = Vec::new();
    for (limit_id, bucket) in observation.ordered_buckets() {
        append_debug_labels(&mut labels, limit_id, bucket);
    }
    let lanes = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| format!("lane-{}={label:?}", index + 1))
        .collect::<Vec<_>>();
    format!("lane_count={} {}", lanes.len(), lanes.join(" "))
}

#[cfg(debug_assertions)]
fn append_debug_labels(labels: &mut Vec<String>, limit_id: &str, bucket: &RateLimitBucket) {
    labels.extend(
        [&bucket.primary, &bucket.secondary]
            .into_iter()
            .flatten()
            .map(|window| sanitized_lane_label(limit_id, bucket, window)),
    );
}

#[cfg(debug_assertions)]
fn debug_observation(observation: &CodexQuotaObservation) {
    eprintln!(
        "[TouchGrassBar][codex-quota] full_read_received {}",
        debug_observation_summary(observation)
    );
}

#[cfg(not(debug_assertions))]
fn debug_observation(_observation: &CodexQuotaObservation) {}

pub(crate) struct CodexProviderObservationAdapter {
    clock: Arc<dyn Clock>,
    database_path: Option<PathBuf>,
    processes: ProviderProcessSupervisor,
    session: Mutex<Option<CodexAppServerSession>>,
    refresh_trigger: Mutex<Option<RefreshTrigger>>,
}

trait AccountUsageReader {
    fn read_account_usage(
        &mut self,
        attempt: &RefreshAttempt,
    ) -> Result<AccountUsageObservation, RefreshFailure>;
}

impl CodexProviderObservationAdapter {
    pub(crate) fn production(
        clock: Arc<dyn Clock>,
        database_path: Option<PathBuf>,
        processes: ProviderProcessSupervisor,
    ) -> Self {
        Self {
            clock,
            database_path,
            processes,
            session: Mutex::new(None),
            refresh_trigger: Mutex::new(None),
        }
    }

    fn refresh_account_usage<R: AccountUsageReader>(
        &self,
        session: &mut R,
        attempt: &RefreshAttempt,
    ) -> Option<CachedAccountUsageObservation> {
        let now = self.clock.now();
        let cached = load_cached_account_usage(self.database_path.as_deref());
        let refresh_reason =
            account_usage_refresh_reason(cached.as_ref(), now, attempt.is_manual());
        let Some(refresh_reason) = refresh_reason else {
            let age_seconds = cached
                .as_ref()
                .map(|cached| (now - cached.observed_at).whole_seconds().max(0))
                .unwrap_or(0);
            debug_usage_event(&format!("account_cache_hit age_seconds={age_seconds}"));
            return cached;
        };

        debug_usage_event(&format!("account_refresh_started reason={refresh_reason}"));
        match session.read_account_usage(attempt) {
            Ok(observation) => {
                let observed_at = self.clock.now();
                let stored = store_cached_account_usage(
                    self.database_path.as_deref(),
                    &observation,
                    observed_at,
                )
                .is_ok();
                debug_usage_event(&format!(
                    "account_refresh_completed days={} cache_stored={stored}",
                    observation.day_count()
                ));
                Some(CachedAccountUsageObservation {
                    observation,
                    observed_at,
                })
            }
            Err(_) => {
                debug_usage_event(&format!(
                    "account_refresh_failed fallback_cached={}",
                    cached.is_some()
                ));
                cached
            }
        }
    }

    fn update_usage_projection(
        &self,
        usage: &mut UsagePeriods,
        top_model_usage: &mut Option<TopModelUsage>,
        account: Option<&CachedAccountUsageObservation>,
        observed_at: OffsetDateTime,
    ) -> bool {
        let local_usage = scan_local_usage(self.database_path.as_deref(), observed_at);
        if account.is_none() && local_usage.is_none() {
            debug_usage_event("projection_preserved reason=no_evidence");
            return false;
        }
        let projected = project_usage_periods_with_account_time(
            account.map(|cached| &cached.observation),
            local_usage.as_ref(),
            observed_at,
            account.map_or(observed_at, |cached| cached.observed_at),
        );
        let previous = usage.clone();
        *usage = preserve_best_known_costs(projected, &previous);
        if let Some(local_usage) = local_usage {
            *top_model_usage = local_usage.top_model_usage;
        }
        let published = &*usage;
        let cost_available = |total: &crate::sanitized::UsageTotal| {
            matches!(
                total,
                crate::sanitized::UsageTotal::Current {
                    api_equivalent_cost_usd: Some(_),
                    ..
                } | crate::sanitized::UsageTotal::Stale {
                    api_equivalent_cost_usd: Some(_),
                    ..
                }
            )
        };
        debug_usage_event(&format!(
            "projection_updated scan={:?} today_scan={:?} seven_day_scan={:?} thirty_day_scan={:?} account_cached={} today_cost={} seven_day_cost={} thirty_day_cost={}",
            published.scan_status,
            published.today_scan_status,
            published.seven_day_scan_status,
            published.thirty_day_scan_status,
            account.is_some(),
            cost_available(&published.today),
            cost_available(&published.seven_days),
            cost_available(&published.thirty_days)
        ));
        true
    }
}

fn account_usage_refresh_reason(
    cached: Option<&CachedAccountUsageObservation>,
    now: OffsetDateTime,
    manual: bool,
) -> Option<&'static str> {
    if manual {
        return Some("manual");
    }
    let Some(cached) = cached else {
        return Some("missing");
    };
    if cached.observed_at > now
        || now - cached.observed_at >= Duration::minutes(ACCOUNT_USAGE_REFRESH_MINUTES)
    {
        return Some("expired");
    }
    None
}

impl ProviderObservationAdapter for CodexProviderObservationAdapter {
    fn provider(&self) -> CodingProvider {
        CodingProvider::Codex
    }

    fn reset_after_cancellation(&self) {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn install_refresh_trigger(&self, trigger: RefreshTrigger) {
        *self
            .refresh_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(trigger);
    }

    fn refresh(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        let mut provider_observation = ProviderObservation {
            quota: cached.quota.clone(),
            usage: cached.usage.clone(),
            top_model_usage: cached.top_model_usage.clone(),
        };
        if attempt.is_local_usage_only() {
            let observed_at = self.clock.now();
            let account = load_cached_account_usage(self.database_path.as_deref());
            attempt.remaining()?;
            self.update_usage_projection(
                &mut provider_observation.usage,
                &mut provider_observation.top_model_usage,
                account.as_ref(),
                observed_at,
            );
            attempt.remaining()?;
            return Ok(Some(provider_observation));
        }
        debug_event("refresh_started");
        let mut session_guard = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session_guard.is_none() {
            let Some(executable) = resolve_codex_executable() else {
                debug_event("refresh_failed stage=executable_not_found");
                return Err(RefreshFailure::SourceUnavailable);
            };
            let trigger = self
                .refresh_trigger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let session =
                CodexAppServerSession::start(&self.processes, &executable, attempt, trigger)
                    .inspect_err(|_| {
                        debug_event("refresh_failed stage=session_start");
                    })?;
            *session_guard = Some(session);
        }
        let result = session_guard
            .as_mut()
            .ok_or(RefreshFailure::SourceUnavailable)?
            .read_observation(attempt);
        let observation = match result {
            Ok(observation) => observation,
            Err(_error) => {
                debug_event("refresh_failed stage=full_read");
                // Quota and usage are independent app-server capabilities.
                // A quota failure must not prevent a due or manual account
                // usage refresh from using the still-live session.
                let account = self.refresh_account_usage(
                    session_guard
                        .as_mut()
                        .ok_or(RefreshFailure::SourceUnavailable)?,
                    attempt,
                );
                session_guard.take();
                let observed_at = self.clock.now();
                attempt.remaining()?;
                self.update_usage_projection(
                    &mut provider_observation.usage,
                    &mut provider_observation.top_model_usage,
                    account.as_ref(),
                    observed_at,
                );
                attempt.remaining()?;
                debug_event("usage_projection_completed source=cached_account_or_local");
                return Ok(Some(provider_observation));
            }
        };
        let quota = observation
            .sanitized_snapshot(self.clock.now())
            .map_err(|_| {
                debug_event("refresh_failed stage=sanitized_projection");
                RefreshFailure::SourceUnavailable
            })?;
        provider_observation.quota = quota;
        let account_usage = self.refresh_account_usage(
            session_guard
                .as_mut()
                .ok_or(RefreshFailure::SourceUnavailable)?,
            attempt,
        );
        let observed_at = self.clock.now();
        attempt.remaining()?;
        if self.update_usage_projection(
            &mut provider_observation.usage,
            &mut provider_observation.top_model_usage,
            account_usage.as_ref(),
            observed_at,
        ) {
            debug_event("usage_projection_completed");
        } else {
            debug_event("usage_projection_preserved");
        }
        debug_event("refresh_completed");
        Ok(Some(provider_observation))
    }
}

struct CodexAppServerSession {
    process: ProviderProcess,
    next_request_id: i64,
    observation: Option<CodexQuotaObservation>,
}

impl CodexAppServerSession {
    fn start(
        processes: &ProviderProcessSupervisor,
        executable: &Path,
        attempt: &RefreshAttempt,
        trigger: Option<RefreshTrigger>,
    ) -> Result<Self, RefreshFailure> {
        attempt.remaining()?;
        let mut command = ProviderCommand::new(executable);
        command.arg("app-server");
        if let Some(parent) = executable.parent() {
            let mut paths = vec![parent.to_path_buf()];
            if let Some(current) = env::var_os("PATH") {
                paths.extend(env::split_paths(&current));
            }
            if let Ok(path) = env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        let observer = trigger.map(|trigger| {
            Arc::new(move |line: &[u8]| {
                if serde_json::from_slice::<Value>(line)
                    .ok()
                    .and_then(|message| {
                        message
                            .get("method")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    == Some("account/rateLimits/updated".to_owned())
                {
                    debug_event("provider_notification_received");
                    trigger();
                }
            }) as Arc<dyn Fn(&[u8]) + Send + Sync>
        });
        let process = processes
            .spawn_piped(
                command,
                ProviderOutputMode::Lines {
                    max_line_bytes: MAX_APP_SERVER_MESSAGE_BYTES,
                    max_buffered_bytes: MAX_APP_SERVER_BUFFERED_BYTES,
                },
                observer,
            )
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        let mut session = Self {
            process,
            next_request_id: INITIALIZE_REQUEST_ID + 1,
            observation: None,
        };
        session.send(
            json!({
                "method": "initialize",
                "id": INITIALIZE_REQUEST_ID,
                "params": {
                    "clientInfo": {
                        "name": "touchgrassbar",
                        "title": "TouchGrassBar",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
            attempt,
        )?;
        session.wait_for_response(INITIALIZE_REQUEST_ID, attempt)?;
        session.send(json!({"method": "initialized", "params": {}}), attempt)?;
        debug_event("session_initialized");
        Ok(session)
    }

    fn read_observation(
        &mut self,
        attempt: &RefreshAttempt,
    ) -> Result<CodexQuotaObservation, RefreshFailure> {
        let _ = self.drain_sparse_notifications()?;
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.send(
            json!({
                "method": "account/rateLimits/read",
                "id": request_id,
                "params": null
            }),
            attempt,
        )?;
        loop {
            let message = self.receive(attempt)?;
            if is_sparse_notification(&message) {
                self.merge_notification(&message)?;
                continue;
            }
            if message.get("id").and_then(Value::as_i64) != Some(request_id) {
                continue;
            }
            if message.get("error").is_some() {
                return Err(RefreshFailure::SourceUnavailable);
            }
            let payload = message
                .get("result")
                .ok_or(RefreshFailure::SourceUnavailable)?;
            let payload =
                serde_json::to_string(payload).map_err(|_| RefreshFailure::SourceUnavailable)?;
            let observation = CodexQuotaObservation::from_full_read(&payload)
                .map_err(|_| RefreshFailure::SourceUnavailable)?;
            debug_observation(&observation);
            self.observation = Some(observation.clone());
            return Ok(observation);
        }
    }

    fn drain_sparse_notifications(&mut self) -> Result<bool, RefreshFailure> {
        let mut updated = false;
        loop {
            match self.process.try_receive() {
                Ok(Some(line)) => {
                    let message: Value = serde_json::from_slice(&line)
                        .map_err(|_| RefreshFailure::SourceUnavailable)?;
                    if is_sparse_notification(&message) {
                        updated |= self.merge_notification(&message)?;
                    }
                }
                Ok(None) => return Ok(updated),
                Err(_) => return Err(RefreshFailure::SourceUnavailable),
            }
        }
    }

    fn read_usage_observation(
        &mut self,
        attempt: &RefreshAttempt,
    ) -> Result<AccountUsageObservation, RefreshFailure> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.send(
            json!({
                "method": "account/usage/read",
                "id": request_id,
                "params": null
            }),
            attempt,
        )?;
        loop {
            let message = self.receive(attempt)?;
            if is_sparse_notification(&message) {
                self.merge_notification(&message)?;
                continue;
            }
            if message.get("id").and_then(Value::as_i64) != Some(request_id) {
                continue;
            }
            if message.get("error").is_some() {
                return Err(RefreshFailure::SourceUnavailable);
            }
            let payload = message
                .get("result")
                .ok_or(RefreshFailure::SourceUnavailable)?;
            let payload =
                serde_json::to_string(payload).map_err(|_| RefreshFailure::SourceUnavailable)?;
            return parse_account_usage(&payload).map_err(|_| RefreshFailure::SourceUnavailable);
        }
    }

    fn merge_notification(&mut self, message: &Value) -> Result<bool, RefreshFailure> {
        let params = message
            .get("params")
            .ok_or(RefreshFailure::SourceUnavailable)?;
        let payload =
            serde_json::to_string(params).map_err(|_| RefreshFailure::SourceUnavailable)?;
        let parsed: SparseRateLimitsNotification =
            serde_json::from_str(&payload).map_err(|_| RefreshFailure::SourceUnavailable)?;
        validate_sparse_snapshot(&parsed.rate_limits)
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        let Some(observation) = self.observation.as_mut() else {
            return Ok(false);
        };
        observation
            .merge_sparse(&payload)
            .map_err(|_| RefreshFailure::SourceUnavailable)
    }

    fn wait_for_response(
        &mut self,
        request_id: i64,
        attempt: &RefreshAttempt,
    ) -> Result<(), RefreshFailure> {
        loop {
            let message = self.receive(attempt)?;
            if message.get("id").and_then(Value::as_i64) != Some(request_id) {
                continue;
            }
            return (!message.get("error").is_some())
                .then_some(())
                .ok_or(RefreshFailure::SourceUnavailable);
        }
    }

    fn send(&self, message: Value, attempt: &RefreshAttempt) -> Result<(), RefreshFailure> {
        let mut encoded =
            serde_json::to_vec(&message).map_err(|_| RefreshFailure::SourceUnavailable)?;
        encoded.push(b'\n');
        self.process
            .write_all(&encoded, attempt.remaining()?)
            .map_err(map_process_error)
    }

    fn receive(&self, attempt: &RefreshAttempt) -> Result<Value, RefreshFailure> {
        loop {
            let remaining = attempt.remaining()?;
            match self
                .process
                .receive_timeout(remaining.min(PROCESS_POLL_INTERVAL))
            {
                Ok(line) => {
                    return serde_json::from_slice(&line)
                        .map_err(|_| RefreshFailure::SourceUnavailable);
                }
                Err(ProviderProcessError::TimedOut) => continue,
                Err(error) => return Err(map_process_error(error)),
            }
        }
    }
}

impl AccountUsageReader for CodexAppServerSession {
    fn read_account_usage(
        &mut self,
        attempt: &RefreshAttempt,
    ) -> Result<AccountUsageObservation, RefreshFailure> {
        self.read_usage_observation(attempt)
    }
}

fn map_process_error(error: ProviderProcessError) -> RefreshFailure {
    match error {
        ProviderProcessError::TimedOut => RefreshFailure::DeadlineExceeded,
        ProviderProcessError::Cancelled => RefreshFailure::Cancelled,
        ProviderProcessError::SupervisorStopping
        | ProviderProcessError::StartFailed
        | ProviderProcessError::InputUnavailable
        | ProviderProcessError::OutputClosed
        | ProviderProcessError::OutputLimit => RefreshFailure::SourceUnavailable,
    }
}

fn is_sparse_notification(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated")
}

fn resolve_codex_executable() -> Option<PathBuf> {
    fn push_candidate(
        candidates: &mut Vec<PathBuf>,
        seen: &mut BTreeSet<PathBuf>,
        candidate: PathBuf,
    ) {
        if seen.insert(candidate.clone()) {
            candidates.push(candidate);
        }
    }

    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            push_candidate(&mut candidates, &mut seen, directory.join("codex"));
        }
    }
    for directory in ["/opt/homebrew/bin", "/usr/local/bin"] {
        push_candidate(
            &mut candidates,
            &mut seen,
            Path::new(directory).join("codex"),
        );
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for directory in [".local/bin", ".bun/bin", ".npm-global/bin", ".volta/bin"] {
            push_candidate(
                &mut candidates,
                &mut seen,
                home.join(directory).join("codex"),
            );
        }
        if let Ok(versions) = fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions = versions.flatten().collect::<Vec<_>>();
            versions.sort_by_key(|version| {
                version
                    .file_name()
                    .to_string_lossy()
                    .trim_start_matches('v')
                    .split('.')
                    .map(|part| part.parse::<u64>().unwrap_or(0))
                    .collect::<Vec<_>>()
            });
            for version in versions.into_iter().rev() {
                push_candidate(&mut candidates, &mut seen, version.path().join("bin/codex"));
            }
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBSERVED_AT: &str = "2026-08-06T10:00:00Z";
    const PRIMARY_RESET: i64 = 1_786_020_000;
    const SECONDARY_RESET: i64 = 1_786_572_000;

    fn full_fixture() -> String {
        format!(
            r#"{{
              "rateLimits": {{
                "credits": null,
                "individualLimit": null,
                "limitId": "codex",
                "limitName": null,
                "planType": "pro",
                "primary": {{
                  "usedPercent": 26,
                  "windowDurationMins": 300,
                  "resetsAt": {PRIMARY_RESET}
                }},
                "rateLimitReachedType": null,
                "secondary": {{
                  "usedPercent": 82,
                  "windowDurationMins": 10080,
                  "resetsAt": {SECONDARY_RESET}
                }},
                "spendControlReached": false
              }},
              "rateLimitsByLimitId": null,
              "rateLimitResetCredits": null
            }}"#,
        )
    }

    fn weekly_only_fixture() -> String {
        format!(
            r#"{{
              "rateLimits": {{
                "limitId": "codex",
                "primary": {{
                  "usedPercent": 82,
                  "windowDurationMins": 10080,
                  "resetsAt": {SECONDARY_RESET}
                }},
                "secondary": null
              }},
              "rateLimitsByLimitId": {{
                "codex": {{
                  "limitId": "codex",
                  "primary": {{
                    "usedPercent": 82,
                    "windowDurationMins": 10080,
                    "resetsAt": {SECONDARY_RESET}
                  }},
                  "secondary": null
                }}
              }},
              "rateLimitResetCredits": null
            }}"#,
        )
    }

    fn observed_at() -> OffsetDateTime {
        OffsetDateTime::parse(OBSERVED_AT, &Rfc3339).expect("valid observation time")
    }

    #[test]
    fn account_usage_cache_refreshes_at_thirty_minutes_and_on_manual_sync() {
        let now = observed_at();
        let observation = parse_account_usage(
            r#"{"dailyUsageBuckets":[{"startDate":"2026-08-06","tokens":340}],"summary":{}}"#,
        )
        .unwrap();
        let fresh = CachedAccountUsageObservation {
            observation,
            observed_at: now - Duration::minutes(29),
        };
        let expired = CachedAccountUsageObservation {
            observed_at: now - Duration::minutes(30),
            ..fresh.clone()
        };

        assert_eq!(
            account_usage_refresh_reason(None, now, false),
            Some("missing")
        );
        assert_eq!(account_usage_refresh_reason(Some(&fresh), now, false), None);
        assert_eq!(
            account_usage_refresh_reason(Some(&fresh), now, true),
            Some("manual")
        );
        assert_eq!(
            account_usage_refresh_reason(Some(&expired), now, false),
            Some("expired")
        );
    }

    struct FixedClock(OffsetDateTime);

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    struct RecordingUsageReader {
        reads: usize,
        observation: AccountUsageObservation,
    }

    impl AccountUsageReader for RecordingUsageReader {
        fn read_account_usage(
            &mut self,
            _attempt: &RefreshAttempt,
        ) -> Result<AccountUsageObservation, RefreshFailure> {
            self.reads += 1;
            Ok(self.observation.clone())
        }
    }

    #[test]
    fn provider_cancellation_drops_the_retained_session_before_the_next_attempt() {
        let processes = ProviderProcessSupervisor::default();
        let mut command = ProviderCommand::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let process = processes
            .spawn_piped(
                command,
                ProviderOutputMode::Lines {
                    max_line_bytes: 1024,
                    max_buffered_bytes: 2048,
                },
                None,
            )
            .expect("retained app-server process");
        let adapter = Arc::new(CodexProviderObservationAdapter::production(
            Arc::new(FixedClock(observed_at())),
            None,
            processes.clone(),
        ));
        *adapter
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CodexAppServerSession {
            process,
            next_request_id: INITIALIZE_REQUEST_ID + 1,
            observation: None,
        });
        let coordinator = crate::providers::ProviderObservationCoordinator::with_processes(
            vec![adapter.clone()],
            processes,
        );

        crate::sanitized::SnapshotRefreshAdapter::cancel_provider(
            &coordinator,
            CodingProvider::Codex,
        );

        assert!(
            adapter
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "the next refresh must create a new app-server session"
        );
    }

    #[test]
    fn account_usage_read_remains_independent_after_a_quota_failure() {
        let now = observed_at();
        let adapter = CodexProviderObservationAdapter::production(
            Arc::new(FixedClock(now)),
            None,
            ProviderProcessSupervisor::default(),
        );
        let mut reader = RecordingUsageReader {
            reads: 0,
            observation: parse_account_usage(
                r#"{"dailyUsageBuckets":[{"startDate":"2026-08-06","tokens":340}],"summary":{}}"#,
            )
            .unwrap(),
        };

        // The quota branch calls this same independent capability before it
        // drops a failed quota session.
        let usage = adapter
            .refresh_account_usage(&mut reader, &RefreshAttempt::test())
            .expect("manual usage refresh must still return account usage");

        assert_eq!(reader.reads, 1);
        assert_eq!(usage.observation.day_count(), 1);
        assert_eq!(usage.observed_at, now);
    }

    #[test]
    fn full_read_initializes_only_sanitized_five_hour_and_weekly_lanes() {
        let observation = CodexQuotaObservation::from_full_read(&full_fixture()).unwrap();
        let snapshot = observation.sanitized_snapshot(observed_at()).unwrap();

        assert_eq!(
            snapshot,
            ProviderSnapshot::Current {
                provider: CodingProvider::Codex,
                observed_at: OBSERVED_AT.to_owned(),
                quota_lanes: vec![
                    QuotaLane {
                        label: "5-hour limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(74.0),
                        reset_at: Some("2026-08-06T12:40:00Z".to_owned()),
                    },
                    QuotaLane {
                        label: "Weekly limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(18.0),
                        reset_at: Some("2026-08-12T22:00:00Z".to_owned()),
                    },
                ],
            }
        );
    }

    #[test]
    fn full_read_accepts_the_provider_returned_lane_count_and_window_identity() {
        let snapshot = CodexQuotaObservation::from_full_read(&weekly_only_fixture())
            .unwrap()
            .sanitized_snapshot(observed_at())
            .unwrap();

        let ProviderSnapshot::Current { quota_lanes, .. } = snapshot else {
            panic!("expected current snapshot");
        };
        assert_eq!(quota_lanes.len(), 1);
        assert_eq!(quota_lanes[0].label, "Weekly limit");
        assert_eq!(quota_lanes[0].remaining, Some(18.0));
    }

    #[test]
    fn full_read_keeps_each_active_limit_bucket_and_provider_name() {
        let fixture = full_fixture().replace(
            "\"rateLimitsByLimitId\": null",
            &format!(
                r#""rateLimitsByLimitId": {{
                  "codex": {{
                    "limitId": "codex",
                    "primary": {{"usedPercent": 26, "windowDurationMins": 300, "resetsAt": {PRIMARY_RESET}}},
                    "secondary": {{"usedPercent": 82, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }},
                  "spark": {{
                    "limitId": "spark",
                    "limitName": "Spark",
                    "primary": {{"usedPercent": 50, "windowDurationMins": 60, "resetsAt": {PRIMARY_RESET}}}
                  }}
                }}"#,
            ),
        );
        let snapshot = CodexQuotaObservation::from_full_read(&fixture)
            .unwrap()
            .sanitized_snapshot(observed_at())
            .unwrap();
        let ProviderSnapshot::Current { quota_lanes, .. } = snapshot else {
            panic!("expected current snapshot");
        };
        assert_eq!(quota_lanes.len(), 3);
        assert_eq!(quota_lanes[2].label, "Spark 1-hour limit");
        assert_eq!(quota_lanes[2].remaining, Some(50.0));
    }

    #[test]
    fn full_read_ignores_the_exact_codex_spark_quota() {
        let fixture = full_fixture().replace(
            "\"rateLimitsByLimitId\": null",
            &format!(
                r#""rateLimitsByLimitId": {{
                  "codex": {{
                    "limitId": "codex",
                    "primary": {{"usedPercent": 26, "windowDurationMins": 300, "resetsAt": {PRIMARY_RESET}}},
                    "secondary": {{"usedPercent": 82, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }},
                  "spark": {{
                    "limitId": "spark",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": {{"usedPercent": 50, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }}
                }}"#,
            ),
        );
        let snapshot = CodexQuotaObservation::from_full_read(&fixture)
            .unwrap()
            .sanitized_snapshot(observed_at())
            .unwrap();
        let ProviderSnapshot::Current { quota_lanes, .. } = snapshot else {
            panic!("expected current snapshot");
        };

        assert_eq!(quota_lanes.len(), 2);
        assert_eq!(quota_lanes[0].label, "5-hour limit");
        assert_eq!(quota_lanes[1].label, "Weekly limit");
    }

    #[test]
    fn full_read_with_only_the_ignored_spark_quota_is_unavailable() {
        let fixture = full_fixture().replace(
            "\"rateLimitsByLimitId\": null",
            &format!(
                r#""rateLimitsByLimitId": {{
                  "spark": {{
                    "limitId": "spark",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": {{"usedPercent": 50, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }}
                }}"#,
            ),
        );
        let snapshot = CodexQuotaObservation::from_full_read(&fixture)
            .unwrap()
            .sanitized_snapshot(observed_at())
            .unwrap();

        assert_eq!(
            snapshot,
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Codex,
                quota_lanes: [],
            }
        );
    }

    #[test]
    fn sparse_update_for_an_ignored_spark_quota_is_also_ignored() {
        let fixture = full_fixture().replace(
            "\"rateLimitsByLimitId\": null",
            &format!(
                r#""rateLimitsByLimitId": {{
                  "codex": {{
                    "limitId": "codex",
                    "primary": {{"usedPercent": 26, "windowDurationMins": 300, "resetsAt": {PRIMARY_RESET}}},
                    "secondary": {{"usedPercent": 82, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }},
                  "spark": {{
                    "limitId": "spark",
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": {{"usedPercent": 50, "windowDurationMins": 10080, "resetsAt": {SECONDARY_RESET}}}
                  }}
                }}"#,
            ),
        );
        let mut observation = CodexQuotaObservation::from_full_read(&fixture).unwrap();
        let update = r#"{
          "rateLimits": {
            "limitId": "spark",
            "primary": { "usedPercent": 60 }
          }
        }"#;

        assert!(!observation.merge_sparse(update).unwrap());
        assert_eq!(observation.buckets.len(), 1);
    }

    #[test]
    fn sparse_update_requires_a_full_observation_and_preserves_missing_fields() {
        let sparse = r#"{
          "rateLimits": {
            "primary": { "usedPercent": 40 },
            "secondary": null
          }
        }"#;

        let mut observation = CodexQuotaObservation::from_full_read(&full_fixture()).unwrap();
        observation.merge_sparse(sparse).unwrap();
        let bucket = observation.buckets.get(DEFAULT_LIMIT_ID).unwrap();
        let primary = bucket.primary.as_ref().unwrap();
        let secondary = bucket.secondary.as_ref().unwrap();

        assert_eq!(primary.used_percent, UsedPercent(40));
        assert_eq!(primary.reset_at, UnixResetAt(PRIMARY_RESET));
        assert_eq!(primary.duration, WindowDurationMinutes(300));
        assert_eq!(secondary.used_percent, UsedPercent(82));
    }

    #[test]
    fn complete_sparse_window_can_add_a_new_lane_to_an_initialized_snapshot() {
        let mut observation =
            CodexQuotaObservation::from_full_read(&weekly_only_fixture()).unwrap();
        let update = format!(
            r#"{{
              "rateLimits": {{
                "limitId": "codex",
                "secondary": {{
                  "usedPercent": 26,
                  "windowDurationMins": 300,
                  "resetsAt": {PRIMARY_RESET}
                }}
              }}
            }}"#,
        );

        assert!(observation.merge_sparse(&update).unwrap());
        let snapshot = observation.sanitized_snapshot(observed_at()).unwrap();
        let ProviderSnapshot::Current { quota_lanes, .. } = snapshot else {
            panic!("expected current snapshot");
        };
        assert_eq!(quota_lanes.len(), 2);
        assert_eq!(quota_lanes[0].label, "Weekly limit");
        assert_eq!(quota_lanes[1].label, "5-hour limit");
    }

    #[test]
    fn sparse_update_cannot_create_an_unknown_limit_or_incomplete_window() {
        let unknown_limit = r#"{"rateLimits":{"limitId":"new","primary":{"usedPercent":40}}}"#;
        let incomplete_window =
            r#"{"rateLimits":{"limitId":"codex","secondary":{"usedPercent":40}}}"#;
        let mut observation =
            CodexQuotaObservation::from_full_read(&weekly_only_fixture()).unwrap();

        assert!(observation.merge_sparse(unknown_limit).is_err());
        assert!(!observation.merge_sparse(incomplete_window).unwrap());
        assert!(
            observation
                .buckets
                .get(DEFAULT_LIMIT_ID)
                .unwrap()
                .secondary
                .is_none()
        );
    }

    #[test]
    fn full_read_rejects_absent_windows_unknown_schema_and_sensitive_fields() {
        let absent = r#"{ "rateLimits": { "primary": null, "secondary": null } }"#;
        let changed = full_fixture().replace("usedPercent", "consumedPercent");
        let sensitive = full_fixture().replace(
            "\"limitId\": \"codex\"",
            "\"limitId\": \"codex\", \"sessionToken\": \"sentinel-secret\"",
        );

        assert!(CodexQuotaObservation::from_full_read(absent).is_err());
        assert!(CodexQuotaObservation::from_full_read(&changed).is_err());
        assert!(CodexQuotaObservation::from_full_read(&sensitive).is_err());
    }

    #[test]
    fn sanitized_snapshot_does_not_serialize_provider_source_material() {
        let observation = CodexQuotaObservation::from_full_read(&full_fixture()).unwrap();
        let output =
            serde_json::to_string(&observation.sanitized_snapshot(observed_at()).unwrap()).unwrap();

        for prohibited in [
            "usedPercent",
            "windowDurationMins",
            "limitId",
            "planType",
            "sessionToken",
            "rawResponse",
            "localPath",
        ] {
            assert!(!output.contains(prohibited), "leaked {prohibited}");
        }
    }

    #[test]
    fn debug_summary_contains_only_lane_count_and_safe_lane_labels() {
        let observation = CodexQuotaObservation::from_full_read(&weekly_only_fixture()).unwrap();
        let summary = debug_observation_summary(&observation);

        assert_eq!(summary, "lane_count=1 lane-1=\"Weekly limit\"");
        for prohibited in [
            "codex",
            "usedPercent",
            "resetsAt",
            "10080",
            "82",
            OBSERVED_AT,
        ] {
            assert!(!summary.contains(prohibited), "leaked {prohibited}");
        }
    }

    #[test]
    fn time_formatter_is_rfc3339() {
        assert_eq!(format_time(observed_at()).unwrap(), OBSERVED_AT);
    }
}
