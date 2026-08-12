use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, params, params_from_iter,
    types::{Value, ValueRef},
};
use serde::Deserialize;
use time::{Date, Duration, Month, OffsetDateTime};

const MAX_TRACE_BODY_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_TRACE_BYTES: usize = 128 * 1024 * 1024;
const MAX_TRACE_ROWS: usize = 16_384;
const MAX_EVENT_RECORDS_PER_BODY: usize = 4_096;
const MAX_TRACE_EVIDENCE: usize = 16_384;
const REQUEST_MARKER: &str = "websocket request:";
const EVENT_MARKER: &str = "websocket event:";
const LEGACY_TARGET: &str = "codex_core::session::handlers";
const LEGACY_SUBMISSION_MARKER: &str = "Submission sub=Submission {";
const LEGACY_SETTINGS_MARKER: &str = "thread_settings: ThreadSettingsOverrides {";
const LEGACY_PRIORITY_MARKER: &str = "service_tier: Some(Some(\"priority\"))";
const RETAINED_ROW_QUERY_CHUNK: usize = 500;

static TRACE_MEMO: OnceLock<Mutex<BTreeMap<PathBuf, TraceMemoState>>> = OnceLock::new();
static TRACE_OBSERVATION_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct FastTurnEvidence {
    pub(super) fingerprint: String,
    pub(super) turns: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TraceFileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone)]
enum OwnedTraceEvidence {
    FastTurn {
        turn_id: String,
        model: Option<String>,
    },
    CompletedModel {
        turn_id: String,
        model: String,
    },
}

#[derive(Clone)]
struct TraceSource {
    day: Date,
    body_bytes: usize,
    evidence: Vec<OwnedTraceEvidence>,
}

// Incremental reads require the Codex logs contract: rows append or prune,
// and an AUTOINCREMENT key prevents row ID reuse. Other schemas use full scans.
#[derive(Clone)]
struct TraceMemoState {
    observation_id: u64,
    coverage_start: Date,
    last_row_id: i64,
    file_identity: TraceFileIdentity,
    sources: BTreeMap<i64, TraceSource>,
}

struct FastTraceLoad {
    turns: BTreeMap<String, Option<String>>,
    #[cfg(test)]
    scanned_rows: usize,
}

#[derive(Deserialize)]
struct RequestRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    #[serde(default)]
    service_tier: Option<&'a str>,
    #[serde(default)]
    turn_id: Option<&'a str>,
    #[serde(default)]
    model: Option<&'a str>,
}

#[derive(Deserialize)]
struct CompletedEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'a str,
    #[serde(default)]
    response: Option<CompletedResponse<'a>>,
}

#[derive(Deserialize)]
struct CompletedResponse<'a> {
    #[serde(default)]
    model: Option<&'a str>,
}

enum TraceEvidence<'a> {
    FastTurn {
        turn_id: &'a str,
        model: Option<&'a str>,
    },
    CompletedModel {
        turn_id: &'a str,
        model: &'a str,
    },
}

#[derive(Default)]
struct FastTurnState {
    request_models: BTreeMap<String, Option<String>>,
    completed_models: BTreeMap<String, String>,
}

impl FastTurnState {
    fn observe(&mut self, evidence: TraceEvidence<'_>) {
        match evidence {
            TraceEvidence::FastTurn { turn_id, model } => {
                self.request_models
                    .insert(turn_id.to_owned(), model.map(str::to_owned));
            }
            TraceEvidence::CompletedModel { turn_id, model } => {
                self.completed_models
                    .insert(turn_id.to_owned(), model.to_owned());
            }
        }
    }

    fn observe_owned(&mut self, evidence: &OwnedTraceEvidence) {
        match evidence {
            OwnedTraceEvidence::FastTurn { turn_id, model } => {
                self.request_models.insert(turn_id.clone(), model.clone());
            }
            OwnedTraceEvidence::CompletedModel { turn_id, model } => {
                self.completed_models.insert(turn_id.clone(), model.clone());
            }
        }
    }

    fn finish(mut self) -> BTreeMap<String, Option<String>> {
        self.request_models
            .into_iter()
            .map(|(turn_id, request_model)| {
                let model = self.completed_models.remove(&turn_id).or(request_model);
                (turn_id, model)
            })
            .collect()
    }
}

pub(super) fn load_fast_turn_evidence(
    codex_home: &Path,
    cutoff: Date,
    today: Date,
) -> Result<FastTurnEvidence, ()> {
    let loaded = load_fast_turns_from_database(&codex_home.join("logs_2.sqlite"), cutoff, today)?;
    let turns = loaded.turns;
    let fingerprint = fast_turn_fingerprint(&turns);
    Ok(FastTurnEvidence { fingerprint, turns })
}

fn load_fast_turns_from_database(
    database: &Path,
    cutoff: Date,
    today: Date,
) -> Result<FastTraceLoad, ()> {
    match database.metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            trace_memo().remove(database);
            return Ok(FastTraceLoad {
                turns: BTreeMap::new(),
                #[cfg(test)]
                scanned_rows: 0,
            });
        }
        Err(_) => return Err(()),
        Ok(metadata) if !metadata.is_file() => return Err(()),
        Ok(_) => {}
    }
    let file_identity = trace_file_identity(database)?;
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|_| ())?;
    if trace_file_identity(database)? != file_identity {
        return Err(());
    }
    let (start_filter, end_filter) = trace_query_bounds(&connection, cutoff, today)?;
    if !logs_have_monotonic_row_ids(&connection)? {
        trace_memo().remove(database);
        return load_fast_turns_full_scan(&connection, &start_filter, &end_filter);
    }
    let max_row_id = connection
        .query_row("SELECT COALESCE(MAX(rowid), 0) FROM logs", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| ())?;
    let observation_id = TRACE_OBSERVATION_ID.fetch_add(1, Ordering::Relaxed) + 1;
    let mut state = trace_memo().get(database).filter(|memo| {
        memo.file_identity == file_identity
            && memo.last_row_id <= max_row_id
            && memo.coverage_start <= cutoff
    });
    let had_memo = state.is_some();
    let mut state = state.take().unwrap_or(TraceMemoState {
        observation_id,
        coverage_start: cutoff,
        last_row_id: 0,
        file_identity,
        sources: BTreeMap::new(),
    });
    state.observation_id = observation_id;
    state.coverage_start = cutoff;
    state.sources.retain(|_, source| source.day >= cutoff);
    if had_memo {
        retain_existing_trace_sources(&connection, &mut state.sources)?;
    }

    let remaining = MAX_TRACE_ROWS.checked_sub(state.sources.len()).ok_or(())?;
    let (mut retained_bytes, mut retained_evidence) = retained_trace_totals(&state.sources)?;
    let query_limit = i64::try_from(remaining.checked_add(1).ok_or(())?).map_err(|_| ())?;
    let has_target = logs_have_target_column(&connection)?;
    let target_projection = if has_target { "target" } else { "''" };
    let legacy_filter = if has_target {
        "OR (target = 'codex_core::session::handlers' AND feedback_log_body LIKE '%service_tier: Some(Some(\"priority\"))%')"
    } else {
        ""
    };
    let uses_timestamp_index = state.last_row_id == 0 && has_timestamp_index(&connection)?;
    let query = if uses_timestamp_index {
        format!(
            "SELECT rowid, ts, {target_projection}, feedback_log_body
             FROM logs INDEXED BY idx_logs_ts
             WHERE ts >= ?1
               AND (feedback_log_body LIKE '%websocket request:%'
                    OR feedback_log_body LIKE '%response.completed%'
                    {legacy_filter})
             ORDER BY rowid
             LIMIT ?2"
        )
    } else {
        format!(
            "SELECT rowid, ts, {target_projection}, feedback_log_body
             FROM logs
             WHERE rowid > ?1 AND ts >= ?2
               AND (feedback_log_body LIKE '%websocket request:%'
                    OR feedback_log_body LIKE '%response.completed%'
                    {legacy_filter})
             ORDER BY rowid
             LIMIT ?3"
        )
    };
    let mut statement = connection.prepare(&query).map_err(|_| ())?;
    let mut rows = if uses_timestamp_index {
        statement
            .query(params![start_filter, query_limit])
            .map_err(|_| ())?
    } else {
        statement
            .query(params![state.last_row_id, start_filter, query_limit])
            .map_err(|_| ())?
    };
    let mut scanned_rows = 0_usize;
    while let Some(row) = rows.next().map_err(|_| ())? {
        scanned_rows = scanned_rows.checked_add(1).ok_or(())?;
        if scanned_rows > remaining {
            return Err(());
        }
        let row_id = row.get::<_, i64>(0).map_err(|_| ())?;
        let day = trace_ranking_day(row.get_ref(1).map_err(|_| ())?)?;
        let target = row.get::<_, String>(2).map_err(|_| ())?;
        let body = row.get::<_, String>(3).map_err(|_| ())?;
        if body.len() > MAX_TRACE_BODY_BYTES {
            return Err(());
        }
        retained_bytes = retained_bytes.checked_add(body.len()).ok_or(())?;
        if retained_bytes > MAX_RETAINED_TRACE_BYTES {
            return Err(());
        }
        let evidence = parse_trace_evidence(&target, &body)?
            .into_iter()
            .map(|evidence| match evidence {
                TraceEvidence::FastTurn { turn_id, model } => OwnedTraceEvidence::FastTurn {
                    turn_id: turn_id.to_owned(),
                    model: model.map(str::to_owned),
                },
                TraceEvidence::CompletedModel { turn_id, model } => {
                    OwnedTraceEvidence::CompletedModel {
                        turn_id: turn_id.to_owned(),
                        model: model.to_owned(),
                    }
                }
            })
            .collect::<Vec<_>>();
        retained_evidence = retained_evidence.checked_add(evidence.len()).ok_or(())?;
        if retained_evidence > MAX_TRACE_EVIDENCE {
            return Err(());
        }
        state.sources.insert(
            row_id,
            TraceSource {
                day,
                body_bytes: body.len(),
                evidence,
            },
        );
    }
    drop(rows);
    drop(statement);
    state.last_row_id = max_row_id;
    let turns = classify_trace_sources(&state.sources, cutoff, today)?;
    let authoritative = trace_memo().store_or_get_newer(database, state)?;
    let turns = if authoritative.observation_id == observation_id {
        turns
    } else {
        classify_trace_sources(&authoritative.sources, cutoff, today)?
    };
    Ok(FastTraceLoad {
        turns,
        #[cfg(test)]
        scanned_rows,
    })
}

fn load_fast_turns_full_scan(
    connection: &Connection,
    start: &Value,
    end: &Value,
) -> Result<FastTraceLoad, ()> {
    let limit = i64::try_from(MAX_TRACE_ROWS + 1).map_err(|_| ())?;
    let has_target = logs_have_target_column(connection)?;
    let target_projection = if has_target { "target" } else { "''" };
    let legacy_filter = if has_target {
        "OR (target = 'codex_core::session::handlers' AND feedback_log_body LIKE '%service_tier: Some(Some(\"priority\"))%')"
    } else {
        ""
    };
    let query = format!(
        "SELECT {target_projection}, feedback_log_body
         FROM logs
         WHERE ts >= ?1 AND ts < ?2
           AND (feedback_log_body LIKE '%websocket request:%'
                OR feedback_log_body LIKE '%response.completed%'
                {legacy_filter})
         ORDER BY rowid
         LIMIT ?3"
    );
    let mut statement = connection.prepare(&query).map_err(|_| ())?;
    let rows = statement
        .query_map(params![start, end, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| ())?;
    let mut state = FastTurnState::default();
    let mut scanned_rows = 0_usize;
    let mut retained_bytes = 0_usize;
    let mut trace_evidence = 0_usize;
    for row in rows {
        scanned_rows = scanned_rows.checked_add(1).ok_or(())?;
        if scanned_rows > MAX_TRACE_ROWS {
            return Err(());
        }
        let (target, body) = row.map_err(|_| ())?;
        if body.len() > MAX_TRACE_BODY_BYTES {
            return Err(());
        }
        retained_bytes = retained_bytes.checked_add(body.len()).ok_or(())?;
        if retained_bytes > MAX_RETAINED_TRACE_BYTES {
            return Err(());
        }
        collect_trace_evidence(&mut state, &target, &body, &mut trace_evidence)?;
    }
    Ok(FastTraceLoad {
        turns: state.finish(),
        #[cfg(test)]
        scanned_rows,
    })
}

fn trace_query_bounds(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<(Value, Value), ()> {
    let storage = connection
        .query_row(
            "SELECT typeof(ts) FROM logs WHERE ts IS NOT NULL ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if storage.as_deref() == Some("text") {
        return Ok((
            Value::Text(cutoff.to_string()),
            Value::Text((today + Duration::days(1)).to_string()),
        ));
    }
    if storage.as_deref().is_some_and(|value| value != "integer") {
        return Err(());
    }
    Ok((
        Value::Integer(cutoff.midnight().assume_utc().unix_timestamp()),
        Value::Integer(
            (today + Duration::days(1))
                .midnight()
                .assume_utc()
                .unix_timestamp(),
        ),
    ))
}

fn trace_memo() -> TraceMemo {
    TraceMemo
}

struct TraceMemo;

impl TraceMemo {
    fn states(&self) -> &Mutex<BTreeMap<PathBuf, TraceMemoState>> {
        TRACE_MEMO.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    fn get(&self, database: &Path) -> Option<TraceMemoState> {
        self.states()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(database)
            .cloned()
    }

    fn remove(&self, database: &Path) {
        self.states()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(database);
    }

    fn store_or_get_newer(
        &self,
        database: &Path,
        state: TraceMemoState,
    ) -> Result<TraceMemoState, ()> {
        let mut states = self
            .states()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = states
            .get(database)
            .filter(|existing| existing.observation_id > state.observation_id)
        {
            if existing.file_identity != state.file_identity
                || existing.coverage_start > state.coverage_start
            {
                return Err(());
            }
            return Ok(existing.clone());
        }
        states.insert(database.to_path_buf(), state.clone());
        Ok(state)
    }
}

fn trace_file_identity(database: &Path) -> Result<TraceFileIdentity, ()> {
    let metadata = database.metadata().map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        Ok(TraceFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    let created = metadata
        .created()
        .map_err(|_| ())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?;
    #[cfg(not(unix))]
    Ok(TraceFileIdentity {
        device: metadata.len(),
        inode: u64::try_from(created.as_nanos()).map_err(|_| ())?,
    })
}

fn trace_ranking_day(value: ValueRef<'_>) -> Result<Date, ()> {
    let timestamp = match value {
        ValueRef::Integer(value) => value,
        ValueRef::Text(value) => {
            let value = std::str::from_utf8(value).map_err(|_| ())?;
            if let Ok(timestamp) = value.parse::<i64>() {
                timestamp
            } else {
                return parse_trace_day_prefix(value);
            }
        }
        _ => return Err(()),
    };
    OffsetDateTime::from_unix_timestamp(timestamp)
        .map(|value| value.date())
        .map_err(|_| ())
}

fn parse_trace_day_prefix(value: &str) -> Result<Date, ()> {
    let day = value.get(..10).ok_or(())?;
    if day.as_bytes().get(4) != Some(&b'-') || day.as_bytes().get(7) != Some(&b'-') {
        return Err(());
    }
    let year = day.get(..4).ok_or(())?.parse::<i32>().map_err(|_| ())?;
    let month = day.get(5..7).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    let day = day.get(8..10).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    Date::from_calendar_date(year, Month::try_from(month).map_err(|_| ())?, day).map_err(|_| ())
}

fn has_timestamp_index(connection: &Connection) -> Result<bool, ()> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM sqlite_master
               WHERE type = 'index' AND tbl_name = 'logs' AND name = 'idx_logs_ts'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| ())
}

fn logs_have_target_column(connection: &Connection) -> Result<bool, ()> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('logs') WHERE name = 'target'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| ())
}

fn logs_have_monotonic_row_ids(connection: &Connection) -> Result<bool, ()> {
    let schema = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'logs'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| ())?;
    let normalized = schema
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    Ok(normalized.contains("INTEGER PRIMARY KEY AUTOINCREMENT"))
}

fn retain_existing_trace_sources(
    connection: &Connection,
    sources: &mut BTreeMap<i64, TraceSource>,
) -> Result<(), ()> {
    if sources.is_empty() {
        return Ok(());
    }
    let source_row_ids = sources.keys().copied().collect::<Vec<_>>();
    let mut retained = BTreeSet::new();
    for chunk in source_row_ids.chunks(RETAINED_ROW_QUERY_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!("SELECT rowid FROM logs WHERE rowid IN ({placeholders})");
        let mut statement = connection.prepare(&query).map_err(|_| ())?;
        let rows = statement
            .query_map(params_from_iter(chunk.iter()), |row| row.get::<_, i64>(0))
            .map_err(|_| ())?;
        for row_id in rows {
            retained.insert(row_id.map_err(|_| ())?);
        }
    }
    sources.retain(|row_id, _| retained.contains(row_id));
    Ok(())
}

fn retained_trace_totals(sources: &BTreeMap<i64, TraceSource>) -> Result<(usize, usize), ()> {
    let mut retained_bytes = 0_usize;
    let mut retained_evidence = 0_usize;
    for source in sources.values() {
        retained_bytes = retained_bytes.checked_add(source.body_bytes).ok_or(())?;
        retained_evidence = retained_evidence
            .checked_add(source.evidence.len())
            .ok_or(())?;
        if retained_bytes > MAX_RETAINED_TRACE_BYTES || retained_evidence > MAX_TRACE_EVIDENCE {
            return Err(());
        }
    }
    Ok((retained_bytes, retained_evidence))
}

fn classify_trace_sources(
    sources: &BTreeMap<i64, TraceSource>,
    cutoff: Date,
    today: Date,
) -> Result<BTreeMap<String, Option<String>>, ()> {
    let mut state = FastTurnState::default();
    let mut trace_evidence = 0_usize;
    for source in sources
        .values()
        .filter(|source| source.day >= cutoff && source.day <= today)
    {
        for evidence in &source.evidence {
            trace_evidence = trace_evidence.checked_add(1).ok_or(())?;
            if trace_evidence > MAX_TRACE_EVIDENCE {
                return Err(());
            }
            state.observe_owned(evidence);
        }
    }
    Ok(state.finish())
}

fn fast_turn_fingerprint(turns: &BTreeMap<String, Option<String>>) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in turns.iter().flat_map(|(turn_id, model)| {
        turn_id
            .bytes()
            .chain(std::iter::once(0))
            .chain(model.iter().flat_map(|model| model.bytes()))
            .chain(std::iter::once(0))
    }) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn parse_trace_evidence<'a>(target: &str, body: &'a str) -> Result<Vec<TraceEvidence<'a>>, ()> {
    let mut evidence = Vec::new();
    let mut active_turn_id = None;
    let mut trace_records = 0_usize;
    for line in body.lines() {
        let request_marker = line.find(REQUEST_MARKER);
        let event_marker = line.find(EVENT_MARKER);
        let provider_record = match (request_marker, event_marker) {
            (Some(request), Some(event)) if request < event => Some((request, REQUEST_MARKER)),
            (Some(_), Some(event)) => Some((event, EVENT_MARKER)),
            (Some(request), None) => Some((request, REQUEST_MARKER)),
            (None, Some(event)) => Some((event, EVENT_MARKER)),
            (None, None) => None,
        };

        if let Some((marker, marker_text)) = provider_record {
            trace_records = trace_records.checked_add(1).ok_or(())?;
            if trace_records > MAX_EVENT_RECORDS_PER_BODY {
                return Err(());
            }
            let prefix = &line[..marker];
            let Ok(line_turn_id) = trace_turn_id(prefix) else {
                continue;
            };
            if line_turn_id.is_some() {
                active_turn_id = line_turn_id;
            }
            let record_body = line[marker + marker_text.len()..].trim();
            if marker_text == REQUEST_MARKER {
                let Ok(request) = serde_json::from_str::<RequestRecord<'_>>(record_body) else {
                    continue;
                };
                if request.record_type != "response.create" {
                    continue;
                }
                let Some(tier) = request.service_tier else {
                    continue;
                };
                if !is_fast_tier(tier) {
                    continue;
                }
                let Some(turn_id) = line_turn_id.or(request.turn_id) else {
                    continue;
                };
                if !valid_turn_id(turn_id) {
                    continue;
                }
                active_turn_id = Some(turn_id);
                evidence.push(TraceEvidence::FastTurn {
                    turn_id,
                    model: request.model.filter(|model| valid_model_name(model)),
                });
                continue;
            }

            if !record_body.contains("response.completed") {
                continue;
            }
            let Ok(event) = serde_json::from_str::<CompletedEvent<'_>>(record_body) else {
                continue;
            };
            if event.event_type != "response.completed" {
                continue;
            }
            let Some(turn_id) = line_turn_id.or(active_turn_id) else {
                continue;
            };
            let Some(model) = event.response.as_ref().and_then(|response| response.model) else {
                continue;
            };
            if !valid_model_name(model) {
                continue;
            }
            evidence.push(TraceEvidence::CompletedModel { turn_id, model });
            continue;
        }

        if target != LEGACY_TARGET || !line.contains(LEGACY_PRIORITY_MARKER) {
            continue;
        }
        trace_records = trace_records.checked_add(1).ok_or(())?;
        if trace_records > MAX_EVENT_RECORDS_PER_BODY {
            return Err(());
        }
        let Some(submission_marker) = line.find(LEGACY_SUBMISSION_MARKER) else {
            continue;
        };
        let Some(settings_marker) = line[submission_marker + LEGACY_SUBMISSION_MARKER.len()..]
            .find(LEGACY_SETTINGS_MARKER)
            .map(|offset| offset + submission_marker + LEGACY_SUBMISSION_MARKER.len())
        else {
            continue;
        };
        let Some(priority_marker) = line[settings_marker + LEGACY_SETTINGS_MARKER.len()..]
            .find(LEGACY_PRIORITY_MARKER)
            .map(|offset| offset + settings_marker + LEGACY_SETTINGS_MARKER.len())
        else {
            continue;
        };
        let submission = &line[submission_marker + LEGACY_SUBMISSION_MARKER.len()..settings_marker];
        let Some(turn_id) = quoted_value_after(submission, "id: \"") else {
            continue;
        };
        if !valid_turn_id(turn_id) || priority_marker <= settings_marker {
            continue;
        }
        evidence.push(TraceEvidence::FastTurn {
            turn_id,
            model: None,
        });
    }
    Ok(evidence)
}

fn collect_trace_evidence(
    state: &mut FastTurnState,
    target: &str,
    body: &str,
    trace_evidence: &mut usize,
) -> Result<(), ()> {
    for evidence in parse_trace_evidence(target, body)? {
        *trace_evidence = trace_evidence.checked_add(1).ok_or(())?;
        if *trace_evidence > MAX_TRACE_EVIDENCE {
            return Err(());
        }
        state.observe(evidence);
    }
    Ok(())
}

fn is_fast_tier(value: &str) -> bool {
    matches!(value, "priority" | "fast")
}

fn trace_turn_id(prefix: &str) -> Result<Option<&str>, ()> {
    let dotted = value_after(prefix, "turn.id=");
    let underscored = value_after(prefix, "turn_id=");
    let value = match (dotted, underscored) {
        (Some(left), Some(right)) if left != right => return Err(()),
        (Some(value), _) | (_, Some(value)) => Some(value),
        (None, None) => None,
    };
    if value.is_some_and(|value| !valid_turn_id(value)) {
        return Err(());
    }
    Ok(value)
}

fn value_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let tail = text.split_once(marker)?.1;
    let value = tail
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ']' | ')' | '}' | ':')
        })
        .next()?;
    (!value.is_empty()).then_some(value)
}

fn quoted_value_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let tail = text.split_once(marker)?.1;
    let end = tail.find('"')?;
    let value = &tail[..end];
    (!value.is_empty()).then_some(value)
}

pub(super) fn valid_turn_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_model_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct TraceFixture(PathBuf);

    impl TraceFixture {
        fn new() -> Self {
            let id = TEST_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "touchgrassbar-fast-trace-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TraceFixture {
        fn drop(&mut self) {
            trace_memo().remove(&self.0.join("logs_2.sqlite"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn insert_trace(connection: &Connection, timestamp: i64, body: &str) -> i64 {
        connection
            .execute(
                "INSERT INTO logs(ts, target, feedback_log_body) VALUES(?1, ?2, ?3)",
                params![timestamp, "codex_core::stream_events_utils", body],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    fn classify(bodies: &[&str]) -> Result<BTreeMap<String, Option<String>>, ()> {
        classify_targeted(&bodies.iter().map(|body| ("", *body)).collect::<Vec<_>>())
    }

    fn classify_targeted(rows: &[(&str, &str)]) -> Result<BTreeMap<String, Option<String>>, ()> {
        let mut state = FastTurnState::default();
        let mut trace_evidence = 0;
        for (target, body) in rows {
            collect_trace_evidence(&mut state, target, body, &mut trace_evidence)?;
        }
        Ok(state.finish())
    }

    #[test]
    fn production_shaped_requests_return_only_bounded_pricing_metadata() {
        let priority = r#"private=ignored turn.id=turn-safe websocket request: {"type":"response.create","model":"request-alias","service_tier":"priority","input":"private"}"#;
        let fast = r#"private=ignored websocket request: {"type":"response.create","turn_id":"turn-fast","model":"request-alias","service_tier":"fast","input":"private"}"#;

        assert_eq!(
            classify(&[priority, fast]).unwrap(),
            BTreeMap::from([
                ("turn-fast".to_owned(), Some("request-alias".to_owned()),),
                ("turn-safe".to_owned(), Some("request-alias".to_owned()),),
            ])
        );
        assert!(
            parse_trace_evidence(
                "",
                &format!(
                    "turn.id={} websocket request: {{\"type\":\"response.create\",\"service_tier\":\"priority\"}}",
                    "x".repeat(129)
                ),
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn production_shaped_legacy_submission_proves_fast_without_a_model() {
        let legacy = concat!(
            "INFO codex_core::session::handlers: session_loop{thread_id=thread}: ",
            "Submission sub=Submission { id: \"turn-legacy\", ",
            "op: UserInput { text: \"private\" }, ",
            "thread_settings: ThreadSettingsOverrides { ",
            "service_tier: Some(Some(\"priority\")) }",
        );

        assert_eq!(
            classify_targeted(&[(LEGACY_TARGET, legacy)]).unwrap(),
            BTreeMap::from([("turn-legacy".to_owned(), None)])
        );
    }

    #[test]
    fn legacy_submission_uses_the_separate_trusted_target_column() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE logs (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   ts INTEGER NOT NULL,
                   target TEXT NOT NULL,
                   feedback_log_body TEXT NOT NULL
                 );
                 CREATE INDEX idx_logs_ts ON logs(ts);",
            )
            .unwrap();
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let timestamp = today.midnight().assume_utc().unix_timestamp();
        let legacy = concat!(
            "Submission sub=Submission { id: \"turn-legacy\", ",
            "op: UserInput { text: \"private\" }, ",
            "thread_settings: ThreadSettingsOverrides { ",
            "service_tier: Some(Some(\"priority\")) }",
        );
        connection
            .execute(
                "INSERT INTO logs(ts, target, feedback_log_body) VALUES(?1, ?2, ?3)",
                params![timestamp, "codex_core::stream_events_utils", legacy],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO logs(ts, target, feedback_log_body) VALUES(?1, ?2, ?3)",
                params![timestamp, LEGACY_TARGET, legacy],
            )
            .unwrap();

        let loaded = load_fast_turns_from_database(&database, today, today).unwrap();

        assert_eq!(loaded.scanned_rows, 1);
        assert_eq!(
            loaded.turns,
            BTreeMap::from([("turn-legacy".to_owned(), None)])
        );
    }

    #[test]
    fn exact_completion_refines_the_fast_turn_model() {
        let completion = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol","service_tier":"default","output":"private"}}"#;
        let request = r#"turn.id=turn-safe websocket request: {"type":"response.create","model":"request-alias","service_tier":"priority","input":"private"}"#;
        let later_completion = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra"}}"#;

        assert_eq!(
            classify(&[completion, request]).unwrap(),
            BTreeMap::from([("turn-safe".to_owned(), Some("gpt-5.6-sol".to_owned()))])
        );
        assert_eq!(
            classify(&[request, completion, later_completion]).unwrap(),
            BTreeMap::from([("turn-safe".to_owned(), Some("gpt-5.6-terra".to_owned()))])
        );
    }

    #[test]
    fn exact_request_model_is_retained_without_a_completion() {
        let request = r#"turn.id=turn-safe websocket request: {"type":"response.create","model":"gpt-5.5","service_tier":"priority"}"#;

        assert_eq!(
            classify(&[request]).unwrap(),
            BTreeMap::from([("turn-safe".to_owned(), Some("gpt-5.5".to_owned()))])
        );
    }

    #[test]
    fn unmarked_prompt_text_and_unrelated_trace_noise_do_not_prove_or_erase_fast() {
        let request = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"fast"}"#;
        let unrelated = concat!(
            "private prompt says response.create service_tier priority ",
            "Submission sub=Submission { id: \"prompt-spoof\" } ",
            "service_tier: Some(Some(\"priority\"))\n",
            "turn.id=turn-safe websocket event: incomplete-unrelated-event\n",
            "turn.id=turn-safe websocket event: {\"type\":\"response.output_item.done\",\"item\":{\"private\":\"ignored\"}}",
        );

        assert!(classify(&[unrelated]).unwrap().is_empty());
        assert_eq!(
            classify(&[request, unrelated]).unwrap(),
            BTreeMap::from([("turn-safe".to_owned(), None)])
        );
    }

    #[test]
    fn default_or_non_response_requests_do_not_prove_fast() {
        let default = r#"turn.id=turn-default websocket request: {"type":"response.create","service_tier":"default"}"#;
        let missing = r#"turn.id=turn-missing websocket request: {"type":"response.create"}"#;
        let unrelated = r#"turn.id=turn-other websocket request: {"type":"session.update","service_tier":"priority"}"#;

        assert!(classify(&[default, missing, unrelated]).unwrap().is_empty());
    }

    #[test]
    fn malformed_provider_evidence_fails_closed() {
        let malformed_request = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"priority""#;
        let missing_model =
            r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{}}"#;
        let malformed_legacy = concat!(
            "INFO codex_core::session::handlers: Submission sub=Submission { ",
            "thread_settings: ThreadSettingsOverrides { ",
            "service_tier: Some(Some(\"priority\")) }",
        );

        assert!(classify(&[malformed_request]).unwrap().is_empty());
        assert!(classify(&[missing_model]).unwrap().is_empty());
        assert!(
            classify_targeted(&[(LEGACY_TARGET, malformed_legacy)])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn trace_memo_reads_only_appends_and_revalidates_deleted_sources() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE logs (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   ts INTEGER NOT NULL,
                   target TEXT NOT NULL,
                   feedback_log_body TEXT NOT NULL
                 );
                 CREATE INDEX idx_logs_ts ON logs(ts);",
            )
            .unwrap();
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let timestamp = today.midnight().assume_utc().unix_timestamp();
        let priority = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"priority"}"#;
        let completion = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol"}}"#;
        let other = r#"turn.id=turn-other websocket request: {"type":"response.create","service_tier":"fast"}"#;

        let priority_row = insert_trace(&connection, timestamp, priority);
        let initial = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(initial.scanned_rows, 1);
        assert_eq!(initial.turns.get("turn-safe"), Some(&None));

        let unchanged = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(unchanged.scanned_rows, 0);
        assert_eq!(unchanged.turns, initial.turns);

        let completion_row = insert_trace(&connection, timestamp, completion);
        let refined = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(refined.scanned_rows, 1);
        assert_eq!(
            refined.turns.get("turn-safe"),
            Some(&Some("gpt-5.6-sol".to_owned()))
        );

        insert_trace(&connection, timestamp, other);
        let appended = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(appended.scanned_rows, 1);
        assert_eq!(appended.turns.get("turn-other"), Some(&None));

        connection
            .execute("DELETE FROM logs WHERE rowid = ?1", [completion_row])
            .unwrap();
        let unrefined = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(unrefined.scanned_rows, 0);
        assert_eq!(unrefined.turns.get("turn-safe"), Some(&None));

        connection
            .execute("DELETE FROM logs WHERE rowid = ?1", [priority_row])
            .unwrap();
        let pruned = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(pruned.scanned_rows, 0);
        assert!(!pruned.turns.contains_key("turn-safe"));
        assert!(pruned.turns.contains_key("turn-other"));
    }

    #[test]
    fn non_monotonic_trace_schema_uses_authoritative_full_scans() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE logs (
                   ts INTEGER NOT NULL,
                   target TEXT NOT NULL,
                   feedback_log_body TEXT NOT NULL
                 );
                 CREATE INDEX idx_logs_ts ON logs(ts);",
            )
            .unwrap();
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let timestamp = today.midnight().assume_utc().unix_timestamp();
        let priority = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"priority"}"#;
        let standard = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"default"}"#;
        let row_id = insert_trace(&connection, timestamp, priority);

        let initial = load_fast_turns_from_database(&database, today, today).unwrap();
        let unchanged = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(initial.scanned_rows, 1);
        assert_eq!(unchanged.scanned_rows, 1);
        assert!(unchanged.turns.contains_key("turn-safe"));

        connection
            .execute(
                "UPDATE logs SET feedback_log_body = ?1 WHERE rowid = ?2",
                params![standard, row_id],
            )
            .unwrap();
        let updated = load_fast_turns_from_database(&database, today, today).unwrap();
        assert_eq!(updated.scanned_rows, 1);
        assert!(!updated.turns.contains_key("turn-safe"));
    }

    #[test]
    fn incremental_trace_accepts_a_bounded_iso_timestamp() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE logs (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   ts TEXT NOT NULL,
                   target TEXT NOT NULL,
                   feedback_log_body TEXT NOT NULL
                 );
                 CREATE INDEX idx_logs_ts ON logs(ts);",
            )
            .unwrap();
        let priority = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"priority"}"#;
        connection
            .execute(
                "INSERT INTO logs(ts, target, feedback_log_body) VALUES(?1, ?2, ?3)",
                params![
                    "2020-01-01T10:00:00Z",
                    "codex_core::stream_events_utils",
                    priority
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO logs(ts, target, feedback_log_body) VALUES(?1, ?2, ?3)",
                params![
                    "2026-08-09T10:00:00Z",
                    "codex_core::stream_events_utils",
                    priority
                ],
            )
            .unwrap();
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();

        let loaded = load_fast_turns_from_database(&database, today, today).unwrap();

        assert_eq!(loaded.scanned_rows, 1);
        assert!(loaded.turns.contains_key("turn-safe"));
    }

    #[test]
    fn timestamp_index_initial_scan_applies_evidence_in_row_order() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE logs (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   ts INTEGER NOT NULL,
                   target TEXT NOT NULL,
                   feedback_log_body TEXT NOT NULL
                 );
                 CREATE INDEX idx_logs_ts ON logs(ts);",
            )
            .unwrap();
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let timestamp = today.midnight().assume_utc().unix_timestamp();
        insert_trace(
            &connection,
            timestamp + 2,
            r#"turn.id=turn-safe websocket request: {"type":"response.create","model":"gpt-5.5","service_tier":"priority"}"#,
        );
        insert_trace(
            &connection,
            timestamp + 1,
            r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol"}}"#,
        );
        insert_trace(
            &connection,
            timestamp,
            r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra"}}"#,
        );

        let loaded = load_fast_turns_from_database(&database, today, today).unwrap();

        assert_eq!(
            loaded.turns.get("turn-safe"),
            Some(&Some("gpt-5.6-terra".to_owned()))
        );
    }

    #[test]
    fn older_overlapping_trace_scan_uses_the_newer_memo() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let identity = TraceFileIdentity {
            device: 1,
            inode: 2,
        };
        let newer = TraceMemoState {
            observation_id: 2,
            coverage_start: today,
            last_row_id: 2,
            file_identity: identity,
            sources: BTreeMap::from([(
                2,
                TraceSource {
                    day: today,
                    body_bytes: 0,
                    evidence: Vec::new(),
                },
            )]),
        };
        let older = TraceMemoState {
            observation_id: 1,
            coverage_start: today,
            last_row_id: 1,
            file_identity: identity,
            sources: BTreeMap::new(),
        };

        trace_memo().store_or_get_newer(&database, newer).unwrap();
        let authoritative = trace_memo().store_or_get_newer(&database, older).unwrap();

        assert_eq!(authoritative.observation_id, 2);
        assert_eq!(authoritative.last_row_id, 2);
        assert_eq!(trace_memo().get(&database).unwrap().observation_id, 2);
    }

    #[test]
    fn aggregate_retained_trace_body_bytes_are_bounded() {
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let source_count = MAX_RETAINED_TRACE_BYTES / MAX_TRACE_BODY_BYTES;
        let mut sources = (0..source_count)
            .map(|row_id| {
                (
                    i64::try_from(row_id).unwrap(),
                    TraceSource {
                        day: today,
                        body_bytes: MAX_TRACE_BODY_BYTES,
                        evidence: Vec::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            retained_trace_totals(&sources).unwrap().0,
            MAX_RETAINED_TRACE_BYTES
        );
        sources.insert(
            i64::try_from(source_count).unwrap(),
            TraceSource {
                day: today,
                body_bytes: 1,
                evidence: Vec::new(),
            },
        );
        assert!(retained_trace_totals(&sources).is_err());
    }

    #[test]
    fn trace_metadata_failure_preserves_the_committed_memo() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        std::fs::create_dir(&database).unwrap();
        let committed = TraceMemoState {
            observation_id: 1,
            coverage_start: today,
            last_row_id: 1,
            file_identity: TraceFileIdentity {
                device: 1,
                inode: 2,
            },
            sources: BTreeMap::new(),
        };
        trace_memo()
            .store_or_get_newer(&database, committed)
            .unwrap();
        assert!(load_fast_turns_from_database(&database, today, today).is_err());
        assert_eq!(trace_memo().get(&database).unwrap().observation_id, 1);
    }

    #[test]
    fn confirmed_missing_trace_is_an_authoritative_empty_result() {
        let fixture = TraceFixture::new();
        let database = fixture.0.join("logs_2.sqlite");
        let today = Date::from_calendar_date(2026, time::Month::August, 9).unwrap();
        let committed = TraceMemoState {
            observation_id: 1,
            coverage_start: today,
            last_row_id: 1,
            file_identity: TraceFileIdentity {
                device: 1,
                inode: 2,
            },
            sources: BTreeMap::new(),
        };
        trace_memo()
            .store_or_get_newer(&database, committed)
            .unwrap();
        let missing = load_fast_turns_from_database(&database, today, today).unwrap();

        assert!(missing.turns.is_empty());
        assert_eq!(missing.scanned_rows, 0);
        assert!(trace_memo().get(&database).is_none());
    }
}
