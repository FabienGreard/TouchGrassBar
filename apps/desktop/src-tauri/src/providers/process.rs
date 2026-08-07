//! Bounded process ownership for coding provider adapters.
//!
//! All provider processes must use this module. It owns process groups, input,
//! output, reaping, and shutdown. Provider adapters cannot access raw child
//! handles or worker threads.

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, TryLockError, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use portable_pty::{Child as PtyChild, CommandBuilder, MasterPty, PtySize, native_pty_system};
use zeroize::Zeroizing;

const SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
const SHUTDOWN_BUDGET: Duration = Duration::from_millis(750);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const LINE_READ_BYTES: usize = 8 * 1024;
const OUTPUT_FRAME_OVERHEAD_BYTES: usize = 32;

const PHASE_RUNNING: u8 = 0;
const PHASE_TERM_SENT: u8 = 1;
const PHASE_KILL_SENT: u8 = 2;
const PHASE_STOPPED: u8 = 3;

type OutputObserver = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[derive(Clone)]
pub(crate) struct ProviderCommand {
    executable: OsString,
    arguments: Vec<OsString>,
    directory: Option<OsString>,
    environment: BTreeMap<OsString, Option<OsString>>,
}

impl ProviderCommand {
    pub(crate) fn new(executable: impl AsRef<OsStr>) -> Self {
        Self {
            executable: executable.as_ref().to_owned(),
            arguments: Vec::new(),
            directory: None,
            environment: BTreeMap::new(),
        }
    }

    pub(crate) fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    pub(crate) fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    pub(crate) fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.environment
            .insert(key.as_ref().to_owned(), Some(value.as_ref().to_owned()));
        self
    }

    pub(crate) fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.environment.insert(key.as_ref().to_owned(), None);
        self
    }

    pub(crate) fn cwd(&mut self, directory: impl AsRef<Path>) -> &mut Self {
        self.directory = Some(directory.as_ref().as_os_str().to_owned());
        self
    }

    fn std_command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(&self.arguments);
        if let Some(directory) = &self.directory {
            command.current_dir(directory);
        }
        for (key, value) in &self.environment {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }
        command
    }

    fn pty_command(&self) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.executable);
        command.args(&self.arguments);
        if let Some(directory) = &self.directory {
            command.cwd(directory);
        }
        for (key, value) in &self.environment {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }
        command
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProviderOutputMode {
    Lines {
        max_line_bytes: usize,
        max_buffered_bytes: usize,
    },
    Chunks {
        chunk_bytes: usize,
        max_buffered_bytes: usize,
    },
}

impl ProviderOutputMode {
    fn max_buffered_bytes(self) -> usize {
        match self {
            Self::Lines {
                max_buffered_bytes, ..
            }
            | Self::Chunks {
                max_buffered_bytes, ..
            } => max_buffered_bytes,
        }
    }

    fn is_valid(self) -> bool {
        match self {
            Self::Lines {
                max_line_bytes,
                max_buffered_bytes,
            } => {
                max_line_bytes > 0
                    && max_buffered_bytes
                        >= max_line_bytes.saturating_add(OUTPUT_FRAME_OVERHEAD_BYTES)
            }
            Self::Chunks {
                chunk_bytes,
                max_buffered_bytes,
            } => {
                chunk_bytes > 0
                    && max_buffered_bytes >= chunk_bytes.saturating_add(OUTPUT_FRAME_OVERHEAD_BYTES)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderProcessError {
    SupervisorStopping,
    StartFailed,
    InputUnavailable,
    OutputClosed,
    OutputLimit,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ShutdownReport {
    pub(crate) root_reaped: bool,
    pub(crate) output_reader_finished: bool,
    pub(crate) forced: bool,
    pub(crate) deadline_reached: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ShutdownSummary {
    pub(crate) process_count: usize,
    pub(crate) deadline_count: usize,
}

#[derive(Clone, Default)]
pub(crate) struct ProviderProcessSupervisor {
    inner: Arc<SupervisorInner>,
}

#[derive(Default)]
struct SupervisorInner {
    registry: Mutex<SupervisorRegistry>,
}

struct SupervisorRegistry {
    accepting: bool,
    next_id: u64,
    pending: BTreeMap<u64, Arc<AtomicBool>>,
    processes: BTreeMap<u64, Weak<ProcessControl>>,
}

impl Default for SupervisorRegistry {
    fn default() -> Self {
        Self {
            accepting: true,
            next_id: 1,
            pending: BTreeMap::new(),
            processes: BTreeMap::new(),
        }
    }
}

impl ProviderProcessSupervisor {
    pub(crate) fn spawn_piped(
        &self,
        command: ProviderCommand,
        output_mode: ProviderOutputMode,
        observer: Option<OutputObserver>,
    ) -> Result<ProviderProcess, ProviderProcessError> {
        self.spawn_registered(|cancelled| {
            build_piped_process(command, output_mode, observer, cancelled)
        })
    }

    pub(crate) fn spawn_pty(
        &self,
        command: ProviderCommand,
        size: PtySize,
        output_mode: ProviderOutputMode,
        observer: Option<OutputObserver>,
    ) -> Result<ProviderProcess, ProviderProcessError> {
        self.spawn_registered(|cancelled| {
            build_pty_process(command, size, output_mode, observer, cancelled)
        })
    }

    fn spawn_registered(
        &self,
        build: impl FnOnce(&AtomicBool) -> Result<ProcessControl, ProviderProcessError>,
    ) -> Result<ProviderProcess, ProviderProcessError> {
        let (registration_id, cancelled) = {
            let mut registry = self
                .inner
                .registry
                .lock()
                .map_err(|_| ProviderProcessError::SupervisorStopping)?;
            if !registry.accepting {
                return Err(ProviderProcessError::SupervisorStopping);
            }
            let registration_id = registry.next_id;
            registry.next_id = registry.next_id.saturating_add(1);
            let cancelled = Arc::new(AtomicBool::new(false));
            registry
                .pending
                .insert(registration_id, Arc::clone(&cancelled));
            (registration_id, cancelled)
        };
        let built = build(&cancelled);
        let mut registry = match self.inner.registry.lock() {
            Ok(registry) => registry,
            Err(_) => {
                if let Ok(control) = built {
                    let _ = control.shutdown_until(Instant::now() + SHUTDOWN_BUDGET);
                }
                return Err(ProviderProcessError::SupervisorStopping);
            }
        };
        registry.pending.remove(&registration_id);
        let control = Arc::new(built?);
        if !registry.accepting || cancelled.load(Ordering::Acquire) {
            drop(registry);
            let _ = control.shutdown_until(Instant::now() + SHUTDOWN_BUDGET);
            return Err(ProviderProcessError::SupervisorStopping);
        }
        registry
            .processes
            .retain(|_, process| process.strong_count() > 0);
        registry
            .processes
            .insert(registration_id, Arc::downgrade(&control));
        Ok(ProviderProcess {
            control,
            supervisor: Arc::downgrade(&self.inner),
            registration_id,
        })
    }

    pub(crate) fn shutdown_all(&self) -> ShutdownSummary {
        self.stop_all(true)
    }

    pub(crate) fn cancel_active(&self) -> ShutdownSummary {
        self.stop_all(false)
    }

    fn stop_all(&self, stop_accepting: bool) -> ShutdownSummary {
        let deadline = Instant::now() + SHUTDOWN_BUDGET;
        let processes = {
            let Ok(mut registry) = self.inner.registry.lock() else {
                return ShutdownSummary {
                    process_count: 0,
                    deadline_count: 1,
                };
            };
            if stop_accepting {
                registry.accepting = false;
            }
            for pending in registry.pending.values() {
                pending.store(true, Ordering::Release);
            }
            let processes = registry
                .processes
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>();
            registry.processes.clear();
            processes
        };

        for process in &processes {
            process.request_stop();
        }
        let grace_deadline = deadline.min(Instant::now() + SHUTDOWN_GRACE);
        while Instant::now() < grace_deadline {
            thread::sleep(POLL_INTERVAL);
        }
        for process in &processes {
            process.force_kill();
        }

        let reports = processes
            .iter()
            .map(|process| process.finish_shutdown(deadline))
            .collect::<Vec<_>>();
        ShutdownSummary {
            process_count: reports.len(),
            deadline_count: reports
                .iter()
                .filter(|report| report.deadline_reached)
                .count(),
        }
    }
}

pub(crate) struct ProviderProcess {
    control: Arc<ProcessControl>,
    supervisor: Weak<SupervisorInner>,
    registration_id: u64,
}

impl ProviderProcess {
    pub(crate) fn write_all(
        &self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<(), ProviderProcessError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(ProviderProcessError::TimedOut)?;
        let result = self.control.write_all(bytes, deadline);
        if result.is_err() {
            self.control.request_stop();
        }
        result
    }

    pub(crate) fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Zeroizing<Vec<u8>>, ProviderProcessError> {
        let result = self.control.output.receive_timeout(timeout);
        if result
            .as_ref()
            .is_err_and(|error| *error != ProviderProcessError::TimedOut)
        {
            self.control.request_stop();
        }
        result
    }

    pub(crate) fn try_receive(&self) -> Result<Option<Zeroizing<Vec<u8>>>, ProviderProcessError> {
        let result = self.control.output.try_receive();
        if result.as_ref().is_err() {
            self.control.request_stop();
        }
        result
    }

    pub(crate) fn shutdown(&self) -> ShutdownReport {
        self.control
            .shutdown_until(Instant::now() + SHUTDOWN_BUDGET)
    }
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();
        if let Some(supervisor) = self.supervisor.upgrade() {
            match supervisor.registry.try_lock() {
                Ok(mut registry) => {
                    registry.processes.remove(&self.registration_id);
                }
                Err(TryLockError::Poisoned(error)) => {
                    error.into_inner().processes.remove(&self.registration_id);
                }
                Err(TryLockError::WouldBlock) => {}
            }
        }
    }
}

struct ProcessControl {
    group_id: libc::pid_t,
    phase: Arc<AtomicU8>,
    cancelled: AtomicBool,
    forced: AtomicBool,
    root_reaped: Arc<AtomicBool>,
    root_reap_gate: Arc<RootReapGate>,
    reader_done: Arc<AtomicBool>,
    cancel_write: UnixStream,
    input: Mutex<Option<ProcessInput>>,
    output: Arc<OutputQueue>,
    root_reaper: Mutex<Option<JoinHandle<()>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    shutdown_gate: Mutex<()>,
}

struct ProcessInput {
    writer: Box<dyn Write + Send>,
    poll_fd: OwnedFd,
    cancel_read: UnixStream,
}

enum RootChild {
    Piped(std::process::Child),
    Pty(Box<dyn PtyChild + Send + Sync>),
}

#[derive(Default)]
struct RootReapGate {
    allowed: Mutex<bool>,
    changed: Condvar,
}

impl RootReapGate {
    fn allow(&self) {
        let mut allowed = self
            .allowed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *allowed = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut allowed = self
            .allowed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*allowed {
            allowed = self
                .changed
                .wait(allowed)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl RootChild {
    fn kill_direct(&mut self) {
        match self {
            Self::Piped(child) => {
                let _ = child.kill();
            }
            Self::Pty(child) => {
                let _ = child.kill();
            }
        }
    }

    fn wait(&mut self) {
        match self {
            Self::Piped(child) => {
                let _ = child.wait();
            }
            Self::Pty(child) => {
                let _ = child.wait();
            }
        }
    }

    fn try_wait(&mut self) -> io::Result<bool> {
        match self {
            Self::Piped(child) => child.try_wait().map(|status| status.is_some()),
            Self::Pty(child) => child.try_wait().map(|status| status.is_some()),
        }
    }
}

fn spawn_root_reaper(
    root: RootChild,
    completed: Arc<AtomicBool>,
    gate: Arc<RootReapGate>,
) -> Result<JoinHandle<()>, RootChild> {
    let root = Arc::new(Mutex::new(Some(root)));
    let thread_root = Arc::clone(&root);
    match thread::Builder::new()
        .name("provider-process-reaper".to_owned())
        .spawn(move || {
            gate.wait();
            let mut root = thread_root
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(root) = root.as_mut() {
                root.wait();
            }
            completed.store(true, Ordering::Release);
        }) {
        Ok(handle) => Ok(handle),
        Err(_) => Err(root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("a failed reaper start must return its root")),
    }
}

impl ProcessControl {
    fn write_all(&self, bytes: &[u8], deadline: Instant) -> Result<(), ProviderProcessError> {
        let mut input = loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(ProviderProcessError::Cancelled);
            }
            match self.input.try_lock() {
                Ok(input) => break input,
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(ProviderProcessError::TimedOut);
                    }
                    thread::sleep(POLL_INTERVAL);
                }
            }
        };
        let input = input
            .as_mut()
            .ok_or(ProviderProcessError::InputUnavailable)?;
        let mut written = 0;
        while written < bytes.len() {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(ProviderProcessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ProviderProcessError::TimedOut);
            }
            match input.writer.write(&bytes[written..]) {
                Ok(0) => return Err(ProviderProcessError::InputUnavailable),
                Ok(count) => written = written.saturating_add(count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_for_fd(
                        input.poll_fd.as_raw_fd(),
                        input.cancel_read.as_raw_fd(),
                        libc::POLLOUT,
                        Some(deadline),
                    )?;
                }
                Err(_) => return Err(ProviderProcessError::InputUnavailable),
            }
        }
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(ProviderProcessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ProviderProcessError::TimedOut);
            }
            match input.writer.flush() {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_for_fd(
                        input.poll_fd.as_raw_fd(),
                        input.cancel_read.as_raw_fd(),
                        libc::POLLOUT,
                        Some(deadline),
                    )?;
                }
                Err(_) => return Err(ProviderProcessError::InputUnavailable),
            }
        }
    }

    fn request_stop(&self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let previous = self.phase.fetch_max(PHASE_TERM_SENT, Ordering::AcqRel);
        let _ = self.cancel_write.shutdown(std::net::Shutdown::Both);
        self.output.cancel();
        if let Ok(mut input) = self.input.try_lock() {
            input.take();
        }
        if let Ok(mut master) = self.master.try_lock() {
            master.take();
        }
        if previous < PHASE_TERM_SENT {
            signal_group(self.group_id, libc::SIGTERM);
        }
    }

    fn force_kill(&self) {
        let previous = self.phase.fetch_max(PHASE_KILL_SENT, Ordering::AcqRel);
        if previous < PHASE_KILL_SENT {
            self.forced.store(true, Ordering::Release);
            signal_group(self.group_id, libc::SIGKILL);
        }
        self.root_reap_gate.allow();
    }

    fn shutdown_until(&self, deadline: Instant) -> ShutdownReport {
        self.request_stop();
        let gate = loop {
            match self.shutdown_gate.try_lock() {
                Ok(gate) => break Some(gate),
                Err(TryLockError::Poisoned(error)) => break Some(error.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        break None;
                    }
                    thread::sleep(POLL_INTERVAL);
                }
            }
        };
        let Some(_gate) = gate else {
            return self.current_report(true);
        };
        if self.phase.load(Ordering::Acquire) < PHASE_KILL_SENT {
            let grace_deadline = deadline.min(Instant::now() + SHUTDOWN_GRACE);
            while Instant::now() < grace_deadline {
                thread::sleep(POLL_INTERVAL);
            }
            self.force_kill();
        }
        self.finish_shutdown_locked(deadline)
    }

    fn finish_shutdown(&self, deadline: Instant) -> ShutdownReport {
        let gate = loop {
            match self.shutdown_gate.try_lock() {
                Ok(gate) => break Some(gate),
                Err(TryLockError::Poisoned(error)) => break Some(error.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        break None;
                    }
                    thread::sleep(POLL_INTERVAL);
                }
            }
        };
        let Some(_gate) = gate else {
            return self.current_report(true);
        };
        self.finish_shutdown_locked(deadline)
    }

    fn finish_shutdown_locked(&self, deadline: Instant) -> ShutdownReport {
        while Instant::now() < deadline {
            let root_finished = self.finish_root_reaper_if_ready();
            let reader_finished = self.finish_reader_if_ready();
            if root_finished && reader_finished {
                self.finish_resources();
                self.phase.store(PHASE_STOPPED, Ordering::Release);
                return self.current_report(false);
            }
            thread::sleep(POLL_INTERVAL);
        }
        self.detach_unfinished_root_reaper();
        self.detach_unfinished_reader();
        self.finish_resources();
        self.phase.store(PHASE_STOPPED, Ordering::Release);
        self.current_report(true)
    }

    fn finish_reader_if_ready(&self) -> bool {
        let mut reader = match self.reader.try_lock() {
            Ok(reader) => reader,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        if reader.is_none() {
            return true;
        }
        if !reader.as_ref().is_some_and(JoinHandle::is_finished) {
            return false;
        }
        if let Some(handle) = reader.take() {
            let _ = handle.join();
        }
        true
    }

    fn finish_root_reaper_if_ready(&self) -> bool {
        let mut root_reaper = match self.root_reaper.try_lock() {
            Ok(root_reaper) => root_reaper,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        if root_reaper.is_none() {
            return true;
        }
        if !root_reaper.as_ref().is_some_and(JoinHandle::is_finished) {
            return false;
        }
        if let Some(handle) = root_reaper.take() {
            let _ = handle.join();
        }
        true
    }

    fn detach_unfinished_root_reaper(&self) {
        let mut root_reaper = match self.root_reaper.try_lock() {
            Ok(root_reaper) => root_reaper,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        root_reaper.take();
    }

    fn detach_unfinished_reader(&self) {
        let mut reader = match self.reader.try_lock() {
            Ok(reader) => reader,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        reader.take();
    }

    fn finish_resources(&self) {
        if let Ok(mut input) = self.input.try_lock() {
            input.take();
        }
        if let Ok(mut master) = self.master.try_lock() {
            master.take();
        }
    }

    fn current_report(&self, deadline_reached: bool) -> ShutdownReport {
        ShutdownReport {
            root_reaped: self.root_reaped.load(Ordering::Acquire),
            output_reader_finished: self.reader_done.load(Ordering::Acquire),
            forced: self.forced.load(Ordering::Acquire),
            deadline_reached,
        }
    }
}

#[derive(Clone, Copy)]
enum OutputTerminal {
    Closed,
    ReadFailed,
    OutputLimit,
    Cancelled,
}

struct OutputQueue {
    max_buffered_bytes: usize,
    state: Mutex<OutputState>,
    changed: Condvar,
}

#[derive(Default)]
struct OutputState {
    frames: VecDeque<Zeroizing<Vec<u8>>>,
    buffered_bytes: usize,
    terminal: Option<OutputTerminal>,
}

impl OutputQueue {
    fn new(max_buffered_bytes: usize) -> Self {
        Self {
            max_buffered_bytes,
            state: Mutex::new(OutputState::default()),
            changed: Condvar::new(),
        }
    }

    fn push(&self, frame: Zeroizing<Vec<u8>>) -> bool {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        };
        if state.terminal.is_some() {
            return false;
        }
        let buffered_size = frame.len().saturating_add(OUTPUT_FRAME_OVERHEAD_BYTES);
        if state.buffered_bytes.saturating_add(buffered_size) > self.max_buffered_bytes {
            state.frames.clear();
            state.buffered_bytes = 0;
            state.terminal = Some(OutputTerminal::OutputLimit);
            self.changed.notify_all();
            return false;
        }
        state.buffered_bytes = state.buffered_bytes.saturating_add(buffered_size);
        state.frames.push_back(frame);
        self.changed.notify_one();
        true
    }

    fn finish(&self, terminal: OutputTerminal) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        };
        if state.terminal.is_none() {
            state.terminal = Some(terminal);
        }
        self.changed.notify_all();
    }

    fn cancel(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        };
        state.frames.clear();
        state.buffered_bytes = 0;
        state.terminal = Some(OutputTerminal::Cancelled);
        self.changed.notify_all();
    }

    fn try_receive(&self) -> Result<Option<Zeroizing<Vec<u8>>>, ProviderProcessError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProviderProcessError::OutputClosed)?;
        if let Some(frame) = state.frames.pop_front() {
            state.buffered_bytes = state
                .buffered_bytes
                .saturating_sub(frame.len().saturating_add(OUTPUT_FRAME_OVERHEAD_BYTES));
            return Ok(Some(frame));
        }
        match state.terminal {
            Some(terminal) => Err(terminal.into()),
            None => Ok(None),
        }
    }

    fn receive_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Zeroizing<Vec<u8>>, ProviderProcessError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(ProviderProcessError::TimedOut)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProviderProcessError::OutputClosed)?;
        loop {
            if let Some(frame) = state.frames.pop_front() {
                state.buffered_bytes = state
                    .buffered_bytes
                    .saturating_sub(frame.len().saturating_add(OUTPUT_FRAME_OVERHEAD_BYTES));
                return Ok(frame);
            }
            if let Some(terminal) = state.terminal {
                return Err(terminal.into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ProviderProcessError::TimedOut);
            }
            let (next, wait) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| ProviderProcessError::OutputClosed)?;
            state = next;
            if wait.timed_out() && state.frames.is_empty() && state.terminal.is_none() {
                return Err(ProviderProcessError::TimedOut);
            }
        }
    }
}

impl From<OutputTerminal> for ProviderProcessError {
    fn from(terminal: OutputTerminal) -> Self {
        match terminal {
            OutputTerminal::Closed | OutputTerminal::ReadFailed => Self::OutputClosed,
            OutputTerminal::OutputLimit => Self::OutputLimit,
            OutputTerminal::Cancelled => Self::Cancelled,
        }
    }
}

struct FailedStartGuard {
    group_id: libc::pid_t,
    root: Option<RootChild>,
}

impl FailedStartGuard {
    fn take(&mut self) -> RootChild {
        self.root.take().expect("start guard must own a root")
    }
}

impl Drop for FailedStartGuard {
    fn drop(&mut self) {
        let Some(mut root) = self.root.take() else {
            return;
        };
        signal_group(self.group_id, libc::SIGKILL);
        root.kill_direct();
        let gate = Arc::new(RootReapGate::default());
        gate.allow();
        if let Err(mut root) = spawn_root_reaper(root, Arc::new(AtomicBool::new(false)), gate) {
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            while Instant::now() < deadline {
                if root.try_wait().unwrap_or(false) {
                    return;
                }
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

fn build_piped_process(
    command: ProviderCommand,
    output_mode: ProviderOutputMode,
    observer: Option<OutputObserver>,
    cancelled: &AtomicBool,
) -> Result<ProcessControl, ProviderProcessError> {
    validate_output_mode(output_mode)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(ProviderProcessError::SupervisorStopping);
    }
    let mut command = command.std_command();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| ProviderProcessError::StartFailed)?;
    let group_id = match valid_group_id(child.id()) {
        Ok(group_id) => group_id,
        Err(error) => {
            cleanup_root_without_group(RootChild::Piped(child));
            return Err(error);
        }
    };
    let Some(stdin) = child.stdin.take() else {
        cleanup_failed_root(group_id, RootChild::Piped(child));
        return Err(ProviderProcessError::StartFailed);
    };
    let Some(stdout) = child.stdout.take() else {
        cleanup_failed_root(group_id, RootChild::Piped(child));
        return Err(ProviderProcessError::StartFailed);
    };
    let mut guard = FailedStartGuard {
        group_id,
        root: Some(RootChild::Piped(child)),
    };
    if cancelled.load(Ordering::Acquire) {
        return Err(ProviderProcessError::SupervisorStopping);
    }
    let input_fd = stdin.as_raw_fd();
    let output_fd = stdout.as_raw_fd();
    build_process_control(
        group_id,
        guard.take(),
        Box::new(stdin),
        input_fd,
        output_fd,
        Box::new(stdout),
        None,
        output_mode,
        observer,
        cancelled,
    )
}

fn build_pty_process(
    command: ProviderCommand,
    size: PtySize,
    output_mode: ProviderOutputMode,
    observer: Option<OutputObserver>,
    cancelled: &AtomicBool,
) -> Result<ProcessControl, ProviderProcessError> {
    validate_output_mode(output_mode)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(ProviderProcessError::SupervisorStopping);
    }
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|_| ProviderProcessError::StartFailed)?;
    let child = pair
        .slave
        .spawn_command(command.pty_command())
        .map_err(|_| ProviderProcessError::StartFailed)?;
    drop(pair.slave);
    let group_id = match child.process_id().and_then(|id| valid_group_id(id).ok()) {
        Some(group_id) => group_id,
        None => {
            cleanup_root_without_group(RootChild::Pty(child));
            return Err(ProviderProcessError::StartFailed);
        }
    };
    let mut guard = FailedStartGuard {
        group_id,
        root: Some(RootChild::Pty(child)),
    };
    if cancelled.load(Ordering::Acquire) {
        return Err(ProviderProcessError::SupervisorStopping);
    }
    let writer = pair
        .master
        .take_writer()
        .map_err(|_| ProviderProcessError::StartFailed)?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| ProviderProcessError::StartFailed)?;
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or(ProviderProcessError::StartFailed)?;
    build_process_control(
        group_id,
        guard.take(),
        writer,
        master_fd,
        master_fd,
        reader,
        Some(pair.master),
        output_mode,
        observer,
        cancelled,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_process_control(
    group_id: libc::pid_t,
    root: RootChild,
    writer: Box<dyn Write + Send>,
    input_fd: RawFd,
    output_fd: RawFd,
    reader: Box<dyn Read + Send>,
    master: Option<Box<dyn MasterPty + Send>>,
    output_mode: ProviderOutputMode,
    observer: Option<OutputObserver>,
    cancelled: &AtomicBool,
) -> Result<ProcessControl, ProviderProcessError> {
    let mut guard = FailedStartGuard {
        group_id,
        root: Some(root),
    };
    set_nonblocking(input_fd).map_err(|_| ProviderProcessError::StartFailed)?;
    if output_fd != input_fd {
        set_nonblocking(output_fd).map_err(|_| ProviderProcessError::StartFailed)?;
    }
    let input_poll_fd = duplicate_fd(input_fd).map_err(|_| ProviderProcessError::StartFailed)?;
    let output_poll_fd = duplicate_fd(output_fd).map_err(|_| ProviderProcessError::StartFailed)?;
    let (cancel_read, cancel_write) =
        UnixStream::pair().map_err(|_| ProviderProcessError::StartFailed)?;
    let reader_cancel = cancel_read
        .try_clone()
        .map_err(|_| ProviderProcessError::StartFailed)?;
    let output = Arc::new(OutputQueue::new(output_mode.max_buffered_bytes()));
    if cancelled.load(Ordering::Acquire) {
        return Err(ProviderProcessError::SupervisorStopping);
    }
    let root_reaped = Arc::new(AtomicBool::new(false));
    let root_reap_gate = Arc::new(RootReapGate::default());
    let phase = Arc::new(AtomicU8::new(PHASE_RUNNING));
    let root_reaper = match spawn_root_reaper(
        guard.take(),
        Arc::clone(&root_reaped),
        Arc::clone(&root_reap_gate),
    ) {
        Ok(root_reaper) => root_reaper,
        Err(root) => {
            guard.root = Some(root);
            return Err(ProviderProcessError::StartFailed);
        }
    };
    let reader_output = Arc::clone(&output);
    let reader_reap_gate = Arc::clone(&root_reap_gate);
    let reader_phase = Arc::clone(&phase);
    let reader_done = Arc::new(AtomicBool::new(false));
    let reader_completion = Arc::clone(&reader_done);
    let reader_result = thread::Builder::new()
        .name("provider-process-output".to_owned())
        .spawn(move || {
            read_output(OutputWorker {
                reader,
                poll_fd: output_poll_fd,
                cancel_read: reader_cancel,
                output_mode,
                output: reader_output,
                observer,
                group_id,
                root_reap_gate: reader_reap_gate,
                phase: reader_phase,
            });
            reader_completion.store(true, Ordering::Release);
        });
    let reader = match reader_result {
        Ok(reader) => reader,
        Err(_) => {
            kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
            let _ = cancel_write.shutdown(std::net::Shutdown::Both);
            output.cancel();
            drop(root_reaper);
            return Err(ProviderProcessError::StartFailed);
        }
    };

    Ok(ProcessControl {
        group_id,
        phase,
        cancelled: AtomicBool::new(false),
        forced: AtomicBool::new(false),
        root_reaped,
        root_reap_gate,
        reader_done,
        cancel_write,
        input: Mutex::new(Some(ProcessInput {
            writer,
            poll_fd: input_poll_fd,
            cancel_read,
        })),
        output,
        root_reaper: Mutex::new(Some(root_reaper)),
        master: Mutex::new(master),
        reader: Mutex::new(Some(reader)),
        shutdown_gate: Mutex::new(()),
    })
}

fn validate_output_mode(output_mode: ProviderOutputMode) -> Result<(), ProviderProcessError> {
    output_mode
        .is_valid()
        .then_some(())
        .ok_or(ProviderProcessError::StartFailed)
}

fn valid_group_id(id: u32) -> Result<libc::pid_t, ProviderProcessError> {
    let id = libc::pid_t::try_from(id).map_err(|_| ProviderProcessError::StartFailed)?;
    if id <= 1 || id == unsafe { libc::getpgrp() } {
        return Err(ProviderProcessError::StartFailed);
    }
    Ok(id)
}

fn cleanup_failed_root(group_id: libc::pid_t, root: RootChild) {
    drop(FailedStartGuard {
        group_id,
        root: Some(root),
    });
}

fn cleanup_root_without_group(mut root: RootChild) {
    root.kill_direct();
    let gate = Arc::new(RootReapGate::default());
    gate.allow();
    if let Err(mut root) = spawn_root_reaper(root, Arc::new(AtomicBool::new(false)), gate) {
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            if root.try_wait().unwrap_or(false) {
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

struct OutputWorker {
    reader: Box<dyn Read + Send>,
    poll_fd: OwnedFd,
    cancel_read: UnixStream,
    output_mode: ProviderOutputMode,
    output: Arc<OutputQueue>,
    observer: Option<OutputObserver>,
    group_id: libc::pid_t,
    root_reap_gate: Arc<RootReapGate>,
    phase: Arc<AtomicU8>,
}

fn read_output(worker: OutputWorker) {
    let OutputWorker {
        mut reader,
        poll_fd,
        cancel_read,
        output_mode,
        output,
        observer,
        group_id,
        root_reap_gate,
        phase,
    } = worker;
    let read_size = match output_mode {
        ProviderOutputMode::Lines { max_line_bytes, .. } => {
            LINE_READ_BYTES.min(max_line_bytes).max(1)
        }
        ProviderOutputMode::Chunks { chunk_bytes, .. } => chunk_bytes,
    };
    let mut read_buffer = Zeroizing::new(vec![0_u8; read_size]);
    let mut line = Zeroizing::new(Vec::new());
    loop {
        match wait_for_fd(
            poll_fd.as_raw_fd(),
            cancel_read.as_raw_fd(),
            libc::POLLIN,
            None,
        ) {
            Ok(()) => {}
            Err(ProviderProcessError::Cancelled) => {
                output.finish(OutputTerminal::Cancelled);
                return;
            }
            Err(_) => {
                output.finish(OutputTerminal::ReadFailed);
                kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
                return;
            }
        }
        let read = match reader.read(&mut read_buffer) {
            Ok(0) => {
                if !line.is_empty()
                    && let ProviderOutputMode::Lines { .. } = output_mode
                {
                    let frame = std::mem::take(&mut *line);
                    push_output(
                        &output,
                        frame,
                        observer.as_ref(),
                        group_id,
                        &root_reap_gate,
                        &phase,
                    );
                }
                output.finish(OutputTerminal::Closed);
                kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
                return;
            }
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(_) => {
                output.finish(OutputTerminal::ReadFailed);
                kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
                return;
            }
        };
        match output_mode {
            ProviderOutputMode::Chunks { .. } => {
                if !push_output(
                    &output,
                    read_buffer[..read].to_vec(),
                    observer.as_ref(),
                    group_id,
                    &root_reap_gate,
                    &phase,
                ) {
                    return;
                }
            }
            ProviderOutputMode::Lines { max_line_bytes, .. } => {
                let mut start = 0;
                for index in 0..read {
                    if read_buffer[index] != b'\n' {
                        continue;
                    }
                    line.extend_from_slice(&read_buffer[start..index]);
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if line.len() > max_line_bytes {
                        output.finish(OutputTerminal::OutputLimit);
                        kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
                        return;
                    }
                    let frame = std::mem::take(&mut *line);
                    if !push_output(
                        &output,
                        frame,
                        observer.as_ref(),
                        group_id,
                        &root_reap_gate,
                        &phase,
                    ) {
                        return;
                    }
                    start = index.saturating_add(1);
                }
                line.extend_from_slice(&read_buffer[start..read]);
                if line.len() > max_line_bytes {
                    output.finish(OutputTerminal::OutputLimit);
                    kill_group_and_allow_reap(group_id, &root_reap_gate, &phase);
                    return;
                }
            }
        }
    }
}

fn push_output(
    output: &OutputQueue,
    frame: Vec<u8>,
    observer: Option<&OutputObserver>,
    group_id: libc::pid_t,
    root_reap_gate: &RootReapGate,
    phase: &AtomicU8,
) -> bool {
    let frame = Zeroizing::new(frame);
    if let Some(observer) = observer {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(&frame)));
    }
    if output.push(frame) {
        true
    } else {
        kill_group_and_allow_reap(group_id, root_reap_gate, phase);
        false
    }
}

fn kill_group_and_allow_reap(
    group_id: libc::pid_t,
    root_reap_gate: &RootReapGate,
    phase: &AtomicU8,
) {
    if phase.fetch_max(PHASE_KILL_SENT, Ordering::AcqRel) < PHASE_KILL_SENT {
        signal_group(group_id, libc::SIGKILL);
    }
    root_reap_gate.allow();
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

fn wait_for_fd(
    data_fd: RawFd,
    cancel_fd: RawFd,
    event: libc::c_short,
    deadline: Option<Instant>,
) -> Result<(), ProviderProcessError> {
    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ProviderProcessError::TimedOut);
                }
                remaining.as_millis().clamp(1, i32::MAX as u128) as libc::c_int
            }
            None => -1,
        };
        let mut descriptors = [
            libc::pollfd {
                fd: cancel_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: data_fd,
                events: event,
                revents: 0,
            },
        ];
        let result =
            unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
        if result == 0 {
            return Err(ProviderProcessError::TimedOut);
        }
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ProviderProcessError::InputUnavailable);
        }
        if descriptors[0].revents != 0 {
            return Err(ProviderProcessError::Cancelled);
        }
        if descriptors[1].revents != 0 {
            return Ok(());
        }
    }
}

fn signal_group(group_id: libc::pid_t, signal: libc::c_int) {
    if group_id <= 1 || group_id == unsafe { libc::getpgrp() } {
        return;
    }
    let _ = unsafe { libc::kill(-group_id, signal) };
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    fn lines() -> ProviderOutputMode {
        ProviderOutputMode::Lines {
            max_line_bytes: 4 * 1024,
            max_buffered_bytes: 64 * 1024,
        }
    }

    fn shell(script: &str) -> ProviderCommand {
        let mut command = ProviderCommand::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn descendant_pid(process: &ProviderProcess) -> libc::pid_t {
        let output = process.receive_timeout(TEST_TIMEOUT).unwrap();
        std::str::from_utf8(&output)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn process_exists(pid: libc::pid_t) -> bool {
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn assert_process_stops(pid: libc::pid_t) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while process_exists(pid) && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(!process_exists(pid), "the descendant process must stop");
    }

    #[test]
    fn piped_shutdown_closes_descendant_owned_output() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_piped(
                shell("sleep 30 & printf '%s\\n' \"$!\"; exit 0"),
                lines(),
                None,
            )
            .unwrap();
        let pid = descendant_pid(&process);

        let started = Instant::now();
        let report = process.shutdown();

        assert!(started.elapsed() < TEST_TIMEOUT);
        assert!(report.root_reaped);
        assert!(report.output_reader_finished);
        assert!(!report.deadline_reached);
        assert_process_stops(pid);
    }

    #[test]
    fn pty_shutdown_closes_descendant_owned_output() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_pty(
                shell("sleep 30 & printf '%s\\n' \"$!\"; wait"),
                PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                },
                lines(),
                None,
            )
            .unwrap();
        let pid = descendant_pid(&process);

        let started = Instant::now();
        let report = process.shutdown();

        assert!(started.elapsed() < TEST_TIMEOUT);
        assert!(report.root_reaped);
        assert!(report.output_reader_finished);
        assert!(!report.deadline_reached);
        assert_process_stops(pid);
    }

    #[test]
    fn shutdown_escalates_for_a_term_resistant_tree() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_piped(
                shell(
                    "trap '' TERM; /bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' & printf '%s\\n' \"$!\"; wait",
                ),
                lines(),
                None,
            )
            .unwrap();
        let pid = descendant_pid(&process);

        let report = process.shutdown();

        assert!(report.forced);
        assert!(report.root_reaped);
        assert!(report.output_reader_finished);
        assert_process_stops(pid);
    }

    #[test]
    fn shutdown_is_idempotent() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();

        let first = process.shutdown();
        let started = Instant::now();
        let second = process.shutdown();

        assert!(first.root_reaped);
        assert!(second.root_reaped);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn cancelling_active_processes_allows_a_fresh_process() {
        let supervisor = ProviderProcessSupervisor::default();
        let active = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();

        let summary = supervisor.cancel_active();

        assert_eq!(summary.process_count, 1);
        assert_eq!(
            active.receive_timeout(Duration::from_millis(50)),
            Err(ProviderProcessError::Cancelled)
        );
        let fresh = supervisor
            .spawn_piped(shell("printf 'fresh\\n'"), lines(), None)
            .unwrap();
        assert_eq!(
            fresh
                .receive_timeout(Duration::from_secs(1))
                .unwrap()
                .as_slice(),
            b"fresh"
        );
    }

    #[test]
    fn a_full_output_queue_does_not_block_drop() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_piped(
                shell("while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done"),
                ProviderOutputMode::Chunks {
                    chunk_bytes: 128,
                    max_buffered_bytes: 160,
                },
                None,
            )
            .unwrap();
        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        drop(process);

        assert!(started.elapsed() < TEST_TIMEOUT);
    }

    #[test]
    fn blocked_input_respects_its_deadline_and_does_not_block_drop() {
        let supervisor = ProviderProcessSupervisor::default();
        let process = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();
        let input = vec![b'x'; 4 * 1024 * 1024];

        let started = Instant::now();
        let result = process.write_all(&input, Duration::from_millis(50));
        drop(process);

        assert_eq!(result, Err(ProviderProcessError::TimedOut));
        assert!(started.elapsed() < TEST_TIMEOUT);
    }

    #[test]
    fn supervisor_stops_all_registered_processes_with_one_budget() {
        let supervisor = ProviderProcessSupervisor::default();
        let first = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();
        let second = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();

        let started = Instant::now();
        let summary = supervisor.shutdown_all();

        assert!(started.elapsed() < TEST_TIMEOUT);
        assert_eq!(summary.process_count, 2);
        assert_eq!(summary.deadline_count, 0);
        assert!(first.shutdown().root_reaped);
        assert!(second.shutdown().root_reaped);
        assert!(matches!(
            supervisor.spawn_piped(shell("true"), lines(), None),
            Err(ProviderProcessError::SupervisorStopping)
        ));
    }

    #[test]
    fn shutdown_does_not_wait_for_a_pending_process_start() {
        let supervisor = ProviderProcessSupervisor::default();
        let existing = supervisor
            .spawn_piped(shell("sleep 30"), lines(), None)
            .unwrap();
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_supervisor = supervisor.clone();
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            worker_supervisor.spawn_registered(|cancelled| {
                worker_started.wait();
                worker_release.wait();
                if cancelled.load(Ordering::Acquire) {
                    return Err(ProviderProcessError::SupervisorStopping);
                }
                build_piped_process(shell("sleep 30"), lines(), None, cancelled)
            })
        });
        started.wait();

        let shutdown_started = Instant::now();
        let summary = supervisor.shutdown_all();
        let shutdown_elapsed = shutdown_started.elapsed();
        release.wait();
        let pending_result = worker.join().unwrap();

        assert!(shutdown_elapsed < TEST_TIMEOUT);
        assert_eq!(summary.process_count, 1);
        assert!(matches!(
            pending_result,
            Err(ProviderProcessError::SupervisorStopping)
        ));
        assert!(existing.shutdown().root_reaped);
    }
}
