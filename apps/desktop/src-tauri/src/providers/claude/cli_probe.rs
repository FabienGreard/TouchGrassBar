//! Claude CLI `/usage` observation.
//!
//! Terminal output is bounded, reduced in native memory, and discarded. Only
//! the two provider quota windows leave this module.

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration as StdDuration, Instant},
};

use chrono::{Datelike, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use portable_pty::PtySize;
use time::OffsetDateTime;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use super::ClaudeQuotaObservation;
use crate::providers::process::{
    ProviderCommand, ProviderOutputMode, ProviderProcess, ProviderProcessError,
    ProviderProcessSupervisor,
};

const MAX_CLI_OUTPUT_BYTES: usize = 1024 * 1024;
const CLI_OUTPUT_CHUNK_BYTES: usize = 8 * 1024;
const STARTUP_DELAY: StdDuration = StdDuration::from_secs(2);
const SAFE_PROMPT_INPUT_DELAY: StdDuration = StdDuration::from_secs(1);
pub(super) const PROBE_SESSION_MARKER: &str = ".touchgrassbar-claude-probe-session";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProbeFailure {
    Cancelled,
    Unavailable,
}

#[derive(Clone, Copy)]
enum ProbeCompletionStage {
    Timeout,
    OutputLimit,
    OutputClosed,
}

impl ProbeCompletionStage {
    const fn name(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::OutputLimit => "output_limit",
            Self::OutputClosed => "output_closed",
        }
    }
}

fn probe_event(event: &'static str) {
    super::debug_event(event);
}

fn finish_capture(
    partial: Option<ClaudeQuotaObservation>,
    stage: ProbeCompletionStage,
) -> Result<ClaudeQuotaObservation, ProbeFailure> {
    if let Some(observation) = partial {
        super::debug_event(&format!("cli_probe_partial stage={}", stage.name()));
        return Ok(observation);
    }
    super::debug_event(&format!("cli_probe_failed stage={}", stage.name()));
    Err(ProbeFailure::Unavailable)
}

#[derive(Default)]
struct AcceptedSafePrompts {
    legacy_folder_trust: bool,
    quick_safety_check: bool,
    ready_to_code: bool,
    continue_prompt: bool,
}

pub(super) fn probe_usage(
    processes: &ProviderProcessSupervisor,
    executable: &Path,
    probe_directory: &Path,
    observed_at: OffsetDateTime,
    timeout: StdDuration,
    cancelled: &dyn Fn() -> bool,
) -> Result<ClaudeQuotaObservation, ProbeFailure> {
    let session_id = probe_session_id().map_err(|()| ProbeFailure::Unavailable)?;
    if !cleanup_probe_session_artifacts(probe_directory) {
        probe_event("cli_probe_failed stage=cleanup_pending");
        return Err(ProbeFailure::Unavailable);
    }
    prepare_probe_directory(probe_directory, session_id).map_err(|()| ProbeFailure::Unavailable)?;
    let _cleanup = ProbeCleanup(probe_directory.to_path_buf());

    let mut command = ProviderCommand::new(executable);
    command.args([
        "--allowed-tools",
        "",
        "--strict-mcp-config",
        "--settings",
        "{\"disableDeepLinkRegistration\":\"disable\"}",
        "--session-id",
        session_id,
    ]);
    command.env("DISABLE_AUTOUPDATER", "1");
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("ANTHROPIC_") {
            command.env_remove(key);
        }
    }
    if let Some(parent) = executable.parent() {
        let mut paths = vec![parent.to_path_buf()];
        if let Some(current) = env::var_os("PATH") {
            paths.extend(env::split_paths(&current));
        }
        if let Ok(path) = env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    command.cwd(probe_directory);

    let process = processes
        .spawn_pty(
            command,
            PtySize {
                rows: 50,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            },
            ProviderOutputMode::Chunks {
                chunk_bytes: CLI_OUTPUT_CHUNK_BYTES,
                max_buffered_bytes: MAX_CLI_OUTPUT_BYTES,
            },
            None,
        )
        .map_err(|_| {
            probe_event("cli_probe_failed stage=process_start");
            ProbeFailure::Unavailable
        })?;

    let result = capture_usage_output(
        &process,
        observed_at,
        timeout.min(StdDuration::from_secs(30)),
        cancelled,
    );
    let _ = process.shutdown();
    result
}

fn capture_usage_output(
    process: &ProviderProcess,
    observed_at: OffsetDateTime,
    timeout: StdDuration,
    cancelled: &dyn Fn() -> bool,
) -> Result<ClaudeQuotaObservation, ProbeFailure> {
    let started_at = Instant::now();
    let deadline = started_at
        .checked_add(timeout)
        .ok_or(ProbeFailure::Unavailable)?;
    let safe_prompt_input_at = started_at + SAFE_PROMPT_INPUT_DELAY;
    let mut usage_input_at = started_at + STARTUP_DELAY;
    let mut usage_sent = false;
    let mut output = Zeroizing::new(Vec::new());
    let mut accepted_prompts = AcceptedSafePrompts::default();
    // The screen draws over several chunks, so a reading that resolves only one
    // window may simply be early. Keep the best one and use it only when the
    // probe runs out of time or output.
    let mut partial: Option<ClaudeQuotaObservation> = None;

    loop {
        if cancelled() {
            probe_event("cli_probe_failed stage=cancelled");
            return Err(ProbeFailure::Cancelled);
        }
        let now = Instant::now();
        if now >= deadline {
            return finish_capture(partial, ProbeCompletionStage::Timeout);
        }

        match process.receive_timeout(StdDuration::from_millis(100)) {
            Ok(chunk) => {
                if output.len().saturating_add(chunk.len()) > MAX_CLI_OUTPUT_BYTES {
                    return finish_capture(partial, ProbeCompletionStage::OutputLimit);
                }
                output.extend_from_slice(&chunk);
            }
            Err(ProviderProcessError::TimedOut) => {}
            Err(ProviderProcessError::Cancelled) if cancelled() => {
                probe_event("cli_probe_failed stage=cancelled");
                return Err(ProbeFailure::Cancelled);
            }
            Err(_) => {
                return finish_capture(partial, ProbeCompletionStage::OutputClosed);
            }
        }

        let now = Instant::now();
        if now >= safe_prompt_input_at
            && handle_safe_prompts(
                &output,
                process,
                &mut accepted_prompts,
                deadline.saturating_duration_since(now),
            )
            .map_err(|()| ProbeFailure::Unavailable)?
        {
            usage_sent = false;
            usage_input_at = now + STARTUP_DELAY;
        }
        if !usage_sent && now >= usage_input_at {
            process
                .write_all(b"/usage\r", deadline.saturating_duration_since(now))
                .map_err(|_| ProbeFailure::Unavailable)?;
            usage_sent = true;
        }
        if usage_sent && let Ok(observation) = parse_usage_output(&output, observed_at) {
            if observation.is_complete() {
                return Ok(observation);
            }
            partial = Some(observation);
        }
    }
}

fn handle_safe_prompts(
    output: &[u8],
    process: &ProviderProcess,
    accepted: &mut AcceptedSafePrompts,
    timeout: StdDuration,
) -> Result<bool, ()> {
    const QUICK_SAFETY_CHECK: &str = "quicksafetycheck:";
    const TRUST_FOLDER_CHOICE: &str = "yes,itrustthisfolder";
    let stripped = strip_ansi_escapes::strip(output);
    let normalized = String::from_utf8_lossy(&stripped)
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .collect::<String>();
    let mut responded = false;
    if !accepted.quick_safety_check
        && normalized.contains(QUICK_SAFETY_CHECK)
        && normalized.contains(TRUST_FOLDER_CHOICE)
    {
        process.write_all(b"\x1b[B\r", timeout).map_err(|_| ())?;
        accepted.quick_safety_check = true;
        responded = true;
    }
    responded |= respond_once(
        &normalized,
        "doyoutrustthefilesinthisfolder?",
        b"y\r",
        process,
        &mut accepted.legacy_folder_trust,
        timeout,
    )?;
    responded |= respond_once(
        &normalized,
        "readytocodehere?",
        b"\r",
        process,
        &mut accepted.ready_to_code,
        timeout,
    )?;
    responded |= respond_once(
        &normalized,
        "pressentertocontinue",
        b"\r",
        process,
        &mut accepted.continue_prompt,
        timeout,
    )?;
    Ok(responded)
}

fn respond_once(
    normalized: &str,
    prompt: &str,
    response: &[u8],
    process: &ProviderProcess,
    accepted: &mut bool,
    timeout: StdDuration,
) -> Result<bool, ()> {
    if *accepted || !normalized.contains(prompt) {
        return Ok(false);
    }
    process.write_all(response, timeout).map_err(|_| ())?;
    *accepted = true;
    Ok(true)
}

fn probe_session_id() -> Result<&'static str, ()> {
    static SESSION_ID: OnceLock<Option<Zeroizing<String>>> = OnceLock::new();
    SESSION_ID
        .get_or_init(|| {
            let mut bytes = [0_u8; 16];
            getrandom::fill(&mut bytes).ok()?;
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            let id = format!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                bytes[4],
                bytes[5],
                bytes[6],
                bytes[7],
                bytes[8],
                bytes[9],
                bytes[10],
                bytes[11],
                bytes[12],
                bytes[13],
                bytes[14],
                bytes[15]
            );
            bytes.fill(0);
            Some(Zeroizing::new(id))
        })
        .as_deref()
        .map(String::as_str)
        .ok_or(())
}

fn prepare_probe_directory(directory: &Path, session_id: &str) -> Result<(), ()> {
    fs::create_dir_all(directory).map_err(|_| ())?;
    let marker = directory.join(PROBE_SESSION_MARKER);
    fs::write(&marker, format!("{session_id}\n")).map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
        fs::set_permissions(marker, fs::Permissions::from_mode(0o600)).map_err(|_| ())?;
    }
    Ok(())
}

struct ProbeCleanup(PathBuf);

impl Drop for ProbeCleanup {
    fn drop(&mut self) {
        let _ = cleanup_probe_session_artifacts(&self.0);
    }
}

fn cleanup_probe_session_artifacts(probe_directory: &Path) -> bool {
    if let Some(config_root) = claude_config_root(probe_directory) {
        cleanup_probe_session_artifacts_at(probe_directory, &config_root)
    } else if probe_session_marker_exists(probe_directory) {
        cleanup_probe_directory(probe_directory, false);
        false
    } else {
        cleanup_probe_directory(probe_directory, true);
        true
    }
}

fn cleanup_probe_session_artifacts_at(probe_directory: &Path, config_root: &Path) -> bool {
    if !probe_session_marker_exists(probe_directory) {
        cleanup_probe_directory(probe_directory, true);
        return true;
    }
    let Some(session_id) = read_probe_session_marker(probe_directory) else {
        cleanup_probe_directory(probe_directory, false);
        return false;
    };
    let project_directory = config_root
        .join("projects")
        .join(claude_project_directory_name(probe_directory));
    let session_file = project_directory.join(format!("{}.jsonl", session_id.as_str()));
    let transcript_absent = match fs::symlink_metadata(&session_file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(&session_file).is_ok()
                && fs::symlink_metadata(&session_file)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        }
        Ok(_) => false,
    };
    if transcript_absent
        && fs::read_dir(&project_directory).is_ok_and(|mut entries| entries.next().is_none())
    {
        let _ = fs::remove_dir(project_directory);
    }
    cleanup_probe_directory(probe_directory, transcript_absent);
    transcript_absent
}

pub(super) enum ProbeTranscriptExclusion {
    None,
    Exact(PathBuf),
    UnsafeProject(PathBuf),
}

pub(super) fn probe_transcript_exclusion(
    probe_directory: &Path,
    config_root: &Path,
) -> ProbeTranscriptExclusion {
    if !probe_session_marker_exists(probe_directory) {
        return ProbeTranscriptExclusion::None;
    }
    let project_directory = config_root
        .join("projects")
        .join(claude_project_directory_name(probe_directory));
    match read_probe_session_marker(probe_directory) {
        Some(session_id) => ProbeTranscriptExclusion::Exact(
            project_directory.join(format!("{}.jsonl", session_id.as_str())),
        ),
        None => ProbeTranscriptExclusion::UnsafeProject(project_directory),
    }
}

#[cfg(test)]
pub(super) fn pending_probe_transcript(
    probe_directory: &Path,
    config_root: &Path,
) -> Option<PathBuf> {
    match probe_transcript_exclusion(probe_directory, config_root) {
        ProbeTranscriptExclusion::Exact(path) => Some(path),
        ProbeTranscriptExclusion::None | ProbeTranscriptExclusion::UnsafeProject(_) => None,
    }
}

fn probe_session_marker_exists(probe_directory: &Path) -> bool {
    fs::symlink_metadata(probe_directory.join(PROBE_SESSION_MARKER)).is_ok()
}

fn read_probe_session_marker(probe_directory: &Path) -> Option<Zeroizing<String>> {
    let marker = probe_directory.join(PROBE_SESSION_MARKER);
    let metadata = fs::symlink_metadata(&marker).ok()?;
    if !metadata.is_file() || metadata.len() > 64 {
        return None;
    }
    let value = Zeroizing::new(fs::read_to_string(marker).ok()?);
    let session_id = value.trim();
    is_valid_session_id(session_id).then(|| Zeroizing::new(session_id.to_owned()))
}

fn is_valid_session_id(session_id: &str) -> bool {
    session_id.len() == 36
        && session_id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn cleanup_probe_directory(probe_directory: &Path, remove_marker: bool) {
    if remove_marker {
        let _ = fs::remove_file(probe_directory.join(PROBE_SESSION_MARKER));
    }
    let settings_directory = probe_directory.join(".claude");
    let _ = fs::remove_file(settings_directory.join("settings.local.json"));
    let _ = fs::remove_dir(settings_directory);
    let _ = fs::remove_dir(probe_directory);
}

fn claude_config_root(probe_directory: &Path) -> Option<PathBuf> {
    if let Some(value) = env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty()) {
        let configured = PathBuf::from(value);
        return Some(if configured.is_absolute() {
            configured
        } else {
            probe_directory.join(configured)
        });
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"))
}

fn claude_project_directory_name(directory: &Path) -> String {
    const MAX_DIRECTORY_NAME_LENGTH: usize = 200;

    let path = directory.to_string_lossy().nfc().collect::<String>();
    let sanitized = path
        .encode_utf16()
        .map(|unit| {
            if unit <= u16::from(u8::MAX) && (unit as u8).is_ascii_alphanumeric() {
                char::from_u32(u32::from(unit)).unwrap_or('-')
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.len() <= MAX_DIRECTORY_NAME_LENGTH {
        return sanitized;
    }
    format!(
        "{}-{}",
        &sanitized[..MAX_DIRECTORY_NAME_LENGTH],
        javascript_hash_base36(&path)
    )
}

fn javascript_hash_base36(value: &str) -> String {
    let hash = value.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    let mut magnitude = if hash < 0 {
        -i64::from(hash)
    } else {
        i64::from(hash)
    };
    if magnitude == 0 {
        return "0".to_owned();
    }
    let mut encoded = Vec::new();
    while magnitude > 0 {
        let digit = (magnitude % 36) as u8;
        encoded.push(if digit < 10 {
            char::from(b'0' + digit)
        } else {
            char::from(b'a' + digit - 10)
        });
        magnitude /= 36;
    }
    encoded.into_iter().rev().collect()
}

/// The largest compacted span inspected after a `% used` counter. One window
/// renders its `Resets` clause immediately after the counter, and the bound
/// limits the reset text that the parser can inspect.
const MAX_WINDOW_SPAN_CHARS: usize = 160;

/// The compacted counter that opens every quota window.
const COUNTER_MARKER: &str = "%used";

/// The compacted clause that closes one quota window.
const RESET_MARKER: &str = "resets";

/// The compacted marker for the supported all-model weekly window.
///
/// This is a preference, never a gate. A plan can render a second, model-
/// specific weekly window that has the same shape as the supported one, and
/// publishing that percentage as the `Weekly limit` lane would report the wrong
/// provider-native limit. When the marker is readable it selects the supported
/// window; when a release renames it, selection falls back to shape alone.
const ALL_MODELS_MARKER: &str = "allmodels";

/// One quota window read from the screen, with the evidence used to prefer it
/// over another window of the same horizon.
struct QuotaCandidate {
    window: super::ClaudeRateLimitWindow,
    has_all_models_marker: bool,
}

impl QuotaCandidate {
    /// Whether this candidate should replace one already held for its horizon.
    ///
    /// A marked window always beats an unmarked one, so a model-specific weekly
    /// window cannot displace the supported all-model window. Between two
    /// windows with the same evidence the later one wins, because a redrawn
    /// terminal repeats the whole screen and the last rendering is current.
    fn supersedes(&self, current: &Self) -> bool {
        self.has_all_models_marker || !current.has_all_models_marker
    }
}

/// Reduce the `/usage` screen to the quota windows it shows.
///
/// The screen is presentation: headings, ordering, and decoration change
/// between Claude Code releases without the quota changing. This reads each
/// window by its own shape instead — a percentage counter followed by the reset
/// clause that belongs to it — and tells the two supported windows apart by
/// whether the reset names a day. A window this parser cannot read is left
/// absent rather than failing the whole probe.
pub(super) fn parse_usage_output(
    output: &[u8],
    observed_at: OffsetDateTime,
) -> Result<ClaudeQuotaObservation, ()> {
    let stripped = strip_ansi_escapes::strip(output);
    let text = String::from_utf8_lossy(&stripped);
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .collect::<String>();
    // Matching is case-insensitive. `to_ascii_lowercase` keeps every byte
    // offset, so an index found here also addresses `compact`, which keeps the
    // original case that the timezone name needs.
    let matchable = compact.to_ascii_lowercase();
    let counters = matchable
        .match_indices(COUNTER_MARKER)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    let mut five_hour: Option<QuotaCandidate> = None;
    let mut seven_day: Option<QuotaCandidate> = None;
    let mut window_start = 0;
    for (position, &counter_end) in counters.iter().enumerate() {
        // A window owns the text from the end of the previous window up to its
        // own counter, which is where its heading renders.
        let heading = &matchable[window_start..counter_end];
        window_start = counter_end;
        // Its reset clause cannot reach past the next counter. The marker must
        // also be the next compacted content, so a missing next-window counter
        // cannot let this counter borrow that window's reset.
        let next_counter = counters
            .get(position + 1)
            .copied()
            .unwrap_or(matchable.len());
        let Ok(used_percentage) = extract_used_percentage(&compact[..counter_end]) else {
            continue;
        };
        let tail = &compact[counter_end..next_counter];
        let span_end = tail
            .char_indices()
            .nth(MAX_WINDOW_SPAN_CHARS)
            .map_or(tail.len(), |(index, _)| index);
        // Locate the clause without case, then read it from the original text,
        // where the timezone name keeps its capitals.
        let Some((reset_start, reset_end)) =
            reset_bounds(&matchable[counter_end..next_counter][..span_end])
        else {
            continue;
        };
        if reset_start != COUNTER_MARKER.len() + RESET_MARKER.len() {
            continue;
        }
        let reset = &tail[reset_start..reset_end];
        // A weekly reset names the day it lands on. A session reset carries a
        // clock alone.
        let horizon = if reset_names_a_day(reset) {
            ResetHorizon::SevenDays
        } else {
            ResetHorizon::FiveHours
        };
        let Ok(resets_at) = parse_reset(reset, observed_at, horizon) else {
            continue;
        };
        let candidate = QuotaCandidate {
            window: super::ClaudeRateLimitWindow {
                resets_at,
                used_percentage,
            },
            has_all_models_marker: heading.contains(ALL_MODELS_MARKER),
        };
        let selected = match horizon {
            ResetHorizon::FiveHours => &mut five_hour,
            ResetHorizon::SevenDays => &mut seven_day,
        };
        if selected
            .as_ref()
            .is_none_or(|current| candidate.supersedes(current))
        {
            *selected = Some(candidate);
        }
    }

    if five_hour.is_none() && seven_day.is_none() {
        return Err(());
    }
    Ok(ClaudeQuotaObservation {
        observed_at,
        five_hour: five_hour.map(|candidate| candidate.window),
        seven_day: seven_day.map(|candidate| candidate.window),
    })
}

#[derive(Clone, Copy)]
enum ResetHorizon {
    FiveHours,
    SevenDays,
}

fn reset_names_a_day(reset: &str) -> bool {
    let Some(timezone_start) = reset.rfind('(') else {
        return false;
    };
    split_reset_at(&reset[..timezone_start])
        .is_some_and(|(date_text, _)| parse_month_day(date_text).is_ok())
}

fn extract_used_percentage(prefix: &str) -> Result<f64, ()> {
    let percent_start = prefix
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_digit() && *character != '.')
        .map_or(0, |(index, character)| index + character.len_utf8());
    let percentage = prefix[percent_start..].parse::<f64>().map_err(|_| ())?;
    (percentage.is_finite() && (0.0..=100.0).contains(&percentage))
        .then_some(percentage)
        .ok_or(())
}

/// The bounds of the `Resets ... (Zone)` clause inside one window.
///
/// The caller searches lowercased text and slices the original with the result,
/// which `to_ascii_lowercase` keeps aligned byte for byte.
fn reset_bounds(section: &str) -> Option<(usize, usize)> {
    let start = section.find(RESET_MARKER)? + RESET_MARKER.len();
    let end = section[start..].find(')')? + start + 1;
    Some((start, end))
}

/// Split a weekly reset into its date and clock halves, without case, so a
/// relabelled screen still parses.
fn split_reset_at(local_reset: &str) -> Option<(&str, &str)> {
    const SEPARATOR: &str = "at";

    let index = local_reset.to_ascii_lowercase().find(SEPARATOR)?;
    Some((
        &local_reset[..index],
        &local_reset[index + SEPARATOR.len()..],
    ))
}

fn parse_reset(reset: &str, observed_at: OffsetDateTime, horizon: ResetHorizon) -> Result<i64, ()> {
    let timezone_start = reset.rfind('(').ok_or(())?;
    let timezone_end = reset.rfind(')').ok_or(())?;
    if timezone_start >= timezone_end {
        return Err(());
    }
    let timezone = reset[timezone_start + 1..timezone_end]
        .parse::<Tz>()
        .map_err(|_| ())?;
    let local_reset = &reset[..timezone_start];
    let observed_utc = chrono::DateTime::<Utc>::from_timestamp(
        observed_at.unix_timestamp(),
        observed_at.nanosecond(),
    )
    .ok_or(())?;
    let observed_local = observed_utc.with_timezone(&timezone);
    match horizon {
        ResetHorizon::FiveHours => {
            let (hour, minute) = parse_clock(local_reset)?;
            let mut date = observed_local.date_naive();
            let mut local = local_datetime(timezone, date, hour, minute, observed_at)?;
            if local <= observed_at.unix_timestamp() {
                date = date.succ_opt().ok_or(())?;
                local = local_datetime(timezone, date, hour, minute, observed_at)?;
            }
            validate_reset(local, observed_at, 6 * 60 * 60)
        }
        ResetHorizon::SevenDays => {
            let (date_text, time_text) = split_reset_at(local_reset).ok_or(())?;
            let (month, day) = parse_month_day(date_text)?;
            let (hour, minute) = parse_clock(time_text)?;
            let mut year = observed_local.year();
            let mut date = NaiveDate::from_ymd_opt(year, month, day).ok_or(())?;
            let mut local = local_datetime(timezone, date, hour, minute, observed_at)?;
            if local <= observed_at.unix_timestamp() {
                year += 1;
                date = NaiveDate::from_ymd_opt(year, month, day).ok_or(())?;
                local = local_datetime(timezone, date, hour, minute, observed_at)?;
            }
            validate_reset(local, observed_at, 8 * 24 * 60 * 60)
        }
    }
}

fn parse_clock(value: &str) -> Result<(u32, u32), ()> {
    let lower = value.to_ascii_lowercase();
    let (clock, afternoon) = if let Some(clock) = lower.strip_suffix("am") {
        (clock, false)
    } else if let Some(clock) = lower.strip_suffix("pm") {
        (clock, true)
    } else {
        return Err(());
    };
    let (hour, minute) = clock.split_once(':').unwrap_or((clock, "0"));
    let mut hour = hour.parse::<u32>().map_err(|_| ())?;
    let minute = minute.parse::<u32>().map_err(|_| ())?;
    if !(1..=12).contains(&hour) || minute > 59 {
        return Err(());
    }
    if hour == 12 {
        hour = 0;
    }
    if afternoon {
        hour += 12;
    }
    Ok((hour, minute))
}

fn parse_month_day(value: &str) -> Result<(u32, u32), ()> {
    let month_text = value.get(..3).ok_or(())?.to_ascii_lowercase();
    let month = match month_text.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return Err(()),
    };
    let day = value.get(3..).ok_or(())?.parse::<u32>().map_err(|_| ())?;
    Ok((month, day))
}

fn local_datetime(
    timezone: Tz,
    date: NaiveDate,
    hour: u32,
    minute: u32,
    observed_at: OffsetDateTime,
) -> Result<i64, ()> {
    let naive = date.and_hms_opt(hour, minute, 0).ok_or(())?;
    match timezone.from_local_datetime(&naive) {
        LocalResult::Single(value) => Ok(value.timestamp()),
        LocalResult::Ambiguous(first, second) => [first.timestamp(), second.timestamp()]
            .into_iter()
            .filter(|candidate| *candidate > observed_at.unix_timestamp())
            .min()
            .ok_or(()),
        LocalResult::None => Err(()),
    }
}

fn validate_reset(
    reset: i64,
    observed_at: OffsetDateTime,
    maximum_seconds: i64,
) -> Result<i64, ()> {
    let delta = reset.checked_sub(observed_at.unix_timestamp()).ok_or(())?;
    (delta > 0 && delta <= maximum_seconds)
        .then_some(reset)
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use time::{Duration, format_description::well_known::Rfc3339};

    use super::*;
    use crate::sanitized::ProviderSnapshot;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct FixtureRoot(PathBuf);

    impl FixtureRoot {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "touchgrassbar-claude-probe-test-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn test_time() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-07T14:30:00Z", &Rfc3339).unwrap()
    }

    #[test]
    fn cli_usage_reduces_the_five_hour_and_weekly_windows() {
        let output = r#"
          Current session
          ▌ 42% used
          Resets 6:50pm (Europe/Paris)

          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert_eq!(observation.observed_at, test_time());
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
        assert_eq!(
            observation.five_hour.unwrap().resets_at,
            (test_time() + Duration::hours(2) + Duration::minutes(20)).unix_timestamp()
        );
        assert_eq!(
            observation.seven_day.unwrap().resets_at,
            OffsetDateTime::parse("2026-08-10T04:00:00Z", &Rfc3339)
                .unwrap()
                .unix_timestamp()
        );
    }

    #[test]
    fn cli_usage_survives_renamed_headings_and_reordered_sections() {
        // Headings, ordering, and decoration are presentation. A release that
        // changes them must not blank the Quota Lanes.
        let output = r#"
          Weekly allowance (every model)
          4% Used
          RESETS Aug 10 at 6am (Europe/Paris)

          This session
          ▌ 42% used
          Resets 6:50pm (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert!(observation.is_complete());
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[test]
    fn cli_usage_keeps_the_window_it_can_read_when_the_other_changes() {
        // One unreadable section leaves its lane out. Discarding the readable
        // window too would report no quota at all.
        let output = r#"
          Current session
          42% used
          Resets 6:50pm (Europe/Paris)

          Current week (all models)
          4 out of 100 credits remaining until the cycle rolls over
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert!(!observation.is_complete());
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert!(observation.seven_day.is_none());

        let ProviderSnapshot::Current { quota_lanes, .. } =
            observation.sanitized_snapshot(test_time()).unwrap()
        else {
            panic!("a readable window must still publish its lane");
        };
        assert_eq!(quota_lanes.len(), 1);
        assert_eq!(quota_lanes[0].label, "5-hour limit");
    }

    #[test]
    fn cli_usage_reads_the_latest_redraw_and_rejects_an_unreadable_screen() {
        // The terminal repeats the whole screen as it redraws. The last
        // readable rendering of each window is the current one.
        let redrawn = r#"
          Current session
          10% used
          Resets 6:50pm (Europe/Paris)
          Current week (all models)
          1% used
          Resets Aug 10 at 6am (Europe/Paris)

          Current session
          42% used
          Resets 6:50pm (Europe/Paris)
          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(redrawn, test_time()).unwrap();
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);

        for unreadable in [
            &b"Current session
Resets 6:50pm (Europe/Paris)"[..],
            &b"Current session
42% used
no reset clause here"[..],
            &b"Current session
42% used
Resets 6:50pm (Not/AZone)"[..],
            &b"Loading your usage..."[..],
        ] {
            assert!(parse_usage_output(unreadable, test_time()).is_err());
        }
    }

    #[test]
    fn cli_usage_prefers_the_all_model_weekly_window_over_a_model_specific_one() {
        // A plan can render a second weekly window for one model family. It has
        // the same shape as the supported all-model window, so shape alone
        // would publish the wrong provider-native limit.
        let output = r#"
          Current session
          42% used
          Resets 6:50pm (Europe/Paris)

          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)

          Current week (Opus)
          81% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[test]
    fn cli_usage_keeps_the_latest_all_model_window_across_a_redraw() {
        // The marker selects the supported window; between two windows carrying
        // it, the later rendering is the current one.
        let output = r#"
          Current week (all models)
          1% used
          Resets Aug 10 at 6am (Europe/Paris)
          Current week (Opus)
          70% used
          Resets Aug 10 at 6am (Europe/Paris)

          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)
          Current week (Opus)
          81% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[test]
    fn cli_usage_does_not_borrow_the_next_window_reset_clause() {
        // A window that lost its own reset clause must stay unread rather than
        // take the following window's and publish an invented pairing.
        let output = r#"
          Current session
          42% used

          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        let observation = parse_usage_output(output, test_time()).unwrap();
        assert!(observation.five_hour.is_none());
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[test]
    fn cli_usage_does_not_borrow_a_reset_when_the_next_counter_is_missing() {
        // The next window can retain its reset after its counter changes shape.
        // The preceding counter must not become that window's percentage.
        let output = r#"
          Current session
          42% used

          Current week (all models)
          Usage data unavailable
          Resets Aug 10 at 6am (Europe/Paris)
        "#
        .as_bytes();

        assert!(parse_usage_output(output, test_time()).is_err());
    }

    #[test]
    fn cli_usage_does_not_pair_a_counter_with_a_distant_reset() {
        // A counter and the reset clause that belongs to it render together. A
        // stray percentage far from any reset must not borrow another window's.
        let filler = "x".repeat(MAX_WINDOW_SPAN_CHARS + 40);
        let output = format!(
            "99% used
{filler}
Current session
42% used
Resets 6:50pm (Europe/Paris)"
        );

        let observation = parse_usage_output(output.as_bytes(), test_time()).unwrap();
        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
    }

    #[test]
    fn cli_usage_discards_terminal_and_account_content() {
        let output = br#"
          Current session
          42% used
          Resets 6:50pm (Europe/Paris)
          Account: REDACTED-ACCOUNT
          Provider output: REDACTED-PROVIDER-CONTENT

          Current week (all models)
          4% used
          Resets Aug 10 at 6am (Europe/Paris)
        "#;

        let observation = parse_usage_output(output, test_time()).unwrap();
        let sanitized = observation.sanitized_snapshot(test_time()).unwrap();
        let serialized = serde_json::to_string(&sanitized).unwrap();
        assert!(!serialized.contains("REDACTED-ACCOUNT"));
        assert!(!serialized.contains("REDACTED-PROVIDER-CONTENT"));
        assert!(!serialized.contains("Current session"));
    }

    #[test]
    fn cli_probe_paths_match_claude_project_encoding() {
        assert_eq!(
            claude_project_directory_name(Path::new(
                "/Users/test.name/tést_under/Library/Application Support/TouchGrassBar/ClaudeProbe"
            )),
            "-Users-test-name-t-st-under-Library-Application-Support-TouchGrassBar-ClaudeProbe"
        );
        assert_eq!(
            claude_project_directory_name(Path::new("/Users/test/emoji_😀/ClaudeProbe")),
            "-Users-test-emoji----ClaudeProbe"
        );
        let long_path = PathBuf::from(format!("/tmp/{}/ClaudeProbe", "segment_".repeat(40)));
        assert_eq!(
            claude_project_directory_name(&long_path),
            "-tmp-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-segment-seg-x9mpdi"
        );
    }

    #[test]
    fn cli_probe_cleanup_removes_only_its_session_file() {
        const OWNED_SESSION: &str = "11111111-1111-4111-8111-111111111111";
        const UNRELATED_SESSION: &str = "22222222-2222-4222-8222-222222222222";

        let fixture = FixtureRoot::new();
        let probe_directory = fixture.0.join("probe");
        let config_root = fixture.0.join("config");
        let project_directory = config_root
            .join("projects")
            .join(claude_project_directory_name(&probe_directory));
        fs::create_dir_all(&probe_directory).unwrap();
        fs::create_dir_all(&project_directory).unwrap();
        fs::write(
            probe_directory.join(PROBE_SESSION_MARKER),
            format!("{OWNED_SESSION}\n"),
        )
        .unwrap();
        fs::write(
            project_directory.join(format!("{OWNED_SESSION}.jsonl")),
            b"owned",
        )
        .unwrap();
        fs::write(
            project_directory.join(format!("{UNRELATED_SESSION}.jsonl")),
            b"unrelated",
        )
        .unwrap();

        assert!(cleanup_probe_session_artifacts_at(
            &probe_directory,
            &config_root
        ));

        assert!(
            !project_directory
                .join(format!("{OWNED_SESSION}.jsonl"))
                .exists()
        );
        assert!(
            project_directory
                .join(format!("{UNRELATED_SESSION}.jsonl"))
                .exists()
        );
        assert!(!probe_directory.exists());
    }

    #[test]
    fn cli_probe_cleanup_retains_its_marker_when_transcript_removal_fails() {
        const OWNED_SESSION: &str = "11111111-1111-4111-8111-111111111111";

        let fixture = FixtureRoot::new();
        let probe_directory = fixture.0.join("probe");
        let config_root = fixture.0.join("config");
        let project_directory = config_root
            .join("projects")
            .join(claude_project_directory_name(&probe_directory));
        let blocked_transcript = project_directory.join(format!("{OWNED_SESSION}.jsonl"));
        fs::create_dir_all(&probe_directory).unwrap();
        fs::create_dir_all(&blocked_transcript).unwrap();
        fs::write(
            probe_directory.join(PROBE_SESSION_MARKER),
            format!("{OWNED_SESSION}\n"),
        )
        .unwrap();

        assert!(!cleanup_probe_session_artifacts_at(
            &probe_directory,
            &config_root
        ));
        assert!(probe_directory.join(PROBE_SESSION_MARKER).is_file());
        assert!(blocked_transcript.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn cli_probe_accepts_the_quick_safety_check_before_requesting_usage() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = FixtureRoot::new();
        let executable = fixture.0.join("claude-cli");
        fs::write(
            &executable,
            br##"#!/bin/sh
printf '%s\n' \
  'Quick safety check: Is this a project you created or one you trust?' \
  '> No, exit' \
  '  Yes, I trust this folder'
IFS= read -r selection
expected_selection=$(printf '\033[B')
[ "$selection" = "$expected_selection" ] || exit 2
[ -f "${0}.ready" ] || exit 4
IFS= read -r command
[ "$command" = '/usage' ] || exit 3
printf '%s\n' \
  'Current session' \
  '42% used' \
  'Resets 6:50pm (Europe/Paris)' \
  'Current week (all models)' \
  '4% used' \
  'Resets Aug 10 at 6am (Europe/Paris)'
while :; do sleep 1; done
"##,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let probe_directory = fixture.0.join("probe");
        let processes = ProviderProcessSupervisor::default();
        let ready_marker = executable.with_extension("ready");
        let readiness = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(750));
            fs::write(ready_marker, b"ready").unwrap();
        });

        let observation = probe_usage(
            &processes,
            &executable,
            &probe_directory,
            test_time(),
            StdDuration::from_secs(5),
            &|| false,
        );
        readiness.join().unwrap();
        let observation = observation.unwrap();

        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[cfg(unix)]
    #[test]
    fn cli_probe_retries_usage_after_a_late_quick_safety_check() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = FixtureRoot::new();
        let executable = fixture.0.join("claude-cli");
        fs::write(
            &executable,
            br##"#!/bin/sh
IFS= read -r early_command
[ "$early_command" = '/usage' ] || exit 2
printf '%s\n' \
  'Quick safety check: Is this a project you created or one you trust?' \
  '> No, exit' \
  '  Yes, I trust this folder'
IFS= read -r selection
expected_selection=$(printf '\033[B')
[ "$selection" = "$expected_selection" ] || exit 3
IFS= read -r command
[ "$command" = '/usage' ] || exit 4
printf '%s\n' \
  'Current session' \
  '42% used' \
  'Resets 6:50pm (Europe/Paris)' \
  'Current week (all models)' \
  '4% used' \
  'Resets Aug 10 at 6am (Europe/Paris)'
while :; do sleep 1; done
"##,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let processes = ProviderProcessSupervisor::default();

        let observation = probe_usage(
            &processes,
            &executable,
            &fixture.0.join("probe"),
            test_time(),
            StdDuration::from_secs(7),
            &|| false,
        )
        .unwrap();

        assert_eq!(observation.five_hour.unwrap().used_percentage, 42.0);
        assert_eq!(observation.seven_day.unwrap().used_percentage, 4.0);
    }

    #[cfg(unix)]
    #[test]
    fn cli_probe_cancellation_terminates_the_child_process() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = FixtureRoot::new();
        let executable = fixture.0.join("blocking-cli");
        let pid_file = executable.with_extension("pid");
        fs::write(
            &executable,
            b"#!/bin/sh\nprintf '%s' \"$$\" > \"${0}.pid\"\nwhile :; do :; done\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let probe_directory = fixture.0.join("probe");
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        let (sender, receiver) = mpsc::channel();
        let task = thread::spawn(move || {
            let processes = ProviderProcessSupervisor::default();
            let result = probe_usage(
                &processes,
                &executable,
                &probe_directory,
                test_time(),
                StdDuration::from_secs(5),
                &|| task_cancelled.load(Ordering::Acquire),
            );
            let _ = sender.send(result);
        });

        let pid_deadline = Instant::now() + StdDuration::from_secs(2);
        let pid = loop {
            if let Ok(value) = fs::read_to_string(&pid_file)
                && let Ok(pid) = value.parse::<u32>()
            {
                break pid;
            }
            if Instant::now() >= pid_deadline {
                cancelled.store(true, Ordering::Release);
                let _ = receiver.recv_timeout(StdDuration::from_secs(2));
                let _ = task.join();
                panic!("probe child did not start");
            }
            thread::sleep(StdDuration::from_millis(10));
        };

        cancelled.store(true, Ordering::Release);
        let result = receiver
            .recv_timeout(StdDuration::from_secs(2))
            .expect("cancelled probe must return");
        task.join().unwrap();

        assert!(matches!(result, Err(ProbeFailure::Cancelled)));
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
}
