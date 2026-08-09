use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use time::{Date, Duration};

const MAX_TRACE_BODY_BYTES: usize = 1024 * 1024;
const MAX_FAST_TURNS: usize = 16_384;
const REQUEST_MARKER: &str = "websocket request:";
const COMPLETED_MARKER: &str = "websocket event:";

pub(super) struct FastTurnEvidence {
    pub(super) fingerprint: String,
    pub(super) turns: BTreeMap<String, Option<String>>,
}

#[derive(Deserialize)]
struct FastRequest<'a> {
    #[serde(rename = "type")]
    request_type: &'a str,
    service_tier: &'a str,
    #[serde(default)]
    turn_id: Option<&'a str>,
    #[serde(default)]
    model: Option<&'a str>,
}

#[derive(Deserialize)]
struct CompletedEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'a str,
    response: CompletedResponse<'a>,
}

#[derive(Deserialize)]
struct CompletedResponse<'a> {
    model: &'a str,
}

pub(super) fn load_fast_turn_evidence(
    codex_home: &Path,
    cutoff: Date,
    today: Date,
) -> Option<FastTurnEvidence> {
    let turns =
        load_fast_turns_from_database(&codex_home.join("logs_2.sqlite"), cutoff, today).ok()?;
    let fingerprint = fast_turn_fingerprint(&turns);
    Some(FastTurnEvidence { fingerprint, turns })
}

fn load_fast_turns_from_database(
    database: &Path,
    cutoff: Date,
    today: Date,
) -> Result<BTreeMap<String, Option<String>>, ()> {
    if !database.is_file() {
        return Ok(BTreeMap::new());
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|_| ())?;
    let start = cutoff.midnight().assume_utc().unix_timestamp();
    let end = (today + Duration::days(1))
        .midnight()
        .assume_utc()
        .unix_timestamp();
    let mut statement = connection
        .prepare(
            "SELECT feedback_log_body
             FROM logs
             WHERE ts >= ?1 AND ts < ?2
               AND (feedback_log_body LIKE '%websocket request:%'
                    OR feedback_log_body LIKE '%response.completed%'
                    OR feedback_log_body LIKE '%service_tier: Some(Some(\"priority\"))%'
                    OR feedback_log_body LIKE '%service_tier: Some(Some(\"fast\"))%')
             ORDER BY rowid
             LIMIT ?3",
        )
        .map_err(|_| ())?;
    let limit = i64::try_from(MAX_FAST_TURNS + 1).map_err(|_| ())?;
    let rows = statement
        .query_map(params![start, end, limit], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    let mut turns = BTreeMap::new();
    let mut completed_models = BTreeMap::new();
    let mut visited = 0_usize;
    for body in rows {
        visited = visited.checked_add(1).ok_or(())?;
        if visited > MAX_FAST_TURNS {
            return Err(());
        }
        let body = body.map_err(|_| ())?;
        if body.len() > MAX_TRACE_BODY_BYTES {
            continue;
        }
        if let Some((turn_id, model)) = parse_completed_model(&body) {
            completed_models.insert(turn_id.to_owned(), model.to_owned());
            if let Some(fast_model) = turns.get_mut(turn_id) {
                *fast_model = Some(model.to_owned());
            }
            continue;
        }
        if let Some((turn_id, request_model)) = parse_fast_turn(&body) {
            let model = completed_models
                .get(turn_id)
                .map(String::as_str)
                .or(request_model)
                .filter(|model| valid_model_name(model))
                .map(str::to_owned);
            turns.insert(turn_id.to_owned(), model);
        }
    }
    Ok(turns)
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

fn parse_fast_turn(body: &str) -> Option<(&str, Option<&str>)> {
    if let Some(marker) = body.find(REQUEST_MARKER) {
        let prefix = &body[..marker];
        let request: FastRequest<'_> =
            serde_json::from_str(body[marker + REQUEST_MARKER.len()..].trim()).ok()?;
        if request.request_type != "response.create" || !is_fast_tier(request.service_tier) {
            return None;
        }
        let turn_id = value_after(prefix, "turn.id=")
            .or_else(|| value_after(prefix, "turn_id="))
            .or(request.turn_id)
            .filter(|value| valid_turn_id(value))?;
        return Some((turn_id, request.model));
    }

    let is_fast_submission = body.contains("service_tier: Some(Some(\"priority\"))")
        || body.contains("service_tier: Some(Some(\"fast\"))");
    if !is_fast_submission {
        return None;
    }
    let submission = body.split_once("Submission sub=Submission {")?.1;
    let turn_id = quoted_value_after(submission, "id: \"").filter(|value| valid_turn_id(value))?;
    Some((turn_id, None))
}

fn parse_completed_model(body: &str) -> Option<(&str, &str)> {
    let marker = body.find(COMPLETED_MARKER)?;
    let prefix = &body[..marker];
    let event: CompletedEvent<'_> =
        serde_json::from_str(body[marker + COMPLETED_MARKER.len()..].trim()).ok()?;
    if event.event_type != "response.completed" || !valid_model_name(event.response.model) {
        return None;
    }
    let turn_id = value_after(prefix, "turn.id=")
        .or_else(|| value_after(prefix, "turn_id="))
        .filter(|value| valid_turn_id(value))?;
    Some((turn_id, event.response.model))
}

fn is_fast_tier(value: &str) -> bool {
    matches!(value, "priority" | "fast")
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
    let value = tail.split_once('"')?.0;
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

    #[test]
    fn exact_fast_trace_shapes_return_only_bounded_pricing_metadata() {
        let websocket = r#"thread_id=ignored turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"fast","turn_id":"fallback","model":"gpt-5.6-sol"}"#;
        let priority = r#"thread_id=ignored turn_id=turn-priority websocket request: {"type":"response.create","service_tier":"priority"}"#;
        let standard = r#"turn.id=standard websocket request: {"type":"response.create","service_tier":"default"}"#;
        let submission = r#"service_tier: Some(Some("priority")) Submission sub=Submission { id: "turn-submission", private: "ignored" }"#;

        assert_eq!(
            parse_fast_turn(websocket),
            Some(("turn-safe", Some("gpt-5.6-sol")))
        );
        assert_eq!(parse_fast_turn(priority), Some(("turn-priority", None)));
        assert_eq!(parse_fast_turn(standard), None);
        assert_eq!(parse_fast_turn(submission), Some(("turn-submission", None)));
        assert_eq!(
            parse_fast_turn(&format!(
                "turn.id={} websocket request: {{\"type\":\"response.create\",\"service_tier\":\"fast\"}}",
                "x".repeat(129)
            )),
            None
        );
    }

    #[test]
    fn completed_event_returns_only_a_bounded_model_for_a_bounded_turn() {
        let completed = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra","private":"ignored"}}"#;
        let unrelated = r#"turn.id=turn-safe websocket event: {"type":"response.output_text.done","response":{"model":"gpt-5.6-terra"}}"#;

        assert_eq!(
            parse_completed_model(completed),
            Some(("turn-safe", "gpt-5.6-terra"))
        );
        assert_eq!(parse_completed_model(unrelated), None);
    }
}
