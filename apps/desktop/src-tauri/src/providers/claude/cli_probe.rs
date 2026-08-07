//! Claude CLI `/usage` observation.
//!
//! Terminal output is bounded, reduced in native memory, and discarded. Only
//! the two provider quota windows leave this module.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration as StdDuration, Instant},
};

use chrono::{Datelike, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use super::ClaudeQuotaObservation;

const MAX_CLI_OUTPUT_BYTES: usize = 1024 * 1024;
const STARTUP_DELAY: StdDuration = StdDuration::from_secs(2);
const ENTER_INTERVAL: StdDuration = StdDuration::from_millis(800);

fn probe_event(event: &'static str) {
    super::debug_event(event);
}

pub(super) fn probe_usage(
    executable: &Path,
    probe_directory: &Path,
    observed_at: OffsetDateTime,
    timeout: StdDuration,
) -> Result<ClaudeQuotaObservation, ()> {
    cleanup_probe_session_artifacts(probe_directory);
    prepare_probe_directory(probe_directory)?;
    let _cleanup = ProbeCleanup(probe_directory.to_path_buf());

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 50,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| {
            probe_event("cli_probe_failed stage=pty_open");
        })?;
    let mut command = CommandBuilder::new(executable);
    command.args([
        "--allowed-tools",
        "",
        "--strict-mcp-config",
        "--session-id",
        probe_session_id()?,
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

    let mut child = pair.slave.spawn_command(command).map_err(|_| {
        probe_event("cli_probe_failed stage=process_start");
    })?;
    drop(pair.slave);
    let mut writer = pair.master.take_writer().map_err(|_| {
        probe_event("cli_probe_failed stage=pty_writer");
    })?;
    let mut reader = pair.master.try_clone_reader().map_err(|_| {
        probe_event("cli_probe_failed stage=pty_reader");
    })?;
    let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(16);
    let reader_thread = thread::Builder::new()
        .name("claude-quota-cli-output".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
            }
            buffer.fill(0);
        })
        .map_err(|_| {
            probe_event("cli_probe_failed stage=reader_thread");
        })?;

    let result = capture_usage_output(
        child.as_mut(),
        writer.as_mut(),
        &receiver,
        observed_at,
        timeout.min(StdDuration::from_secs(30)),
    );
    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    drop(pair.master);
    let _ = reader_thread.join();
    result
}

fn capture_usage_output(
    child: &mut dyn portable_pty::Child,
    writer: &mut dyn Write,
    receiver: &mpsc::Receiver<Vec<u8>>,
    observed_at: OffsetDateTime,
    timeout: StdDuration,
) -> Result<ClaudeQuotaObservation, ()> {
    let started_at = Instant::now();
    let deadline = started_at.checked_add(timeout).ok_or(())?;
    let mut usage_sent = false;
    let mut next_enter = started_at + STARTUP_DELAY + ENTER_INTERVAL;
    let mut output = Zeroizing::new(Vec::new());
    let mut accepted_prompts = [false; 5];

    loop {
        let now = Instant::now();
        if now >= deadline {
            probe_event("cli_probe_failed stage=timeout");
            return Err(());
        }
        if !usage_sent && now.duration_since(started_at) >= STARTUP_DELAY {
            writer.write_all(b"/usage\r").map_err(|_| ())?;
            writer.flush().map_err(|_| ())?;
            usage_sent = true;
        }
        if usage_sent && now >= next_enter {
            writer.write_all(b"\r").map_err(|_| ())?;
            writer.flush().map_err(|_| ())?;
            next_enter = now + ENTER_INTERVAL;
        }
        if child.try_wait().map_err(|_| ())?.is_some() {
            probe_event("cli_probe_failed stage=process_exit");
            return Err(());
        }

        match receiver.recv_timeout(StdDuration::from_millis(100)) {
            Ok(chunk) => {
                if output.len().saturating_add(chunk.len()) > MAX_CLI_OUTPUT_BYTES {
                    probe_event("cli_probe_failed stage=output_limit");
                    return Err(());
                }
                output.extend_from_slice(&chunk);
                handle_safe_prompts(&output, writer, &mut accepted_prompts)?;
                if usage_sent && let Ok(observation) = parse_usage_output(&output, observed_at) {
                    return Ok(observation);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                probe_event("cli_probe_failed stage=output_closed");
                return Err(());
            }
        }
    }
}

fn handle_safe_prompts(
    output: &[u8],
    writer: &mut dyn Write,
    accepted: &mut [bool; 5],
) -> Result<(), ()> {
    const PROMPTS: [(&str, &[u8]); 5] = [
        ("doyoutrustthefilesinthisfolder?", b"y\r"),
        ("quicksafetycheck:", b"\r"),
        ("yes,itrustthisfolder", b"\r"),
        ("readytocodehere?", b"\r"),
        ("pressentertocontinue", b"\r"),
    ];
    let stripped = strip_ansi_escapes::strip(output);
    let normalized = String::from_utf8_lossy(&stripped)
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .collect::<String>();
    for (index, (prompt, response)) in PROMPTS.iter().enumerate() {
        if !accepted[index] && normalized.contains(prompt) {
            writer.write_all(response).map_err(|_| ())?;
            writer.flush().map_err(|_| ())?;
            accepted[index] = true;
        }
    }
    Ok(())
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

fn prepare_probe_directory(directory: &Path) -> Result<(), ()> {
    let settings_directory = directory.join(".claude");
    fs::create_dir_all(&settings_directory).map_err(|_| ())?;
    let settings = settings_directory.join("settings.local.json");
    fs::write(
        &settings,
        b"{\"disableDeepLinkRegistration\":\"disable\"}\n",
    )
    .map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
        fs::set_permissions(&settings_directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| ())?;
        fs::set_permissions(settings, fs::Permissions::from_mode(0o600)).map_err(|_| ())?;
    }
    Ok(())
}

struct ProbeCleanup(PathBuf);

impl Drop for ProbeCleanup {
    fn drop(&mut self) {
        cleanup_probe_session_artifacts(&self.0);
    }
}

fn cleanup_probe_session_artifacts(probe_directory: &Path) {
    let config_root = env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")));
    if let Some(config_root) = config_root {
        let project_directory = config_root
            .join("projects")
            .join(claude_project_directory_name(probe_directory));
        if let Ok(entries) = fs::read_dir(&project_directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
                    && entry.file_type().is_ok_and(|file_type| file_type.is_file())
                {
                    let _ = fs::remove_file(path);
                }
            }
        }
        if fs::read_dir(&project_directory).is_ok_and(|mut entries| entries.next().is_none()) {
            let _ = fs::remove_dir(project_directory);
        }
    }
    let settings_directory = probe_directory.join(".claude");
    let _ = fs::remove_file(settings_directory.join("settings.local.json"));
    let _ = fs::remove_dir(settings_directory);
    let _ = fs::remove_dir(probe_directory);
}

fn claude_project_directory_name(directory: &Path) -> String {
    let path = directory.to_string_lossy();
    path.encode_utf16()
        .map(|unit| {
            if unit <= u16::from(u8::MAX) && (unit as u8).is_ascii_alphanumeric() {
                char::from_u32(u32::from(unit)).unwrap_or('-')
            } else {
                '-'
            }
        })
        .take(200)
        .collect()
}

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
    let session_start = compact.rfind("Currentsession").ok_or(())?;
    let weekly_offset = compact[session_start..]
        .find("Currentweek(allmodels)")
        .ok_or(())?;
    let weekly_start = session_start + weekly_offset;
    let session = &compact[session_start..weekly_start];
    let weekly = &compact[weekly_start..];

    Ok(ClaudeQuotaObservation {
        observed_at,
        five_hour: quota_window(session, observed_at, ResetHorizon::FiveHours)?,
        seven_day: quota_window(weekly, observed_at, ResetHorizon::SevenDays)?,
    })
}

#[derive(Clone, Copy)]
enum ResetHorizon {
    FiveHours,
    SevenDays,
}

fn quota_window(
    section: &str,
    observed_at: OffsetDateTime,
    horizon: ResetHorizon,
) -> Result<super::ClaudeRateLimitWindow, ()> {
    let used_percentage = extract_used_percentage(section)?;
    let reset = extract_reset(section)?;
    let resets_at = parse_reset(reset, observed_at, horizon)?;
    Ok(super::ClaudeRateLimitWindow {
        resets_at,
        used_percentage,
    })
}

fn extract_used_percentage(section: &str) -> Result<f64, ()> {
    let percent_end = section.find("%used").ok_or(())?;
    let prefix = &section[..percent_end];
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

fn extract_reset(section: &str) -> Result<&str, ()> {
    let reset = section
        .split_once("Resets")
        .map(|(_, reset)| reset)
        .ok_or(())?;
    let timezone_end = reset.find(')').ok_or(())?;
    Ok(&reset[..=timezone_end])
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
            let (date_text, time_text) = local_reset.split_once("at").ok_or(())?;
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
    let (hour, minute) = clock.split_once(':').map_or((clock, "0"), |parts| parts);
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
    use time::{Duration, format_description::well_known::Rfc3339};

    use super::*;

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
        assert_eq!(observation.five_hour.used_percentage, 42.0);
        assert_eq!(observation.seven_day.used_percentage, 4.0);
        assert_eq!(
            observation.five_hour.resets_at,
            (test_time() + Duration::hours(2) + Duration::minutes(20)).unix_timestamp()
        );
        assert_eq!(
            observation.seven_day.resets_at,
            OffsetDateTime::parse("2026-08-10T04:00:00Z", &Rfc3339)
                .unwrap()
                .unix_timestamp()
        );
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
}
