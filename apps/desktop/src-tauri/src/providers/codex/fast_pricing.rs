use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use time::{Date, Duration};

const MAX_TRACE_BODY_BYTES: usize = 1024 * 1024;
const MAX_TRACE_ROWS: usize = 16_384;
const MAX_EVENT_RECORDS_PER_BODY: usize = 4_096;
const MAX_COMPLETED_RESPONSES: usize = 16_384;
const COMPLETED_MARKER: &str = "websocket event:";

pub(super) struct FastTurnEvidence {
    pub(super) fingerprint: String,
    pub(super) turns: BTreeMap<String, Option<String>>,
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
    #[serde(default)]
    service_tier: Option<&'a str>,
}

struct CompletedResponseEvidence<'a> {
    turn_id: &'a str,
    model: Option<&'a str>,
    service_tier: Option<&'a str>,
}

struct TurnCompletionState {
    all_fast: bool,
    model: Option<String>,
    model_is_consistent: bool,
}

impl TurnCompletionState {
    fn from_evidence(evidence: &CompletedResponseEvidence<'_>) -> Self {
        let model = evidence.model.filter(|model| valid_model_name(model));
        Self {
            all_fast: evidence.service_tier.is_some_and(is_fast_tier),
            model: model.map(str::to_owned),
            model_is_consistent: model.is_some(),
        }
    }

    fn observe(&mut self, evidence: &CompletedResponseEvidence<'_>) {
        self.all_fast &= evidence.service_tier.is_some_and(is_fast_tier);
        let Some(model) = evidence.model.filter(|model| valid_model_name(model)) else {
            self.model_is_consistent = false;
            return;
        };
        if self.model.as_deref().is_some_and(|known| known != model) {
            self.model_is_consistent = false;
        } else if self.model.is_none() {
            self.model = Some(model.to_owned());
        }
    }

    fn fast_model(self) -> Option<String> {
        (self.all_fast && self.model_is_consistent)
            .then_some(self.model)
            .flatten()
    }
}

pub(super) fn load_fast_turn_evidence(
    codex_home: &Path,
    cutoff: Date,
    today: Date,
) -> Result<FastTurnEvidence, ()> {
    let turns = load_fast_turns_from_database(&codex_home.join("logs_2.sqlite"), cutoff, today)?;
    let fingerprint = fast_turn_fingerprint(&turns);
    Ok(FastTurnEvidence { fingerprint, turns })
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
               AND feedback_log_body LIKE '%response.completed%'
             ORDER BY rowid
             LIMIT ?3",
        )
        .map_err(|_| ())?;
    let limit = i64::try_from(MAX_TRACE_ROWS + 1).map_err(|_| ())?;
    let rows = statement
        .query_map(params![start, end, limit], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    let mut completed_turns = BTreeMap::new();
    let mut visited_rows = 0_usize;
    let mut completed_responses = 0_usize;
    for body in rows {
        visited_rows = visited_rows.checked_add(1).ok_or(())?;
        if visited_rows > MAX_TRACE_ROWS {
            return Err(());
        }
        let body = body.map_err(|_| ())?;
        if body.len() > MAX_TRACE_BODY_BYTES {
            return Err(());
        }
        collect_completed_responses(&mut completed_turns, &body, &mut completed_responses)?;
    }
    Ok(classified_fast_turns(completed_turns))
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

fn parse_completed_responses(body: &str) -> Result<Vec<CompletedResponseEvidence<'_>>, ()> {
    let mut responses = Vec::new();
    let mut active_turn_id = None;
    let mut event_records = 0_usize;
    for line in body.lines() {
        let Some(marker) = line.find(COMPLETED_MARKER) else {
            if line.contains("response.completed") {
                return Err(());
            }
            continue;
        };
        event_records = event_records.checked_add(1).ok_or(())?;
        if event_records > MAX_EVENT_RECORDS_PER_BODY
            || line[marker + COMPLETED_MARKER.len()..].contains(COMPLETED_MARKER)
        {
            return Err(());
        }
        let prefix = &line[..marker];
        let line_turn_id =
            match value_after(prefix, "turn.id=").or_else(|| value_after(prefix, "turn_id=")) {
                Some(value) if valid_turn_id(value) => Some(value),
                Some(_) => return Err(()),
                None => None,
            };
        if line_turn_id.is_some() {
            active_turn_id = line_turn_id;
        }
        let event: CompletedEvent<'_> =
            serde_json::from_str(line[marker + COMPLETED_MARKER.len()..].trim()).map_err(|_| ())?;
        if event.event_type != "response.completed" {
            continue;
        }
        let turn_id = line_turn_id.or(active_turn_id).ok_or(())?;
        responses.push(CompletedResponseEvidence {
            turn_id,
            model: event.response.as_ref().and_then(|response| response.model),
            service_tier: event
                .response
                .as_ref()
                .and_then(|response| response.service_tier),
        });
    }
    Ok(responses)
}

fn collect_completed_responses(
    completed_turns: &mut BTreeMap<String, TurnCompletionState>,
    body: &str,
    completed_responses: &mut usize,
) -> Result<(), ()> {
    for evidence in parse_completed_responses(body)? {
        *completed_responses = completed_responses.checked_add(1).ok_or(())?;
        if *completed_responses > MAX_COMPLETED_RESPONSES {
            return Err(());
        }
        completed_turns
            .entry(evidence.turn_id.to_owned())
            .and_modify(|state| state.observe(&evidence))
            .or_insert_with(|| TurnCompletionState::from_evidence(&evidence));
    }
    Ok(())
}

fn classified_fast_turns(
    completed_turns: BTreeMap<String, TurnCompletionState>,
) -> BTreeMap<String, Option<String>> {
    completed_turns
        .into_iter()
        .filter_map(|(turn_id, state)| state.fast_model().map(|model| (turn_id, Some(model))))
        .collect()
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

    fn classify(bodies: &[&str]) -> Result<BTreeMap<String, Option<String>>, ()> {
        let mut completed_turns = BTreeMap::new();
        let mut completed_responses = 0;
        for body in bodies {
            collect_completed_responses(&mut completed_turns, body, &mut completed_responses)?;
        }
        Ok(classified_fast_turns(completed_turns))
    }

    #[test]
    fn production_shaped_event_bundle_returns_only_bounded_pricing_metadata() {
        let priority = concat!(
            "private=ignored turn.id=turn-safe websocket event: {\"type\":\"response.created\",\"response\":{\"private\":\"ignored\"}}\n",
            "private=ignored websocket event: {\"type\":\"response.output_item.done\",\"item\":{\"private\":\"ignored\"}}\n",
            "private=ignored websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-terra\",\"service_tier\":\"priority\",\"private\":\"ignored\"}}\n",
            "private=ignored websocket event: {\"type\":\"response.done\",\"private\":\"ignored\"}\n",
        );
        let fast = r#"private=ignored turn_id=turn-fast websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol","service_tier":"fast"}}"#;

        assert_eq!(
            classify(&[priority, fast]).unwrap(),
            BTreeMap::from([
                ("turn-fast".to_owned(), Some("gpt-5.6-sol".to_owned())),
                ("turn-safe".to_owned(), Some("gpt-5.6-terra".to_owned())),
            ])
        );
        assert!(
            parse_completed_responses(&format!(
                "turn.id={} websocket event: {{\"type\":\"response.completed\",\"response\":{{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"priority\"}}}}",
                "x".repeat(129)
            ))
            .is_err()
        );
    }

    #[test]
    fn request_or_unproved_completion_does_not_prove_fast_mode() {
        let request = r#"turn.id=turn-safe websocket request: {"type":"response.create","service_tier":"fast","model":"gpt-5.6-terra"}"#;
        let downgraded = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra","service_tier":"default"}}"#;
        let missing_tier = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra"}}"#;
        let unrelated = r#"turn.id=turn-safe websocket event: {"type":"response.output_text.done","response":{"model":"gpt-5.6-terra","service_tier":"priority"}}"#;

        assert!(classify(&[request]).unwrap().is_empty());
        assert!(classify(&[downgraded]).unwrap().is_empty());
        assert!(classify(&[missing_tier]).unwrap().is_empty());
        assert!(classify(&[unrelated]).unwrap().is_empty());
    }

    #[test]
    fn mixed_completed_tiers_or_models_do_not_prove_fast_mode() {
        let priority = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol","service_tier":"priority"}}"#;
        let standard = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-sol","service_tier":"default"}}"#;
        let conflicting_model = r#"turn.id=turn-safe websocket event: {"type":"response.completed","response":{"model":"gpt-5.6-terra","service_tier":"priority"}}"#;

        assert!(classify(&[priority, standard]).unwrap().is_empty());
        assert!(classify(&[priority, conflicting_model]).unwrap().is_empty());
    }
}
