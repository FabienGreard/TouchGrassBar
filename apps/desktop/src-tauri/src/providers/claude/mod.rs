//! Passive Claude quota capture through Claude Code's status-line input.
//!
//! Claude Code sends one JSON document to a configured status-line command.
//! The bridge keeps that document in native memory only, reduces it to the two
//! supported quota lanes, and then runs the user's prior status-line command.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration as StdDuration,
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
#[cfg(any(test, not(debug_assertions)))]
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

use super::{ProviderObservation, ProviderObservationAdapter};
use crate::sanitized::{
    Clock, CodingProvider, ProviderPresentation, ProviderSnapshot, QuotaLane, RefreshAttempt,
    RefreshFailure, RefreshTrigger,
};

const STATUS_LINE_ARGUMENT: &str = "--touchgrassbar-claude-status-line";
const CAPTURE_SCHEMA_MODULE: &str = "claude-quota-capture";
const CAPTURE_SCHEMA_VERSION: i64 = 1;
const MAX_STATUS_LINE_BYTES: u64 = 1024 * 1024;
const MAX_SESSION_ID_BYTES: usize = 256;
const RESPONSE_CURSOR_RETENTION_DAYS: i64 = 60;
const UPSTREAM_ARGUMENT: &str = "--touchgrassbar-upstream-hex";

#[cfg(debug_assertions)]
fn debug_event(event: &str) {
    eprintln!("[TouchGrassBar][claude-quota] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_event(_event: &str) {}

fn capture_stored_event() {
    eprintln!("[TouchGrassBar][claude-quota] capture_stored lane_count=2");
}

#[derive(Debug, Deserialize)]
struct ClaudeStatusLineInput {
    cost: ClaudeStatusLineCost,
    rate_limits: ClaudeRateLimits,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeStatusLineCost {
    total_api_duration_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeRateLimits {
    five_hour: ClaudeRateLimitWindow,
    seven_day: ClaudeRateLimitWindow,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ClaudeRateLimitWindow {
    resets_at: i64,
    used_percentage: f64,
}

impl ClaudeRateLimitWindow {
    fn validate(self, now: OffsetDateTime) -> Result<Self, ()> {
        let reset_at = OffsetDateTime::from_unix_timestamp(self.resets_at).map_err(|_| ())?;
        (self.used_percentage.is_finite()
            && (0.0..=100.0).contains(&self.used_percentage)
            && reset_at > now)
            .then_some(self)
            .ok_or(())
    }

    fn sanitized_lane(self, label: &str) -> Result<QuotaLane, ()> {
        let reset_at = OffsetDateTime::from_unix_timestamp(self.resets_at).map_err(|_| ())?;
        Ok(QuotaLane {
            label: label.to_owned(),
            unit: "percent".to_owned(),
            allowance: Some(100.0),
            remaining: Some(100.0 - self.used_percentage),
            reset_at: Some(reset_at.format(&Rfc3339).map_err(|_| ())?),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ClaudeQuotaObservation {
    observed_at: OffsetDateTime,
    five_hour: ClaudeRateLimitWindow,
    seven_day: ClaudeRateLimitWindow,
}

impl ClaudeQuotaObservation {
    fn sanitized_snapshot(&self, now: OffsetDateTime) -> Result<ProviderSnapshot, ()> {
        if self.observed_at > now {
            return Err(());
        }
        let five_hour = self.five_hour.validate(now)?;
        let seven_day = self.seven_day.validate(now)?;
        Ok(ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: self.observed_at.format(&Rfc3339).map_err(|_| ())?,
            quota_lanes: vec![
                five_hour.sanitized_lane("5-hour limit")?,
                seven_day.sanitized_lane("Weekly limit")?,
            ],
        })
    }
}

pub(crate) struct ClaudeProviderObservationAdapter {
    clock: Arc<dyn Clock>,
    database_path: Option<PathBuf>,
    notification_path: Option<PathBuf>,
    notification_listener_started: AtomicBool,
}

impl ClaudeProviderObservationAdapter {
    pub(crate) fn production(clock: Arc<dyn Clock>, database_path: Option<PathBuf>) -> Self {
        let notification_path = database_path.as_deref().map(notification_path);
        Self {
            clock,
            database_path,
            notification_path,
            notification_listener_started: AtomicBool::new(false),
        }
    }

    fn refresh_with_diagnostics(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
        mut report: impl FnMut(&'static str),
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        if attempt.is_local_usage_only() {
            report("refresh_skipped reason=local_usage_only");
            return Ok(None);
        }
        attempt.remaining()?;
        let Some(database_path) = self.database_path.as_deref() else {
            report("capture_unavailable reason=database_unavailable");
            return Ok(None);
        };
        let observation = match load_quota_observation(database_path) {
            Ok(Some(observation)) => observation,
            Ok(None) => {
                report("capture_unavailable reason=not_observed");
                return Ok(None);
            }
            Err(()) => {
                report("capture_unavailable reason=storage_unavailable");
                return Err(RefreshFailure::SourceUnavailable);
            }
        };
        attempt.remaining()?;
        if cached_observed_at(&cached.quota)
            .is_some_and(|cached_at| cached_at >= observation.observed_at)
        {
            report("capture_unchanged");
            return Ok(None);
        }
        let quota = match observation.sanitized_snapshot(self.clock.now()) {
            Ok(quota) => quota,
            Err(()) => {
                report("capture_unavailable reason=expired_or_invalid");
                return Err(RefreshFailure::SourceUnavailable);
            }
        };
        report("capture_loaded lane_count=2");
        Ok(Some(ProviderObservation {
            quota,
            usage: cached.usage.clone(),
        }))
    }
}

impl ProviderObservationAdapter for ClaudeProviderObservationAdapter {
    fn provider(&self) -> CodingProvider {
        CodingProvider::Claude
    }

    fn install_refresh_trigger(&self, trigger: RefreshTrigger) {
        let Some(path) = self.notification_path.clone() else {
            return;
        };
        if self
            .notification_listener_started
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        start_notification_listener(path, trigger);
    }

    fn refresh(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        self.refresh_with_diagnostics(cached, attempt, debug_event)
    }
}

fn cached_observed_at(snapshot: &ProviderSnapshot) -> Option<OffsetDateTime> {
    let observed_at = match snapshot {
        ProviderSnapshot::Current { observed_at, .. }
        | ProviderSnapshot::Stale { observed_at, .. } => observed_at,
        ProviderSnapshot::Unavailable { .. } => return None,
    };
    OffsetDateTime::parse(observed_at, &Rfc3339).ok()
}

fn notification_path(database_path: &Path) -> PathBuf {
    database_path.with_extension("claude-quota.sock")
}

#[cfg(unix)]
fn start_notification_listener(path: PathBuf, trigger: RefreshTrigger) {
    use std::os::{unix::fs::FileTypeExt, unix::net::UnixDatagram};

    let existing_is_socket = fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(true);
    if !existing_is_socket {
        return;
    }
    if path.exists() && fs::remove_file(&path).is_err() {
        return;
    }
    let Ok(socket) = UnixDatagram::bind(&path) else {
        return;
    };
    let _ = thread::Builder::new()
        .name("claude-quota-events".to_owned())
        .spawn(move || {
            let mut message = [0_u8; 1];
            while socket.recv(&mut message).is_ok() {
                trigger();
            }
        });
}

#[cfg(not(unix))]
fn start_notification_listener(_path: PathBuf, _trigger: RefreshTrigger) {}

#[cfg(unix)]
fn send_notification(path: &Path) {
    use std::os::unix::net::UnixDatagram;

    if let Ok(socket) = UnixDatagram::unbound() {
        let _ = socket.send_to(&[1], path);
    }
}

#[cfg(not(unix))]
fn send_notification(_path: &Path) {}

fn open_capture_database(path: &Path) -> Result<Connection, ()> {
    let mut connection = Connection::open(path).map_err(|_| ())?;
    connection
        .busy_timeout(StdDuration::from_millis(500))
        .map_err(|_| ())?;
    ensure_capture_schema(&mut connection, Some(path))?;
    Ok(connection)
}

fn capture_schema_version(connection: &Connection) -> Result<i64, ()> {
    let schema_table_exists = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type = 'table' AND name = 'touchgrassbar_schema_versions'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| ())?;
    if !schema_table_exists {
        return Ok(0);
    }
    connection
        .query_row(
            "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1",
            [CAPTURE_SCHEMA_MODULE],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|version| version.unwrap_or(0))
        .map_err(|_| ())
}

fn capture_backup_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!("sqlite3.claude-quota-v{source_version}.backup"))
}

fn capture_backup_partial_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!(
        "sqlite3.claude-quota-v{source_version}.backup.partial"
    ))
}

fn capture_backup_is_valid(connection: &Connection, source_version: i64) -> Result<bool, ()> {
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    Ok(integrity == "ok" && capture_schema_version(connection)? == source_version)
}

fn backup_capture_schema(
    connection: &Connection,
    path: &Path,
    source_version: i64,
) -> Result<(), ()> {
    let backup_path = capture_backup_path(path, source_version);
    if backup_path.exists() {
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| ())?;
        return capture_backup_is_valid(&backup, source_version)?
            .then_some(())
            .ok_or(());
    }
    let partial_path = capture_backup_partial_path(path, source_version);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| ())?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &partial_path, None)
        .map_err(|_| ())?;
    fs::File::open(&partial_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| ())?;
    let backup =
        Connection::open_with_flags(&partial_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| ())?;
    if !capture_backup_is_valid(&backup, source_version)? {
        return Err(());
    }
    drop(backup);
    fs::rename(partial_path, backup_path).map_err(|_| ())
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, ()> {
    connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|_| ())?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())
}

fn validate_capture_schema(connection: &Connection) -> Result<(), ()> {
    let expected = [
        (
            "claude_quota_observation",
            vec![
                "singleton",
                "observed_at",
                "five_hour_used_percentage",
                "five_hour_resets_at",
                "seven_day_used_percentage",
                "seven_day_resets_at",
            ],
        ),
        (
            "claude_response_cursors",
            vec!["session_id", "total_api_duration_ms", "observed_at"],
        ),
    ];
    expected
        .into_iter()
        .all(|(table, columns)| {
            table_columns(connection, table).is_ok_and(|actual| actual == columns)
        })
        .then_some(())
        .ok_or(())
}

fn ensure_capture_schema(
    connection: &mut Connection,
    database_path: Option<&Path>,
) -> Result<(), ()> {
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|_| ())?;
    let source_version = capture_schema_version(connection)?;
    if source_version > CAPTURE_SCHEMA_VERSION {
        return Err(());
    }
    if source_version == CAPTURE_SCHEMA_VERSION {
        return validate_capture_schema(connection);
    }
    if let Some(database_path) = database_path {
        backup_capture_schema(connection, database_path, source_version)?;
    }
    let transaction = connection.transaction().map_err(|_| ())?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
               module TEXT PRIMARY KEY,
               version INTEGER NOT NULL CHECK (version >= 1)
             );
             CREATE TABLE claude_quota_observation (
               singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
               observed_at TEXT NOT NULL,
               five_hour_used_percentage REAL NOT NULL
                 CHECK(five_hour_used_percentage >= 0 AND five_hour_used_percentage <= 100),
               five_hour_resets_at INTEGER NOT NULL CHECK(five_hour_resets_at > 0),
               seven_day_used_percentage REAL NOT NULL
                 CHECK(seven_day_used_percentage >= 0 AND seven_day_used_percentage <= 100),
               seven_day_resets_at INTEGER NOT NULL CHECK(seven_day_resets_at > 0)
             );
             CREATE TABLE claude_response_cursors (
               session_id TEXT PRIMARY KEY NOT NULL,
               total_api_duration_ms INTEGER NOT NULL CHECK(total_api_duration_ms > 0),
               observed_at TEXT NOT NULL
             );",
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO touchgrassbar_schema_versions(module, version) VALUES(?1, ?2)",
            params![CAPTURE_SCHEMA_MODULE, CAPTURE_SCHEMA_VERSION],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())?;
    validate_capture_schema(connection)
}

fn capture_status_line_payload(
    database_path: &Path,
    payload: &[u8],
    now: OffsetDateTime,
) -> Result<bool, ()> {
    let input: ClaudeStatusLineInput = serde_json::from_slice(payload).map_err(|_| ())?;
    if input.session_id.is_empty()
        || input.session_id.len() > MAX_SESSION_ID_BYTES
        || input.session_id.chars().any(char::is_control)
        || input.cost.total_api_duration_ms == 0
        || input.cost.total_api_duration_ms > i64::MAX as u64
    {
        return Err(());
    }
    let five_hour = input.rate_limits.five_hour.validate(now)?;
    let seven_day = input.rate_limits.seven_day.validate(now)?;
    let api_duration = i64::try_from(input.cost.total_api_duration_ms).map_err(|_| ())?;
    let observed_at = now
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| ())?;
    let mut connection = open_capture_database(database_path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| ())?;
    let cursor_cutoff = response_cursor_retention_cutoff(now)
        .format(&Rfc3339)
        .map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM claude_response_cursors WHERE observed_at < ?1",
            [&cursor_cutoff],
        )
        .map_err(|_| ())?;
    let previous_duration = transaction
        .query_row(
            "SELECT total_api_duration_ms FROM claude_response_cursors WHERE session_id = ?1",
            [&input.session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if previous_duration.is_some_and(|previous| previous >= api_duration) {
        transaction.commit().map_err(|_| ())?;
        return Ok(false);
    }
    let current_observed_at = transaction
        .query_row(
            "SELECT observed_at
             FROM claude_quota_observation
             WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if let Some(current_at) = current_observed_at {
        let current_at = OffsetDateTime::parse(&current_at, &Rfc3339).map_err(|_| ())?;
        if now <= current_at {
            store_response_cursor(&transaction, &input.session_id, api_duration, &observed_at)?;
            transaction.commit().map_err(|_| ())?;
            return Ok(false);
        }
    }
    store_response_cursor(&transaction, &input.session_id, api_duration, &observed_at)?;
    transaction
        .execute(
            "INSERT INTO claude_quota_observation(
               singleton,
               observed_at,
               five_hour_used_percentage,
               five_hour_resets_at,
               seven_day_used_percentage,
               seven_day_resets_at
             ) VALUES(1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(singleton) DO UPDATE SET
               observed_at=excluded.observed_at,
               five_hour_used_percentage=excluded.five_hour_used_percentage,
               five_hour_resets_at=excluded.five_hour_resets_at,
               seven_day_used_percentage=excluded.seven_day_used_percentage,
               seven_day_resets_at=excluded.seven_day_resets_at",
            params![
                observed_at,
                five_hour.used_percentage,
                five_hour.resets_at,
                seven_day.used_percentage,
                seven_day.resets_at,
            ],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())?;
    Ok(true)
}

fn response_cursor_retention_cutoff(now: OffsetDateTime) -> OffsetDateTime {
    let today = now.to_offset(time::UtcOffset::UTC).date();
    (today - time::Duration::days(RESPONSE_CURSOR_RETENTION_DAYS - 1))
        .midnight()
        .assume_utc()
}

fn store_response_cursor(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &str,
    api_duration: i64,
    observed_at: &str,
) -> Result<(), ()> {
    transaction
        .execute(
            "INSERT INTO claude_response_cursors(session_id, total_api_duration_ms, observed_at)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
               total_api_duration_ms=excluded.total_api_duration_ms,
               observed_at=excluded.observed_at",
            params![session_id, api_duration, observed_at],
        )
        .map_err(|_| ())?;
    Ok(())
}

fn load_quota_observation(database_path: &Path) -> Result<Option<ClaudeQuotaObservation>, ()> {
    let connection = open_capture_database(database_path)?;
    let row = connection
        .query_row(
            "SELECT
               observed_at,
               five_hour_used_percentage,
               five_hour_resets_at,
               seven_day_used_percentage,
               seven_day_resets_at
             FROM claude_quota_observation
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|_| ())?;
    let Some((observed_at, five_used, five_reset, seven_used, seven_reset)) = row else {
        return Ok(None);
    };
    Ok(Some(ClaudeQuotaObservation {
        observed_at: OffsetDateTime::parse(&observed_at, &Rfc3339).map_err(|_| ())?,
        five_hour: ClaudeRateLimitWindow {
            used_percentage: five_used,
            resets_at: five_reset,
        },
        seven_day: ClaudeRateLimitWindow {
            used_percentage: seven_used,
            resets_at: seven_reset,
        },
    }))
}

#[cfg(any(test, not(debug_assertions)))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(test, not(debug_assertions)))]
fn hex_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(1 + value.len() * 2);
    encoded.push('x');
    for byte in value.bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(value: &str) -> Result<String, ()> {
    let value = value.strip_prefix('x').ok_or(())?;
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect::<Result<Vec<_>, ()>>()?;
    String::from_utf8(bytes).map_err(|_| ())
}

fn hex_digit(value: u8) -> Result<u8, ()> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(any(test, not(debug_assertions)))]
fn bridge_command(
    executable: &Path,
    database_path: &Path,
    notification_path: &Path,
    upstream: Option<&str>,
) -> Result<String, ()> {
    let executable = executable.to_str().ok_or(())?;
    let database_path = database_path.to_str().ok_or(())?;
    let notification_path = notification_path.to_str().ok_or(())?;
    let mut command = format!(
        "{} {STATUS_LINE_ARGUMENT} {} {}",
        shell_quote(executable),
        shell_quote(database_path),
        shell_quote(notification_path),
    );
    if let Some(upstream) = upstream {
        command.push(' ');
        command.push_str(UPSTREAM_ARGUMENT);
        command.push(' ');
        command.push_str(&hex_encode(upstream));
    }
    Ok(command)
}

#[cfg(any(test, not(debug_assertions)))]
fn is_touchgrassbar_bridge(command: &str) -> bool {
    command
        .split_whitespace()
        .any(|part| part == STATUS_LINE_ARGUMENT)
}

#[cfg(any(test, not(debug_assertions)))]
fn bridge_upstream(command: &str) -> Result<Option<String>, ()> {
    if !is_touchgrassbar_bridge(command) {
        return Err(());
    }
    let delimiter = format!(" {UPSTREAM_ARGUMENT} ");
    let Some((_, encoded)) = command.rsplit_once(&delimiter) else {
        return (!command.contains(" -- ")).then_some(None).ok_or(());
    };
    if encoded.contains(char::is_whitespace) {
        return Err(());
    }
    hex_decode(encoded).map(Some)
}

#[cfg(any(test, not(debug_assertions)))]
fn read_settings(path: &Path) -> Result<Value, ()> {
    match fs::read(path) {
        Ok(bytes) => {
            let settings: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
            settings.is_object().then_some(settings).ok_or(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(_) => Err(()),
    }
}

#[cfg(any(test, not(debug_assertions)))]
fn write_settings_atomically(path: &Path, settings: &Value) -> Result<(), ()> {
    let parent = path.parent().ok_or(())?;
    fs::create_dir_all(parent).map_err(|_| ())?;
    let filename = path.file_name().and_then(|name| name.to_str()).ok_or(())?;
    let partial = parent.join(format!(
        ".{filename}.touchgrassbar-{}.tmp",
        std::process::id()
    ));
    let mut contents = serde_json::to_vec_pretty(settings).map_err(|_| ())?;
    contents.push(b'\n');
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&partial).map_err(|_| ())?;
        file.write_all(&contents).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        fs::rename(&partial, path).map_err(|_| ())?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ())
    })();
    if result.is_err() {
        let _ = fs::remove_file(partial);
    }
    result
}

#[cfg(any(test, not(debug_assertions)))]
fn configure_status_line_at(
    settings_path: &Path,
    executable: &Path,
    database_path: &Path,
    notification_path: &Path,
) -> Result<(), ()> {
    let mut settings = read_settings(settings_path)?;
    let root = settings.as_object_mut().ok_or(())?;
    let upstream = match root.get("statusLine") {
        None => None,
        Some(status_line) => {
            let status_line = status_line.as_object().ok_or(())?;
            if status_line.get("type").and_then(Value::as_str) != Some("command") {
                return Err(());
            }
            let current = status_line
                .get("command")
                .and_then(Value::as_str)
                .ok_or(())?
                .to_owned();
            if is_touchgrassbar_bridge(&current) {
                bridge_upstream(&current)?
            } else {
                Some(current)
            }
        }
    };
    drop(open_capture_database(database_path)?);
    let desired_command = bridge_command(
        executable,
        database_path,
        notification_path,
        upstream.as_deref(),
    )?;
    match root.get_mut("statusLine") {
        None => {
            root.insert(
                "statusLine".to_owned(),
                Value::Object(Map::from_iter([
                    ("type".to_owned(), Value::String("command".to_owned())),
                    ("command".to_owned(), Value::String(desired_command)),
                ])),
            );
        }
        Some(status_line) => {
            status_line
                .as_object_mut()
                .ok_or(())?
                .insert("command".to_owned(), Value::String(desired_command));
        }
    }
    write_settings_atomically(settings_path, &settings)
}

#[cfg(not(debug_assertions))]
pub(super) fn configure_production_status_line(database_path: &Path) -> Result<(), ()> {
    let home = env::var_os("HOME").map(PathBuf::from).ok_or(())?;
    let settings_path = home.join(".claude/settings.json");
    let executable = env::current_exe().map_err(|_| ())?;
    configure_status_line_at(
        &settings_path,
        &executable,
        database_path,
        &notification_path(database_path),
    )
}

fn run_upstream(command: &str, input: &[u8]) -> Result<(i32, Vec<u8>), ()> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| ())?;
    child
        .stdin
        .take()
        .ok_or(())?
        .write_all(input)
        .map_err(|_| ())?;
    let output = child.wait_with_output().map_err(|_| ())?;
    Ok((output.status.code().unwrap_or(1), output.stdout))
}

struct StatusLineOutcome {
    captured: bool,
    exit_code: i32,
}

fn run_status_line<R: Read, W: Write>(
    database_path: &Path,
    notification_path: &Path,
    upstream: Option<&str>,
    input: R,
    mut output: W,
    now: OffsetDateTime,
) -> StatusLineOutcome {
    let mut payload = Zeroizing::new(Vec::new());
    let input_read = input
        .take(MAX_STATUS_LINE_BYTES + 1)
        .read_to_end(&mut payload)
        .is_ok();
    let captured = input_read
        && u64::try_from(payload.len()).is_ok_and(|length| length <= MAX_STATUS_LINE_BYTES)
        && capture_status_line_payload(database_path, &payload, now).unwrap_or(false);
    if captured {
        send_notification(notification_path);
        capture_stored_event();
    }
    let Some(upstream) = upstream else {
        return StatusLineOutcome {
            captured,
            exit_code: 0,
        };
    };
    match run_upstream(upstream, &payload) {
        Ok((exit_code, upstream_output)) => {
            let _ = output.write_all(&upstream_output);
            StatusLineOutcome {
                captured,
                exit_code,
            }
        }
        Err(()) => StatusLineOutcome {
            captured,
            exit_code: 1,
        },
    }
}

pub(super) fn run_status_line_from_args() -> Option<i32> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new(STATUS_LINE_ARGUMENT)) {
        return None;
    }
    let Some(database_path) = arguments.next().map(PathBuf::from) else {
        return Some(1);
    };
    let Some(notification_path) = arguments.next().map(PathBuf::from) else {
        return Some(1);
    };
    let upstream = match arguments.next() {
        None => None,
        Some(argument) if argument.as_os_str() == std::ffi::OsStr::new(UPSTREAM_ARGUMENT) => {
            let Some(encoded) = arguments.next() else {
                return Some(1);
            };
            if arguments.next().is_some() {
                return Some(1);
            }
            let Ok(encoded) = encoded.into_string() else {
                return Some(1);
            };
            let Ok(command) = hex_decode(&encoded) else {
                return Some(1);
            };
            Some(command)
        }
        Some(_) => return Some(1),
    };
    let outcome = run_status_line(
        &database_path,
        &notification_path,
        upstream.as_deref(),
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        OffsetDateTime::now_utc(),
    );
    let _ = outcome.captured;
    Some(outcome.exit_code)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;

    use super::*;
    use crate::sanitized::{ProviderPresenceStatus, UsagePeriods, UsageScanStatus, UsageTotal};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct FixtureDirectory(PathBuf);

    impl FixtureDirectory {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "touchgrassbar-claude-quota-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn database(&self) -> PathBuf {
            self.0.join("fixture.sqlite3")
        }

        fn settings(&self) -> PathBuf {
            self.0.join("provider-settings.json")
        }

        fn socket(&self) -> PathBuf {
            #[cfg(unix)]
            {
                PathBuf::from("/tmp").join(format!(
                    "tgb-{}.sock",
                    self.0.file_name().unwrap().to_string_lossy()
                ))
            }
            #[cfg(not(unix))]
            self.0.join("quota.sock")
        }
    }

    impl Drop for FixtureDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.socket());
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FixedClock(OffsetDateTime);

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    fn test_time() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-07T12:00:00Z", &Rfc3339).unwrap()
    }

    fn unavailable_usage() -> UsagePeriods {
        UsagePeriods {
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        }
    }

    fn cached_claude(quota: ProviderSnapshot) -> ProviderPresentation {
        ProviderPresentation {
            provider: CodingProvider::Claude,
            display_name: "Claude".to_owned(),
            presence: ProviderPresenceStatus::Detected,
            quota,
            usage: unavailable_usage(),
        }
    }

    fn status_payload(api_duration: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "cost": {
                "total_api_duration_ms": api_duration,
                "total_cost_usd": 0.1
            },
            "cwd": "/redacted/workspace",
            "credential": "REDACTED-CREDENTIAL",
            "prompt": "REDACTED-PROVIDER-CONTENT",
            "rate_limits": {
                "five_hour": {
                    "resets_at": (test_time() + time::Duration::hours(4)).unix_timestamp(),
                    "used_percentage": 23.5
                },
                "seven_day": {
                    "resets_at": (test_time() + time::Duration::days(6)).unix_timestamp(),
                    "used_percentage": 41.25
                }
            },
            "session_id": "REDACTED-SESSION",
            "transcript_path": "/redacted/transcript"
        }))
        .unwrap()
    }

    fn status_payload_for(
        session_id: &str,
        api_duration: u64,
        five_hour_used: f64,
        seven_day_used: f64,
    ) -> Vec<u8> {
        let mut payload: Value = serde_json::from_slice(&status_payload(api_duration)).unwrap();
        payload["session_id"] = Value::String(session_id.to_owned());
        payload["rate_limits"]["five_hour"]["used_percentage"] = json!(five_hour_used);
        payload["rate_limits"]["seven_day"]["used_percentage"] = json!(seven_day_used);
        serde_json::to_vec(&payload).unwrap()
    }

    #[test]
    fn captures_only_new_complete_response_events_and_sanitizes_the_projection() {
        let fixture = FixtureDirectory::new();
        let database = fixture.database();
        drop(open_capture_database(&database).unwrap());

        #[cfg(unix)]
        let notification = {
            use std::os::unix::net::UnixDatagram;

            let socket = UnixDatagram::bind(fixture.socket()).unwrap();
            socket
                .set_read_timeout(Some(StdDuration::from_secs(1)))
                .unwrap();
            socket
        };

        let first = run_status_line(
            &database,
            &fixture.socket(),
            None,
            status_payload(100).as_slice(),
            Vec::new(),
            test_time(),
        );
        assert!(first.captured);
        #[cfg(unix)]
        {
            let mut message = [0_u8; 1];
            assert_eq!(notification.recv(&mut message).unwrap(), 1);
            notification
                .set_read_timeout(Some(StdDuration::from_millis(25)))
                .unwrap();
        }
        let duplicate = run_status_line(
            &database,
            &fixture.socket(),
            None,
            status_payload(100).as_slice(),
            Vec::new(),
            test_time() + time::Duration::minutes(1),
        );
        assert!(!duplicate.captured, "a timer rerun is not a new response");
        #[cfg(unix)]
        {
            let mut message = [0_u8; 1];
            assert!(notification.recv(&mut message).is_err());
        }

        let mut incomplete: Value = serde_json::from_slice(&status_payload(101)).unwrap();
        incomplete
            .pointer_mut("/rate_limits")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("seven_day");
        assert!(
            !run_status_line(
                &database,
                &fixture.socket(),
                None,
                serde_json::to_vec(&incomplete).unwrap().as_slice(),
                Vec::new(),
                test_time() + time::Duration::minutes(2),
            )
            .captured
        );

        let mut unknown_rate_limit: Value = serde_json::from_slice(&status_payload(102)).unwrap();
        unknown_rate_limit["rate_limits"]["unexpected"] = json!({
            "resets_at": (test_time() + time::Duration::days(1)).unix_timestamp(),
            "used_percentage": 1
        });
        assert!(
            !run_status_line(
                &database,
                &fixture.socket(),
                None,
                serde_json::to_vec(&unknown_rate_limit).unwrap().as_slice(),
                Vec::new(),
                test_time() + time::Duration::minutes(3),
            )
            .captured
        );

        let adapter = ClaudeProviderObservationAdapter::production(
            Arc::new(FixedClock(test_time())),
            Some(database),
        );
        let mut diagnostics = Vec::new();
        let observation = adapter
            .refresh_with_diagnostics(
                &cached_claude(ProviderSnapshot::Unavailable {
                    provider: CodingProvider::Claude,
                    quota_lanes: [],
                }),
                &RefreshAttempt::test(),
                |event| diagnostics.push(event),
            )
            .unwrap()
            .unwrap();
        assert_eq!(diagnostics, ["capture_loaded lane_count=2"]);
        assert_eq!(
            observation.quota,
            ProviderSnapshot::Current {
                provider: CodingProvider::Claude,
                observed_at: "2026-08-07T12:00:00Z".to_owned(),
                quota_lanes: vec![
                    QuotaLane {
                        label: "5-hour limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(76.5),
                        reset_at: Some("2026-08-07T16:00:00Z".to_owned()),
                    },
                    QuotaLane {
                        label: "Weekly limit".to_owned(),
                        unit: "percent".to_owned(),
                        allowance: Some(100.0),
                        remaining: Some(58.75),
                        reset_at: Some("2026-08-13T12:00:00Z".to_owned()),
                    },
                ],
            }
        );
        let serialized = serde_json::to_string(&(observation.quota, observation.usage)).unwrap();
        for sentinel in [
            "REDACTED-CREDENTIAL",
            "REDACTED-PROVIDER-CONTENT",
            "REDACTED-SESSION",
            "/redacted/",
        ] {
            assert!(!serialized.contains(sentinel));
        }
    }

    #[test]
    fn preserves_existing_status_line_output_and_settings() {
        let fixture = FixtureDirectory::new();
        let settings = json!({
            "otherSetting": true,
            "statusLine": {
                "command": "printf kept",
                "hideVimModeIndicator": true,
                "padding": 2,
                "refreshInterval": 1,
                "type": "command"
            }
        });
        fs::write(
            fixture.settings(),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        let executable = fixture.0.join("OriginalBinary");
        configure_status_line_at(
            &fixture.settings(),
            &executable,
            &fixture.database(),
            &fixture.socket(),
        )
        .unwrap();
        let moved_executable = fixture.0.join("MovedBinary");
        configure_status_line_at(
            &fixture.settings(),
            &moved_executable,
            &fixture.database(),
            &fixture.socket(),
        )
        .unwrap();

        let configured: Value =
            serde_json::from_slice(&fs::read(fixture.settings()).unwrap()).unwrap();
        assert_eq!(configured["otherSetting"], true);
        assert_eq!(configured["statusLine"]["padding"], 2);
        assert_eq!(configured["statusLine"]["refreshInterval"], 1);
        assert_eq!(configured["statusLine"]["hideVimModeIndicator"], true);
        assert!(
            configured["statusLine"]["command"]
                .as_str()
                .unwrap()
                .contains(STATUS_LINE_ARGUMENT)
        );
        let bridge = configured["statusLine"]["command"].as_str().unwrap();
        assert!(bridge.contains(moved_executable.to_str().unwrap()));
        assert!(!bridge.contains(executable.to_str().unwrap()));
        let upstream = bridge_upstream(bridge).unwrap();
        assert_eq!(upstream.as_deref(), Some("printf kept"));

        let mut output = Vec::new();
        let outcome = run_status_line(
            &fixture.database(),
            &fixture.socket(),
            upstream.as_deref(),
            status_payload(100).as_slice(),
            &mut output,
            test_time(),
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(output, b"kept");
    }

    #[test]
    fn exact_response_cursors_reject_replays_without_assuming_quota_direction() {
        let fixture = FixtureDirectory::new();
        let database = fixture.database();
        let older = status_payload_for("fixture-older-session", 100, 23.5, 41.25);
        let newer = status_payload_for("fixture-newer-session", 100, 35.0, 52.0);
        let delayed = status_payload_for("fixture-delayed-session", 100, 60.0, 70.0);
        let corrected = status_payload_for("fixture-newer-session", 200, 30.0, 48.0);

        assert!(capture_status_line_payload(&database, &older, test_time()).unwrap());
        assert!(
            capture_status_line_payload(
                &database,
                &newer,
                test_time() + time::Duration::minutes(1),
            )
            .unwrap()
        );
        assert!(
            !capture_status_line_payload(
                &database,
                &delayed,
                test_time() + time::Duration::seconds(30),
            )
            .unwrap(),
            "a delayed helper cannot replace an observation received later"
        );
        assert!(
            !capture_status_line_payload(
                &database,
                &delayed,
                test_time() + time::Duration::minutes(2),
            )
            .unwrap(),
            "a timer rerun cannot replay a delayed response after it was rejected"
        );

        assert!(
            !capture_status_line_payload(
                &database,
                &older,
                test_time() + time::Duration::minutes(2),
            )
            .unwrap(),
            "an exact response cursor rejects a timer replay"
        );

        assert!(
            capture_status_line_payload(
                &database,
                &corrected,
                test_time() + time::Duration::minutes(3),
            )
            .unwrap(),
            "a later response is accepted even when provider quota values decrease"
        );
        assert!(
            !capture_status_line_payload(
                &database,
                &corrected,
                test_time() + time::Duration::minutes(4),
            )
            .unwrap(),
            "the later response is still rejected when its timer reruns"
        );

        let observation = load_quota_observation(&database).unwrap().unwrap();
        assert_eq!(
            observation.observed_at,
            test_time() + time::Duration::minutes(3)
        );
        assert_eq!(observation.five_hour.used_percentage, 30.0);
        assert_eq!(observation.seven_day.used_percentage, 48.0);
    }

    #[test]
    fn response_cursors_retain_only_sixty_utc_ranking_days() {
        let fixture = FixtureDirectory::new();
        let database = fixture.database();
        drop(open_capture_database(&database).unwrap());
        let cutoff = response_cursor_retention_cutoff(test_time());
        let expired_at = (cutoff - time::Duration::seconds(1))
            .format(&Rfc3339)
            .unwrap();
        let retained_at = cutoff.format(&Rfc3339).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "INSERT INTO claude_response_cursors(
                   session_id,
                   total_api_duration_ms,
                   observed_at
                 ) VALUES(?1, ?2, ?3), (?4, ?5, ?6)",
                params![
                    "fixture-expired-session",
                    100,
                    expired_at,
                    "fixture-retained-session",
                    100,
                    retained_at,
                ],
            )
            .unwrap();
        drop(connection);

        assert!(capture_status_line_payload(&database, &status_payload(100), test_time()).unwrap());

        let connection = Connection::open(&database).unwrap();
        let expired_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM claude_response_cursors WHERE session_id = ?1",
                ["fixture-expired-session"],
                |row| row.get(0),
            )
            .unwrap();
        let retained_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM claude_response_cursors WHERE session_id = ?1",
                ["fixture-retained-session"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expired_count, 0);
        assert_eq!(retained_count, 1);
    }

    #[test]
    fn unsupported_settings_and_capture_schemas_fail_closed() {
        let fixture = FixtureDirectory::new();
        let unsupported = br#"{"statusLine":{"type":"unknown","command":"printf kept"}}"#;
        fs::write(fixture.settings(), unsupported).unwrap();
        assert!(
            configure_status_line_at(
                &fixture.settings(),
                &fixture.0.join("TouchGrassBar"),
                &fixture.database(),
                &fixture.socket(),
            )
            .is_err()
        );
        assert_eq!(fs::read(fixture.settings()).unwrap(), unsupported);

        drop(open_capture_database(&fixture.database()).unwrap());
        let connection = Connection::open(fixture.database()).unwrap();
        connection
            .execute(
                "UPDATE touchgrassbar_schema_versions SET version = 2 WHERE module = ?1",
                [CAPTURE_SCHEMA_MODULE],
            )
            .unwrap();
        assert!(load_quota_observation(&fixture.database()).is_err());
        let adapter = ClaudeProviderObservationAdapter::production(
            Arc::new(FixedClock(test_time())),
            Some(fixture.database()),
        );
        let mut diagnostics = Vec::new();
        assert!(matches!(
            adapter.refresh_with_diagnostics(
                &cached_claude(ProviderSnapshot::Unavailable {
                    provider: CodingProvider::Claude,
                    quota_lanes: [],
                }),
                &RefreshAttempt::test(),
                |event| diagnostics.push(event),
            ),
            Err(RefreshFailure::SourceUnavailable)
        ));
        assert_eq!(
            diagnostics,
            ["capture_unavailable reason=storage_unavailable"]
        );
        let mut output = Vec::new();
        let outcome = run_status_line(
            &fixture.database(),
            &fixture.socket(),
            Some("printf kept"),
            status_payload(100).as_slice(),
            &mut output,
            test_time(),
        );
        assert!(!outcome.captured);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(output, b"kept");
    }

    #[test]
    fn adapter_uses_only_new_live_capture_and_never_renews_cached_freshness() {
        let fixture = FixtureDirectory::new();
        let database = fixture.database();
        drop(open_capture_database(&database).unwrap());
        let adapter = ClaudeProviderObservationAdapter::production(
            Arc::new(FixedClock(test_time() + time::Duration::minutes(1))),
            Some(database.clone()),
        );
        let unavailable = ProviderSnapshot::Unavailable {
            provider: CodingProvider::Claude,
            quota_lanes: [],
        };
        let mut diagnostics = Vec::new();
        assert!(
            adapter
                .refresh_with_diagnostics(
                    &cached_claude(unavailable.clone()),
                    &RefreshAttempt::test(),
                    |event| diagnostics.push(event),
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(diagnostics, ["capture_unavailable reason=not_observed"]);

        assert!(capture_status_line_payload(&database, &status_payload(100), test_time()).unwrap());
        let observation = load_quota_observation(&database).unwrap().unwrap();
        let snapshot = observation.sanitized_snapshot(test_time()).unwrap();

        diagnostics.clear();
        assert!(
            adapter
                .refresh_with_diagnostics(
                    &cached_claude(snapshot.clone()),
                    &RefreshAttempt::test(),
                    |event| diagnostics.push(event),
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(diagnostics, ["capture_unchanged"]);
        let stale = match snapshot {
            ProviderSnapshot::Current {
                provider,
                observed_at,
                quota_lanes,
            } => ProviderSnapshot::Stale {
                provider,
                observed_at,
                quota_lanes,
            },
            _ => unreachable!(),
        };
        assert!(
            adapter
                .refresh(&cached_claude(stale.clone()), &RefreshAttempt::test())
                .unwrap()
                .is_none()
        );

        let expired_adapter = ClaudeProviderObservationAdapter::production(
            Arc::new(FixedClock(test_time() + time::Duration::hours(5))),
            Some(database),
        );
        diagnostics.clear();
        assert!(matches!(
            expired_adapter.refresh_with_diagnostics(
                &cached_claude(unavailable),
                &RefreshAttempt::test(),
                |event| diagnostics.push(event),
            ),
            Err(RefreshFailure::SourceUnavailable)
        ));
        assert_eq!(
            diagnostics,
            ["capture_unavailable reason=expired_or_invalid"]
        );
        assert!(
            expired_adapter
                .refresh(&cached_claude(stale), &RefreshAttempt::test())
                .unwrap()
                .is_none(),
            "an unchanged capture cannot resurrect an expired lane"
        );
    }
}
