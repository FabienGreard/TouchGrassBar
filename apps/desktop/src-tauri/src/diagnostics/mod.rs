//! Failure-only diagnostics. This module has no dependency on SQLite readiness.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod queue;
mod transport;
mod types;
mod validation;

pub(crate) use types::*;

use std::{
    path::Path,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::profile::ProfileCoordinator;
use queue::Queue;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DiagnosticError;

#[derive(Clone, Copy)]
pub(super) enum Delivery {
    Accepted,
    Retry(u64),
    Rejected,
    Revoked,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    wait: Mutex<()>,
    stopped: AtomicBool,
    suspended: AtomicBool,
    authority_epoch: AtomicU64,
    captured: SyncSender<Captured>,
    pending: Mutex<Receiver<Captured>>,
}

const MAX_HANDOFF: usize = 64;

struct Captured {
    payload: Box<[u8]>,
    epoch: u64,
    occurred_at: u64,
}

impl Shared {
    fn new(queue: Queue, suspended: bool) -> Self {
        let (captured, pending) = sync_channel(MAX_HANDOFF);
        Self {
            queue: Mutex::new(queue),
            wake: Condvar::new(),
            wait: Mutex::new(()),
            stopped: AtomicBool::new(false),
            suspended: AtomicBool::new(suspended),
            authority_epoch: AtomicU64::new(0),
            captured,
            pending: Mutex::new(pending),
        }
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

pub(crate) fn initialize(app_data_dir: &Path, app_version: &str) -> Result<(), DiagnosticError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    // If parent creation fails, Queue::open installs the bounded memory queue.
    let _ = builder.create(app_data_dir);
    initialize_inner(Some(&app_data_dir.join("diagnostics")), app_version)
}

pub(crate) fn initialize_memory(app_version: &str) -> Result<(), DiagnosticError> {
    initialize_inner(None, app_version)
}

fn initialize_inner(directory: Option<&Path>, app_version: &str) -> Result<(), DiagnosticError> {
    if SHARED.get().is_some() {
        return Ok(());
    }
    let app = AppContext {
        version: if types::numeric_version(app_version) {
            app_version.to_owned()
        } else {
            "0.0.0".into()
        },
        build: None,
        os_version: os_version(),
        architecture: match std::env::consts::ARCH {
            "aarch64" => Architecture::Aarch64,
            "x86_64" => Architecture::X86_64,
            _ => Architecture::Unknown,
        },
    };
    let mut queue = Queue::open(directory, app, now());
    #[cfg(target_os = "macos")]
    let suspended = transport::initialize_binding(&mut queue).unwrap_or(false);
    #[cfg(not(target_os = "macos"))]
    let suspended = false;
    let _ = SHARED.set(Arc::new(Shared::new(queue, suspended)));
    Ok(())
}

/// Record a classified failure. There is deliberately no success-report operation.
/// Collection and upload failures cannot call this function recursively.
pub(crate) fn capture(failure: Failure) {
    capture_at_epoch(failure, capture_epoch());
}

pub(crate) fn capture_epoch() -> u64 {
    SHARED
        .get()
        .map_or(0, |shared| shared.authority_epoch.load(Ordering::SeqCst))
}

pub(crate) fn capture_at_epoch(failure: Failure, epoch: u64) {
    if epoch != capture_epoch() {
        return;
    }
    #[cfg(test)]
    if TEST_CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(failures) = slot.as_mut() {
            if failure.valid() {
                failures.push(failure.clone());
            }
            true
        } else {
            false
        }
    }) {
        return;
    }
    let Some(shared) = SHARED.get() else {
        return;
    };
    capture_for(shared, failure, epoch);
}

fn capture_for(shared: &Shared, failure: Failure, epoch: u64) {
    handoff(shared, failure, epoch, now());
}

fn handoff(shared: &Shared, failure: Failure, epoch: u64, occurred_at: u64) {
    if shared.stopped.load(Ordering::Relaxed)
        || shared.suspended.load(Ordering::SeqCst)
        || shared.authority_epoch.load(Ordering::SeqCst) != epoch
        || !failure.valid()
    {
        return;
    }
    let Ok(payload) = serde_json::to_vec(&failure) else {
        return;
    };
    if payload.len() > queue::MAX_REPORT_BYTES {
        return;
    }
    // This never waits for the worker, disk, or network. Full handoffs drop the
    // new context; diagnostics cannot delay product work.
    let _ = shared.captured.try_send(Captured {
        payload: payload.into_boxed_slice(),
        epoch,
        occurred_at,
    });
    shared.wake.notify_one();
}

fn persist_suspension(shared: &Shared) {
    if shared.suspended.load(Ordering::SeqCst) {
        if let Ok(mut queue) = shared.queue.lock() {
            if !queue.suspended() {
                let _ = queue.suspend();
            }
        }
    }
}

fn drain_captured(shared: &Shared) {
    persist_suspension(shared);
    let Ok(pending) = shared.pending.lock() else {
        return;
    };
    let captures: Vec<_> = pending.try_iter().take(MAX_HANDOFF).collect();
    drop(pending);
    let Ok(mut queue) = shared.queue.lock() else {
        return;
    };
    // Local retention applies even when Keychain, registration, or transport is unavailable.
    let _ = queue.expire(now());
    for captured in captures {
        if !shared.suspended.load(Ordering::SeqCst)
            && shared.authority_epoch.load(Ordering::SeqCst) == captured.epoch
            && now().saturating_sub(captured.occurred_at) <= queue::MAX_AGE_MS
        {
            if let Ok(failure) = serde_json::from_slice(&captured.payload) {
                let _ = queue.capture(failure, captured.occurred_at);
            }
        }
    }
}

pub(crate) fn safe_catalog_reference(value: &str) -> String {
    if validation::catalog_version(value) {
        return value.to_owned();
    }
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hash}")
}

/// Recovery changes the Profile authority. Pause capture synchronously before it
/// starts, so a failure during the transition cannot acquire the old authority.
pub(crate) fn suspend_for_profile_transition() {
    if let Some(shared) = SHARED.get() {
        suspend(shared);
    }
}

fn suspend(shared: &Shared) {
    shared.authority_epoch.fetch_add(1, Ordering::SeqCst);
    shared.suspended.store(true, Ordering::SeqCst);
    shared.wake.notify_one();
}

pub(crate) struct DiagnosticRuntime {
    shared: Option<Arc<Shared>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DiagnosticRuntime {
    pub(crate) fn start(profile: Arc<Mutex<ProfileCoordinator>>) -> Self {
        let shared = SHARED.get().cloned();
        let worker = shared.as_ref().and_then(|shared| {
            let shared = Arc::clone(shared);
            std::thread::Builder::new()
                .name("failure-diagnostics".into())
                .spawn(move || {
                    #[cfg(target_os = "macos")]
                    let mut client = transport::Client::new();
                    while !shared.stopped.load(Ordering::Relaxed) {
                        drain_captured(&shared);
                        #[cfg(target_os = "macos")]
                        client.pump(&shared, &profile);
                        #[cfg(not(target_os = "macos"))]
                        let _ = &profile;
                        let Ok(wait) = shared.wait.lock() else {
                            break;
                        };
                        if !shared.stopped.load(Ordering::Relaxed) {
                            let _ = shared.wake.wait_timeout(wait, Duration::from_secs(10));
                        }
                    }
                })
                .ok()
        });
        Self {
            shared,
            worker: Mutex::new(worker),
        }
    }

    pub(crate) fn shutdown(&self) {
        if let Some(shared) = &self.shared {
            shared.stopped.store(true, Ordering::Relaxed);
            shared.wake.notify_all();
        }
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                if worker.is_finished() {
                    let _ = worker.join();
                }
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u64::MAX as u128) as u64
        })
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes = [0u8; 64];
        let mut length = bytes.len();
        // Fixed OS metadata only; this does not inspect provider processes or files.
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.osproductversion".as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0 || length == 0 || length > bytes.len() {
            return None;
        }
        let value = std::str::from_utf8(&bytes[..length])
            .ok()?
            .trim_end_matches('\0');
        types::numeric_version(value).then(|| value.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    None
}

#[cfg(test)]
thread_local! { static TEST_CAPTURE: std::cell::RefCell<Option<Vec<Failure>>> = const { std::cell::RefCell::new(None) }; }

#[cfg(test)]
pub(crate) fn collect_failures_for_test(action: impl FnOnce()) -> Vec<Failure> {
    struct Restore(Option<Vec<Failure>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_CAPTURE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let restore = Restore(TEST_CAPTURE.with(|slot| slot.borrow_mut().replace(Vec::new())));
    action();
    let result = TEST_CAPTURE.with(|slot| slot.borrow_mut().take().unwrap_or_default());
    drop(restore);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deferred_scan_cannot_capture_under_replacement_profile_authority() {
        let time = now();
        let original = queue::Binding {
            reporter_id: "reporterone".into(),
            generation: 1,
        };
        let replacement = queue::Binding {
            reporter_id: "reportertwo".into(),
            generation: 1,
        };
        let mut queue = Queue::open(
            None,
            AppContext {
                version: "0.0.34".into(),
                build: None,
                os_version: None,
                architecture: Architecture::Unknown,
            },
            time,
        );
        queue.bind(Some(original)).unwrap();
        let shared = Shared::new(queue, false);
        let epoch_at_scan_start = shared.authority_epoch.load(Ordering::SeqCst);
        suspend(&shared);
        shared
            .queue
            .lock()
            .unwrap()
            .resume(replacement.clone())
            .unwrap();
        shared.authority_epoch.fetch_add(1, Ordering::SeqCst);
        shared.suspended.store(false, Ordering::SeqCst);
        let failure = Failure::Database {
            code: DatabaseCode::DatabaseOpenFailed,
            provider: None,
            context: DatabaseContext {
                stage: Some("open-database".into()),
                observed_format: None,
                expected_format: None,
                modules: vec![],
                backup_state: BackupState::Unknown,
            },
        };
        capture_for(&shared, failure.clone(), epoch_at_scan_start);
        assert!(
            shared
                .queue
                .lock()
                .unwrap()
                .next(&replacement, time + 1000)
                .unwrap()
                .is_none()
        );
        capture_for(
            &shared,
            failure,
            shared.authority_epoch.load(Ordering::SeqCst),
        );
        drain_captured(&shared);
        assert!(
            shared
                .queue
                .lock()
                .unwrap()
                .next(&replacement, time + 1000)
                .unwrap()
                .is_some()
        );
    }

    fn fixture() -> (Arc<Shared>, queue::Binding, Failure) {
        let mut queue = Queue::open(
            None,
            AppContext {
                version: "0.0.34".into(),
                build: None,
                os_version: None,
                architecture: Architecture::Unknown,
            },
            now(),
        );
        let binding = queue::Binding {
            reporter_id: "reporterone".into(),
            generation: 1,
        };
        queue.bind(Some(binding.clone())).unwrap();
        let failure = Failure::Database {
            code: DatabaseCode::DatabaseOpenFailed,
            provider: None,
            context: DatabaseContext {
                stage: Some("open-database".into()),
                observed_format: None,
                expected_format: None,
                modules: vec![],
                backup_state: BackupState::Unknown,
            },
        };
        (Arc::new(Shared::new(queue, false)), binding, failure)
    }

    #[test]
    fn blocked_persistence_cannot_stall_capture_or_profile_suspension() {
        let (shared, _, failure) = fixture();
        // Hold the same lock that the worker holds across disk persistence.
        let disk_blocked = shared.queue.lock().unwrap();
        let worker_shared = Arc::clone(&shared);
        let (done, result) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            capture_for(&worker_shared, failure, 0);
            suspend(&worker_shared);
            done.send(()).unwrap();
        });
        let returned_without_disk = result.recv_timeout(Duration::from_millis(500)).is_ok();
        drop(disk_blocked);
        thread.join().unwrap();
        assert!(returned_without_disk);
    }

    #[test]
    fn handoff_is_bounded_and_keeps_capture_time_when_worker_is_delayed() {
        let (shared, binding, failure) = fixture();
        let occurred_at = now().saturating_sub(60_000);
        for _ in 0..MAX_HANDOFF + 20 {
            handoff(&shared, failure.clone(), 0, occurred_at);
        }
        drain_captured(&shared);
        let report = shared
            .queue
            .lock()
            .unwrap()
            .next(&binding, now())
            .unwrap()
            .unwrap();
        assert_eq!(report.occurrence_count, MAX_HANDOFF as u64);
        assert_eq!(report.first_occurred_at, occurred_at);
        assert_eq!(report.context_captured_at, occurred_at);
    }

    #[test]
    fn queued_capture_is_discarded_if_authority_changes_before_worker_drain() {
        let (shared, binding, failure) = fixture();
        capture_for(&shared, failure, 0);
        shared.authority_epoch.fetch_add(1, Ordering::SeqCst);
        drain_captured(&shared);
        assert!(
            shared
                .queue
                .lock()
                .unwrap()
                .next(&binding, now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn arbitrary_catalog_names_are_hashed_before_they_leave_the_device() {
        assert_eq!(
            safe_catalog_reference("openai-api-2026-09-11-v1"),
            "openai-api-2026-09-11-v1"
        );
        let reference = safe_catalog_reference("/Users/private/example-token");
        assert!(validation::catalog_version(&reference));
        assert!(!reference.contains("private"));
        assert!(!reference.contains("example-token"));
    }
}
