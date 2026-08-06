use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::sanitized::{
    Clock, CodingProvider, ProviderSnapshot, QuotaLane, RefreshAttempt, RefreshFailure,
    RefreshTrigger, SanitizedDesktopStateV2, SnapshotRefreshAdapter,
};

const INITIALIZE_REQUEST_ID: i64 = 1;
const DEFAULT_LIMIT_ID: &str = "codex";

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
        if !snapshots.contains_key(DEFAULT_LIMIT_ID) {
            return Err(());
        }

        let mut buckets = BTreeMap::new();
        for (limit_id, snapshot) in snapshots {
            if snapshot
                .limit_id
                .as_deref()
                .is_some_and(|id| id != limit_id)
            {
                return Err(());
            }
            let require_two_windows = limit_id == DEFAULT_LIMIT_ID;
            let bucket = complete_bucket(snapshot, require_two_windows)?;
            buckets.insert(limit_id, bucket);
        }
        Ok(Self { buckets })
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
        let current = self.buckets.get_mut(&limit_id).ok_or(())?;
        let mut changed = false;
        if let Some(name) = notification.rate_limits.limit_name {
            current.name = nonempty_name(name)?;
            changed = true;
        }
        if let Some(primary) = notification.rate_limits.primary {
            merge_window(current.primary.as_mut().ok_or(())?, primary)?;
            changed = true;
        }
        if let Some(secondary) = notification.rate_limits.secondary {
            merge_window(current.secondary.as_mut().ok_or(())?, secondary)?;
            changed = true;
        }
        Ok(changed)
    }

    fn sanitized_snapshot(&self, observed_at: OffsetDateTime) -> Result<ProviderSnapshot, ()> {
        let mut quota_lanes = Vec::new();
        if let Some(bucket) = self.buckets.get(DEFAULT_LIMIT_ID) {
            append_bucket_lanes(&mut quota_lanes, DEFAULT_LIMIT_ID, bucket)?;
        }
        for (limit_id, bucket) in &self.buckets {
            if limit_id != DEFAULT_LIMIT_ID {
                append_bucket_lanes(&mut quota_lanes, limit_id, bucket)?;
            }
        }
        if quota_lanes.is_empty() {
            return Err(());
        }
        Ok(ProviderSnapshot::Current {
            provider: CodingProvider::Codex,
            observed_at: format_time(observed_at)?,
            quota_lanes,
        })
    }
}

fn complete_bucket(
    snapshot: RawRateLimitSnapshot,
    require_two_windows: bool,
) -> Result<RateLimitBucket, ()> {
    let primary = snapshot.primary.map(complete_window).transpose()?;
    let secondary = snapshot.secondary.map(complete_window).transpose()?;
    if (require_two_windows && (primary.is_none() || secondary.is_none()))
        || (primary.is_none() && secondary.is_none())
    {
        return Err(());
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
    Ok(RateLimitBucket {
        name,
        primary,
        secondary,
    })
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
    (!name.trim().is_empty()).then_some(Some(name)).ok_or(())
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
        let mut label = window_label(window.duration);
        if limit_id != DEFAULT_LIMIT_ID {
            let bucket_label = bucket.name.as_deref().unwrap_or(limit_id);
            label = format!("{bucket_label} {label}");
        }
        lanes.push(sanitized_lane(window, label)?);
    }
    Ok(())
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

pub(crate) struct CodexQuotaRefreshAdapter {
    clock: Arc<dyn Clock>,
    session: Mutex<Option<CodexAppServerSession>>,
    refresh_trigger: Mutex<Option<RefreshTrigger>>,
}

impl CodexQuotaRefreshAdapter {
    pub(crate) fn production(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            session: Mutex::new(None),
            refresh_trigger: Mutex::new(None),
        }
    }
}

impl SnapshotRefreshAdapter for CodexQuotaRefreshAdapter {
    fn install_refresh_trigger(&self, trigger: RefreshTrigger) {
        *self
            .refresh_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(trigger);
    }

    fn refresh(
        &self,
        mut cached: SanitizedDesktopStateV2,
        attempt: &RefreshAttempt,
    ) -> Result<Option<SanitizedDesktopStateV2>, RefreshFailure> {
        let mut session_guard = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session_guard.is_none() {
            let executable = resolve_codex_executable().ok_or(RefreshFailure::SourceUnavailable)?;
            let trigger = self
                .refresh_trigger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            *session_guard = Some(CodexAppServerSession::start(&executable, attempt, trigger)?);
        }
        let result = session_guard
            .as_mut()
            .ok_or(RefreshFailure::SourceUnavailable)?
            .read_observation(attempt);
        let observation = match result {
            Ok(observation) => observation,
            Err(error) => {
                session_guard.take();
                return Err(error);
            }
        };
        cached.providers[0] = observation
            .sanitized_snapshot(self.clock.now())
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        Ok(Some(cached))
    }
}

struct CodexAppServerSession {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<Result<String, std::io::Error>>,
    reader: Option<JoinHandle<()>>,
    next_request_id: i64,
    observation: Option<CodexQuotaObservation>,
}

impl CodexAppServerSession {
    fn start(
        executable: &Path,
        attempt: &RefreshAttempt,
        trigger: Option<RefreshTrigger>,
    ) -> Result<Self, RefreshFailure> {
        attempt.remaining()?;
        let mut command = Command::new(executable);
        command
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(parent) = executable.parent() {
            let mut paths = vec![parent.to_path_buf()];
            if let Some(current) = env::var_os("PATH") {
                paths.extend(env::split_paths(&current));
            }
            if let Ok(path) = env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        let stdin = child
            .stdin
            .take()
            .ok_or(RefreshFailure::SourceUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(RefreshFailure::SourceUnavailable)?;
        let (sender, messages) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if let Ok(ref line) = line
                    && serde_json::from_str::<Value>(line)
                        .ok()
                        .and_then(|message| {
                            message
                                .get("method")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        == Some("account/rateLimits/updated".to_owned())
                    && let Some(trigger) = &trigger
                {
                    trigger();
                }
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            stdin,
            messages,
            reader: Some(reader),
            next_request_id: INITIALIZE_REQUEST_ID + 1,
            observation: None,
        };
        session.send(json!({
            "method": "initialize",
            "id": INITIALIZE_REQUEST_ID,
            "params": {
                "clientInfo": {
                    "name": "touchgrassbar",
                    "title": "TouchGrassBar",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))?;
        session.wait_for_response(INITIALIZE_REQUEST_ID, attempt)?;
        session.send(json!({"method": "initialized", "params": {}}))?;
        Ok(session)
    }

    fn read_observation(
        &mut self,
        attempt: &RefreshAttempt,
    ) -> Result<CodexQuotaObservation, RefreshFailure> {
        if self.drain_sparse_notifications()? {
            return self
                .observation
                .clone()
                .ok_or(RefreshFailure::SourceUnavailable);
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.send(json!({
            "method": "account/rateLimits/read",
            "id": request_id,
            "params": null
        }))?;
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
            self.observation = Some(observation.clone());
            return Ok(observation);
        }
    }

    fn drain_sparse_notifications(&mut self) -> Result<bool, RefreshFailure> {
        let mut updated = false;
        loop {
            match self.messages.try_recv() {
                Ok(Ok(line)) => {
                    let message: Value = serde_json::from_str(&line)
                        .map_err(|_| RefreshFailure::SourceUnavailable)?;
                    if is_sparse_notification(&message) {
                        updated |= self.merge_notification(&message)?;
                    }
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(RefreshFailure::SourceUnavailable);
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(updated),
            }
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

    fn send(&mut self, message: Value) -> Result<(), RefreshFailure> {
        writeln!(self.stdin, "{message}")
            .and_then(|_| self.stdin.flush())
            .map_err(|_| RefreshFailure::SourceUnavailable)
    }

    fn receive(&self, attempt: &RefreshAttempt) -> Result<Value, RefreshFailure> {
        let line = self
            .messages
            .recv_timeout(attempt.remaining()?)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => RefreshFailure::DeadlineExceeded,
                mpsc::RecvTimeoutError::Disconnected => RefreshFailure::SourceUnavailable,
            })?
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
        serde_json::from_str(&line).map_err(|_| RefreshFailure::SourceUnavailable)
    }
}

impl Drop for CodexAppServerSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
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

    fn observed_at() -> OffsetDateTime {
        OffsetDateTime::parse(OBSERVED_AT, &Rfc3339).expect("valid observation time")
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
    fn sparse_update_cannot_create_an_unknown_limit_or_window() {
        let unknown_limit = r#"{"rateLimits":{"limitId":"new","primary":{"usedPercent":40}}}"#;
        let unknown_window = full_fixture().replace(
            &format!(
                "\"primary\": {{\n                  \"usedPercent\": 26,\n                  \"windowDurationMins\": 300,\n                  \"resetsAt\": {PRIMARY_RESET}\n                }}",
            ),
            "\"primary\": null",
        );
        let mut observation = CodexQuotaObservation::from_full_read(&full_fixture()).unwrap();
        assert!(observation.merge_sparse(unknown_limit).is_err());
        assert!(CodexQuotaObservation::from_full_read(&unknown_window).is_err());
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
    fn time_formatter_is_rfc3339() {
        assert_eq!(format_time(observed_at()).unwrap(), OBSERVED_AT);
    }
}
