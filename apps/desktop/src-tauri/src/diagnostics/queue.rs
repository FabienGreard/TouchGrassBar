use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{AppContext, DiagnosticError, Failure, Report};

pub(super) const MAX_REPORT_BYTES: usize = 32 * 1024;
// Leave half of the 2 MiB disk budget for the atomic replacement file.
pub(super) const MAX_QUEUE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const GROUP_INTERVAL_MS: u64 = 5 * 60_000;
const MAX_ENTRIES: usize = 32;
const MAX_ATTEMPTS_PER_HOUR: usize = 30;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub reporter_id: String,
    pub generation: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    report: Report,
    frozen: bool,
    attempts: u32,
    next_attempt_at: u64,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredQueue {
    version: u8,
    #[serde(default)]
    authority_suspended: bool,
    binding: Option<Binding>,
    entries: Vec<Entry>,
    groups: BTreeMap<String, u64>,
    attempt_times: Vec<u64>,
}

pub(super) struct Queue {
    path: Option<PathBuf>,
    app: AppContext,
    state: StoredQueue,
}

impl Queue {
    pub fn open(directory: Option<&Path>, app: AppContext, now: u64) -> Self {
        let path = directory.and_then(|directory| {
            private_directory(directory).ok()?;
            Some(directory.join("pending-v1.json"))
        });
        let mut state = path
            .as_ref()
            .and_then(|path| read_state(path).ok())
            .unwrap_or_default();
        state.version = 1;
        state.entries.retain(|entry| {
            entry.report.valid(now)
                && serde_json::to_vec(&entry.report)
                    .is_ok_and(|bytes| bytes.len() <= MAX_REPORT_BYTES)
        });
        let mut queue = Self { path, app, state };
        queue.prune(now);
        // Remove expired or invalid stored data even if the app has no new failure.
        let _ = queue.persist();
        queue
    }

    pub fn bind(&mut self, binding: Option<Binding>) -> Result<(), DiagnosticError> {
        if self.state.binding != binding {
            // Unregistered reports and reports from a former authority stay local until discarded.
            // They must never acquire the next Profile's authority.
            self.state.entries.clear();
            self.state.groups.clear();
            self.state.attempt_times.clear();
            self.state.binding = binding;
            self.persist()?;
        }
        Ok(())
    }

    pub fn suspended(&self) -> bool {
        self.state.authority_suspended
    }

    pub fn matches_binding(&self, binding: &Binding) -> bool {
        self.state.binding.as_ref() == Some(binding)
    }

    pub fn expire(&mut self, now: u64) -> Result<(), DiagnosticError> {
        if self.prune(now) {
            self.persist()?;
        }
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), DiagnosticError> {
        self.state.authority_suspended = true;
        self.bind(None)?;
        self.persist()
    }

    pub fn resume(&mut self, binding: Binding) -> Result<(), DiagnosticError> {
        self.bind(Some(binding))?;
        if self.state.authority_suspended {
            self.state.authority_suspended = false;
            self.persist()?;
        }
        Ok(())
    }

    pub fn capture(&mut self, failure: Failure, now: u64) -> Result<(), DiagnosticError> {
        if !failure.valid() {
            return Err(DiagnosticError);
        }
        self.prune(now);
        let group = format!("{}:{}", self.app.version, failure.group());
        if let Some(entry) = self.state.entries.iter_mut().find(|entry| {
            !entry.frozen
                && entry.report.app == self.app
                && format!(
                    "{}:{}",
                    entry.report.app.version,
                    entry.report.failure.group()
                ) == group
        }) {
            entry.report.occurrence_count = (entry.report.occurrence_count + 1).min(10_000);
            entry.report.first_occurred_at = entry.report.first_occurred_at.min(now);
            entry.report.last_occurred_at = entry.report.last_occurred_at.max(now);
            if now >= entry.report.context_captured_at {
                entry.report.context_captured_at = now;
                entry.report.failure = failure;
            }
        } else {
            let due = self.state.groups.get(&group).map_or(now, |previous| {
                now.max(previous.saturating_add(GROUP_INTERVAL_MS))
            });
            let report = Report {
                schema_version: 1,
                report_id: random_uuid()?,
                first_occurred_at: now,
                last_occurred_at: now,
                context_captured_at: now,
                occurrence_count: 1,
                app: self.app.clone(),
                failure,
            };
            if !report.valid(now)
                || serde_json::to_vec(&report)
                    .map_err(|_| DiagnosticError)?
                    .len()
                    > MAX_REPORT_BYTES
            {
                return Err(DiagnosticError);
            }
            self.state.entries.push(Entry {
                report,
                frozen: false,
                attempts: 0,
                next_attempt_at: due,
            });
            self.state.groups.insert(group, due);
        }
        self.prune(now);
        self.persist()
    }

    /// Persist the immutable report and attempt counter before any network request.
    pub fn next(&mut self, binding: &Binding, now: u64) -> Result<Option<Report>, DiagnosticError> {
        let pruned = self.prune(now);
        if self.state.authority_suspended
            || self.state.binding.as_ref() != Some(binding)
            || self.state.attempt_times.len() >= MAX_ATTEMPTS_PER_HOUR
        {
            if pruned {
                self.persist()?;
            }
            return Ok(None);
        }
        let Some(entry) = self
            .state
            .entries
            .iter_mut()
            .find(|entry| entry.next_attempt_at <= now)
        else {
            if pruned {
                self.persist()?;
            }
            return Ok(None);
        };
        let was_frozen = entry.frozen;
        let persisted = self.path.is_some();
        entry.frozen = true;
        entry.attempts = entry.attempts.saturating_add(1);
        entry.next_attempt_at = now.saturating_add(backoff_ms(entry.attempts));
        let report_id = entry.report.report_id.clone();
        self.state.attempt_times.push(now);
        self.persist()?;
        let entry = self
            .state
            .entries
            .iter_mut()
            .find(|entry| entry.report.report_id == report_id)
            .ok_or(DiagnosticError)?;
        if persisted && self.path.is_none() && !was_frozen {
            // The disk may retain an older unfrozen payload under its previous ID.
            // A memory-only first attempt must use a new ID.
            entry.report.report_id = random_uuid()?;
        }
        Ok(Some(entry.report.clone()))
    }

    pub fn finish(
        &mut self,
        report_id: &str,
        outcome: super::Delivery,
        now: u64,
    ) -> Result<(), DiagnosticError> {
        match outcome {
            super::Delivery::Accepted => self
                .state
                .entries
                .retain(|entry| entry.report.report_id != report_id),
            super::Delivery::Retry(after) => {
                if let Some(entry) = self
                    .state
                    .entries
                    .iter_mut()
                    .find(|entry| entry.report.report_id == report_id)
                {
                    entry.next_attempt_at = entry
                        .next_attempt_at
                        .max(now.saturating_add(after.clamp(30_000, 60 * 60_000)));
                }
            }
            super::Delivery::Rejected => self
                .state
                .entries
                .retain(|entry| entry.report.report_id != report_id),
            super::Delivery::Revoked => return self.bind(None),
        }
        self.persist()
    }

    fn prune(&mut self, now: u64) -> bool {
        let before = (
            self.state.entries.len(),
            self.state.attempt_times.len(),
            self.state.groups.len(),
        );
        self.state
            .entries
            .retain(|entry| now.saturating_sub(entry.report.first_occurred_at) <= MAX_AGE_MS);
        self.state
            .attempt_times
            .retain(|time| now.saturating_sub(*time) < 60 * 60_000);
        self.state.attempt_times.truncate(MAX_ATTEMPTS_PER_HOUR);
        self.state
            .groups
            .retain(|_, time| now.saturating_sub(*time) < 60 * 60_000);
        while self.state.groups.len() > 64 {
            let Some(key) = self
                .state
                .groups
                .iter()
                .min_by_key(|(_, time)| *time)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.state.groups.remove(&key);
        }
        while self.state.entries.len() > MAX_ENTRIES
            || serde_json::to_vec(&self.state).map_or(true, |bytes| bytes.len() > MAX_QUEUE_BYTES)
        {
            if self.state.entries.is_empty() {
                break;
            }
            self.state.entries.remove(0);
        }
        before
            != (
                self.state.entries.len(),
                self.state.attempt_times.len(),
                self.state.groups.len(),
            )
    }

    fn persist(&mut self) -> Result<(), DiagnosticError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(&self.state).map_err(|_| DiagnosticError)?;
        if bytes.len() > MAX_QUEUE_BYTES {
            return Err(DiagnosticError);
        }
        if atomic_write(path, &bytes).is_err() {
            // Keep the bounded memory queue if the disk fails. The app can still report a disk error.
            self.path = None;
            for entry in &mut self.state.entries {
                if !entry.frozen {
                    entry.report.report_id = random_uuid()?;
                }
            }
        }
        Ok(())
    }
}

fn backoff_ms(attempt: u32) -> u64 {
    30_000_u64
        .saturating_mul(1_u64 << attempt.saturating_sub(1).min(7))
        .min(60 * 60_000)
}

fn random_uuid() -> Result<String, DiagnosticError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| DiagnosticError)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn private_directory(directory: &Path) -> Result<(), DiagnosticError> {
    if let Ok(metadata) = fs::symlink_metadata(directory) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(DiagnosticError);
        }
    } else {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory).map_err(|_| DiagnosticError)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| DiagnosticError)?;
    }
    Ok(())
}

fn read_state(path: &Path) -> Result<StoredQueue, DiagnosticError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DiagnosticError)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_QUEUE_BYTES as u64
    {
        return Err(DiagnosticError);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    let file = options.open(path).map_err(|_| DiagnosticError)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| DiagnosticError)?;
    }
    file.take((MAX_QUEUE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| DiagnosticError)?;
    let state: StoredQueue = serde_json::from_slice(&bytes).map_err(|_| DiagnosticError)?;
    if state.version != 1 {
        return Err(DiagnosticError);
    }
    Ok(state)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), DiagnosticError> {
    let temporary = path.with_extension("tmp");
    // A stale replacement contains no credentials and is never trusted or followed.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(DiagnosticError),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary).map_err(|_| DiagnosticError)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| DiagnosticError)?;
    fs::rename(&temporary, path).map_err(|_| DiagnosticError)?;
    File::open(path.parent().ok_or(DiagnosticError)?)
        .and_then(|file| file.sync_all())
        .map_err(|_| DiagnosticError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const NOW: u64 = 1_800_000_000_000;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "touchgrass-diagnostics-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn app() -> AppContext {
        AppContext {
            version: "0.0.34".into(),
            build: None,
            os_version: None,
            architecture: Architecture::Unknown,
        }
    }
    fn binding() -> Binding {
        Binding {
            reporter_id: "reporter-one".into(),
            generation: 1,
        }
    }
    fn failure() -> Failure {
        Failure::Parser {
            provider: Provider::Claude,
            code: ParserCode::ParserScanFailed,
            context: ParserContext {
                parser_version: Some(11),
                source_versions: vec![],
                review_status: ReviewStatus::Unknown,
                files_seen: None,
                records_accepted: None,
                records_rejected: None,
                reason: ParserReason::ReadFailed,
            },
        }
    }

    #[test]
    fn restart_retries_exact_frozen_report_and_never_uploads_without_failure() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.bind(Some(binding())).unwrap();
        assert!(queue.next(&binding(), NOW).unwrap().is_none());
        queue.capture(failure(), NOW).unwrap();
        let first = queue.next(&binding(), NOW).unwrap().unwrap();
        let mut restarted = Queue::open(Some(&temp.0), app(), NOW + 30_000);
        let retry = restarted.next(&binding(), NOW + 30_000).unwrap().unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&retry).unwrap()
        );
        restarted
            .finish(&retry.report_id, Delivery::Accepted, NOW + 30_000)
            .unwrap();
        assert!(restarted.next(&binding(), NOW + 60_000).unwrap().is_none());
    }

    #[test]
    fn repeats_coalesce_locally_but_cannot_change_a_frozen_report() {
        let mut queue = Queue::open(None, app(), NOW);
        queue.bind(Some(binding())).unwrap();
        queue.capture(failure(), NOW).unwrap();
        queue.capture(failure(), NOW + 1).unwrap();
        queue.capture(failure(), NOW - 1).unwrap();
        let first = queue.next(&binding(), NOW + 1).unwrap().unwrap();
        assert_eq!(first.occurrence_count, 3);
        assert_eq!(first.first_occurred_at, NOW - 1);
        assert_eq!(first.last_occurred_at, NOW + 1);
        assert_eq!(first.context_captured_at, NOW + 1);
        queue.capture(failure(), NOW + 2).unwrap();
        queue.capture(failure(), NOW + 3).unwrap();
        let retry = queue.next(&binding(), NOW + 30_001).unwrap().unwrap();
        assert_eq!(first, retry);
        queue
            .finish(&first.report_id, Delivery::Accepted, NOW + 30_001)
            .unwrap();
        assert!(queue.next(&binding(), NOW + 60_000).unwrap().is_none());
        assert_eq!(
            queue
                .next(&binding(), NOW + GROUP_INTERVAL_MS)
                .unwrap()
                .unwrap()
                .occurrence_count,
            2
        );
    }

    #[test]
    fn reports_never_cross_profile_or_generation_boundaries() {
        let mut queue = Queue::open(None, app(), NOW);
        queue.capture(failure(), NOW).unwrap();
        queue.bind(Some(binding())).unwrap();
        assert!(queue.next(&binding(), NOW).unwrap().is_none());
        queue.capture(failure(), NOW).unwrap();
        let newer = Binding {
            generation: 2,
            ..binding()
        };
        assert!(queue.next(&newer, NOW).unwrap().is_none());
        queue.bind(Some(newer.clone())).unwrap();
        assert!(queue.next(&newer, NOW).unwrap().is_none());
    }

    #[test]
    fn local_age_and_capacity_are_bounded() {
        let mut queue = Queue::open(None, app(), NOW);
        queue.bind(Some(binding())).unwrap();
        for index in 0..100 {
            queue
                .capture(failure(), NOW + index * GROUP_INTERVAL_MS)
                .unwrap();
            if let Some(entry) = queue.state.entries.last_mut() {
                entry.frozen = true;
            }
        }
        assert!(queue.state.entries.len() <= MAX_ENTRIES);
        assert!(serde_json::to_vec(&queue.state).unwrap().len() <= MAX_QUEUE_BYTES);
        queue.prune(NOW + MAX_AGE_MS + 100 * GROUP_INTERVAL_MS);
        assert!(queue.state.entries.is_empty());
    }

    #[test]
    fn raw_content_is_rejected_before_persistence() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        let mut bad = failure();
        if let Failure::Parser { context, .. } = &mut bad {
            context
                .source_versions
                .push("/Users/private/secret-token".into());
        }
        assert!(queue.capture(bad, NOW).is_err());
        let stored = fs::read_to_string(temp.0.join("pending-v1.json")).unwrap();
        assert!(!stored.contains("secret-token"));
        assert!(queue.state.entries.is_empty());
        assert!(serde_json::from_value::<Failure>(serde_json::json!({"area":"parser","code":"parser_scan_failed","provider":"claude","context":{},"rawLog":"secret"})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn queue_is_private_and_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.capture(failure(), NOW).unwrap();
        assert_eq!(
            fs::metadata(&temp.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(temp.0.join("pending-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let link = Temp::new();
        symlink(&temp.0, &link.0).unwrap();
        let linked = Queue::open(Some(&link.0), app(), NOW);
        assert!(linked.path.is_none());
        fs::remove_file(&link.0).unwrap();
    }

    #[test]
    fn an_upgrade_does_not_attach_new_context_to_the_old_app_version() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.bind(Some(binding())).unwrap();
        queue.capture(failure(), NOW).unwrap();
        let mut next_app = app();
        next_app.version = "0.0.35".into();
        let mut restarted = Queue::open(Some(&temp.0), next_app, NOW + 1);
        restarted.capture(failure(), NOW + 1).unwrap();
        let first = restarted.next(&binding(), NOW + 1).unwrap().unwrap();
        assert_eq!(first.app.version, "0.0.34");
        assert_eq!(first.occurrence_count, 1);
        restarted
            .finish(&first.report_id, Delivery::Accepted, NOW + 1)
            .unwrap();
        let second = restarted.next(&binding(), NOW + 1).unwrap().unwrap();
        assert_eq!(second.app.version, "0.0.35");
        assert_eq!(second.occurrence_count, 1);
    }

    #[test]
    fn a_new_parser_or_failure_reason_remains_separate() {
        let mut queue = Queue::open(None, app(), NOW);
        queue.bind(Some(binding())).unwrap();
        queue.capture(failure(), NOW).unwrap();
        let mut next_parser = failure();
        if let Failure::Parser { context, .. } = &mut next_parser {
            context.parser_version = Some(12);
        }
        queue.capture(next_parser, NOW + 1).unwrap();
        let mut next_reason = failure();
        if let Failure::Parser { context, .. } = &mut next_reason {
            context.reason = ParserReason::InvalidCounter;
        }
        queue.capture(next_reason, NOW + 1).unwrap();
        assert_eq!(queue.state.entries.len(), 3);
    }

    #[test]
    fn failed_uploads_back_off_without_creating_more_reports_and_honor_server_delay() {
        let mut queue = Queue::open(None, app(), NOW);
        queue.bind(Some(binding())).unwrap();
        queue.capture(failure(), NOW).unwrap();
        let first = queue.next(&binding(), NOW).unwrap().unwrap();
        queue
            .finish(&first.report_id, Delivery::Retry(120_000), NOW)
            .unwrap();
        assert!(queue.next(&binding(), NOW + 119_999).unwrap().is_none());
        assert_eq!(
            queue.next(&binding(), NOW + 120_000).unwrap().unwrap(),
            first
        );
        assert_eq!(queue.state.entries.len(), 1);
    }

    #[test]
    fn stale_reports_are_deleted_from_disk_on_a_healthy_restart() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.capture(failure(), NOW).unwrap();
        let restarted = Queue::open(Some(&temp.0), app(), NOW + MAX_AGE_MS + 1);
        assert!(restarted.state.entries.is_empty());
        assert!(
            !fs::read_to_string(temp.0.join("pending-v1.json"))
                .unwrap()
                .contains("parser_scan_failed")
        );
    }

    #[test]
    fn a_recovery_pause_survives_restart_and_needs_confirmed_registration() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.bind(Some(binding())).unwrap();
        queue.capture(failure(), NOW).unwrap();
        queue.suspend().unwrap();
        let mut restarted = Queue::open(Some(&temp.0), app(), NOW + 1);
        assert!(restarted.suspended());
        restarted.bind(Some(binding())).unwrap();
        assert!(restarted.next(&binding(), NOW + 1).unwrap().is_none());
        restarted.resume(binding()).unwrap();
        restarted.capture(failure(), NOW + 1).unwrap();
        assert!(restarted.next(&binding(), NOW + 1).unwrap().is_some());
    }

    #[test]
    fn idle_expiry_deletes_local_reports_without_registered_authority() {
        let temp = Temp::new();
        let mut queue = Queue::open(Some(&temp.0), app(), NOW);
        queue.capture(failure(), NOW).unwrap();
        assert!(queue.state.binding.is_none());
        queue.expire(NOW + MAX_AGE_MS + 1).unwrap();
        assert!(queue.state.entries.is_empty());
        assert!(
            !fs::read_to_string(temp.0.join("pending-v1.json"))
                .unwrap()
                .contains("parser_scan_failed")
        );
    }
}
