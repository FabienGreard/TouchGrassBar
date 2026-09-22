//! Independent response accounting. Cumulative notifications remain legacy evidence.
use super::{RequiredWhenPresent, TokenUsage, normalized_session_id, usage_is_at_or_below};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::Date;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseCursor {
    response_id: String,
    total: TokenUsage,
    usage: TokenUsage,
    #[serde(default)]
    incomplete_days: BTreeSet<String>,
    #[serde(default)]
    observed_day: Option<String>,
}

impl ResponseCursor {
    pub(super) fn record_day(&mut self, day: Date, cutoff: Date, complete: bool) -> Result<(), ()> {
        let previous = self
            .observed_day
            .as_deref()
            .map(super::parse_ranking_day)
            .transpose()?;
        self.incomplete_days
            .retain(|day| day.as_str() >= cutoff.to_string().as_str());
        if !complete {
            let mut missing = previous.unwrap_or(day).min(day).max(cutoff);
            while missing <= day {
                self.incomplete_days.insert(missing.to_string());
                missing = missing.next_day().ok_or(())?;
            }
        }
        self.observed_day = Some(
            previous
                .map_or(day, |previous| previous.max(day))
                .to_string(),
        );
        Ok(())
    }

    pub(super) fn incomplete_days(&self) -> Result<BTreeSet<Date>, ()> {
        self.incomplete_days
            .iter()
            .map(|day| super::parse_ranking_day(day))
            .collect()
    }

    pub(super) fn decode(value: &str) -> Result<Self, ()> {
        if value.len() > 4096 {
            return Err(());
        }
        let cursor: Self = serde_json::from_str(value).map_err(|_| ())?;
        if cursor.incomplete_days.len() > 60 {
            return Err(());
        }
        for day in &cursor.incomplete_days {
            super::parse_ranking_day(day)?;
        }
        cursor
            .observed_day
            .as_deref()
            .map(super::parse_ranking_day)
            .transpose()?;
        cursor.total.validate()?;
        cursor.usage.validate()?;
        cursor.total.delta_from(cursor.usage)?;
        normalized_session_id(Some(cursor.response_id.clone())).ok_or(())?;
        Ok(cursor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    #[serde(default, rename = "ordinal")]
    _ordinal: RequiredWhenPresent<u64>,
    #[serde(rename = "timestamp")]
    _timestamp: String,
    #[serde(rename = "type")]
    _kind: String,
    payload: ResponseRecord,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseRecord {
    thread_id: String,
    turn_id: String,
    #[serde(rename = "session_id")]
    _session_id: String,
    #[serde(rename = "root_turn_id")]
    _root_turn_id: String,
    response_id: String,
    usage: TokenUsage,
    #[serde(rename = "turn_token_usage")]
    _turn_total: TokenUsage,
    thread_token_usage: TokenUsage,
}

pub(super) struct Observation {
    pub tokens: TokenUsage,
    pub complete: bool,
}

impl ResponseRecord {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, ()> {
        let record = serde_json::from_slice::<Line>(bytes)
            .map_err(|_| ())?
            .payload;
        record.usage.validate()?;
        record.thread_token_usage.validate()?;
        record._turn_total.validate()?;
        record._turn_total.delta_from(record.usage)?;
        record.thread_token_usage.delta_from(record.usage)?;
        for value in [&record.thread_id, &record.turn_id, &record.response_id] {
            normalized_session_id(Some(value.clone())).ok_or(())?;
        }
        Ok(record)
    }
    pub(super) fn thread_id(&self) -> &str {
        &self.thread_id
    }
    pub(super) fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub(super) fn observe(
        &self,
        cursor: &mut Option<ResponseCursor>,
        legacy: Option<TokenUsage>,
    ) -> Result<Observation, ()> {
        let total = self.thread_token_usage;
        let mut complete = true;
        let observed_day = cursor
            .as_ref()
            .and_then(|cursor| cursor.observed_day.clone());
        let incomplete_days = cursor
            .as_ref()
            .map(|cursor| cursor.incomplete_days.clone())
            .unwrap_or_default();
        if let Some(previous) = cursor {
            if self.response_id == previous.response_id {
                if total != previous.total || self.usage != previous.usage {
                    return Err(());
                }
                return Ok(Observation {
                    tokens: TokenUsage::default(),
                    complete: true,
                });
            }
            if usage_is_at_or_below(total, previous.total) {
                return Ok(Observation {
                    tokens: TokenUsage::default(),
                    complete: true,
                });
            }
            let delta = total.delta_from(previous.total)?;
            if !usage_is_at_or_below(self.usage, delta) {
                return Err(());
            }
            complete = delta == self.usage;
        } else if let Some(previous) = legacy {
            // A stream that changes format needs a proved handover. Do not
            // count an ambiguous first response again; later records are exact.
            let delta = total.delta_from(previous).ok();
            if delta != Some(self.usage) {
                *cursor = Some(ResponseCursor {
                    response_id: self.response_id.clone(),
                    total,
                    usage: self.usage,
                    incomplete_days,
                    observed_day,
                });
                return Ok(Observation {
                    tokens: TokenUsage::default(),
                    complete: false,
                });
            }
        }
        *cursor = Some(ResponseCursor {
            response_id: self.response_id.clone(),
            total,
            usage: self.usage,
            incomplete_days,
            observed_day,
        });
        Ok(Observation {
            tokens: self.usage,
            complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(tokens: u64) -> TokenUsage {
        TokenUsage {
            input: tokens,
            total: tokens,
            ..TokenUsage::default()
        }
    }

    fn record(id: &str, tokens: u64, total: u64) -> ResponseRecord {
        ResponseRecord {
            thread_id: "task".into(),
            turn_id: "turn".into(),
            _session_id: "session".into(),
            _root_turn_id: "turn".into(),
            response_id: id.into(),
            usage: usage(tokens),
            _turn_total: usage(total),
            thread_token_usage: usage(total),
        }
    }

    #[test]
    fn saved_cursor_rejects_conflicting_duplicates_and_skips_old_responses() {
        let mut cursor = None;
        assert_eq!(
            record("one", 100, 1100)
                .observe(&mut cursor, None)
                .unwrap()
                .tokens
                .total,
            100
        );
        let saved = serde_json::to_string(cursor.as_ref().unwrap()).unwrap();
        let mut cursor = Some(ResponseCursor::decode(&saved).unwrap());
        assert_eq!(
            record("one", 100, 1100)
                .observe(&mut cursor, None)
                .unwrap()
                .tokens
                .total,
            0
        );
        assert!(record("one", 99, 1100).observe(&mut cursor, None).is_err());
        assert_eq!(
            record("older", 50, 1000)
                .observe(&mut cursor, None)
                .unwrap()
                .tokens
                .total,
            0
        );
        assert_eq!(
            record("two", 100, 1200)
                .observe(&mut cursor, None)
                .unwrap()
                .tokens
                .total,
            100
        );
    }

    #[test]
    fn many_responses_remain_exact_across_restarts_and_duplicate_notifications() {
        let mut cursor = None;
        let mut cumulative = 10_000;
        let mut counted = 0;
        let mut expected = 0;
        for index in 0..1000 {
            let tokens = (index * 37 % 97) + 1;
            cumulative += tokens;
            expected += tokens;
            let next = record(&format!("response-{index}"), tokens, cumulative);
            counted += next.observe(&mut cursor, None).unwrap().tokens.total;
            if index % 13 == 0 {
                counted += next.observe(&mut cursor, None).unwrap().tokens.total;
            }
            if index % 17 == 0 {
                cursor = Some(
                    ResponseCursor::decode(
                        &serde_json::to_string(cursor.as_ref().unwrap()).unwrap(),
                    )
                    .unwrap(),
                );
            }
            assert_eq!(counted, expected);
        }
    }

    #[test]
    fn gaps_keep_only_observed_usage_and_mark_coverage_partial() {
        let mut cursor = None;
        record("one", 100, 1100).observe(&mut cursor, None).unwrap();
        let next = record("three", 100, 1500)
            .observe(&mut cursor, None)
            .unwrap();
        assert_eq!(next.tokens.total, 100);
        assert!(!next.complete);
        assert!(
            record("four", 100, 1600)
                .observe(&mut cursor, None)
                .unwrap()
                .complete
        );
    }

    #[test]
    fn legacy_transition_requires_matching_counters() {
        let mut exact = None;
        let known = record("one", 100, 1100)
            .observe(&mut exact, Some(usage(1000)))
            .unwrap();
        assert!(known.complete);
        assert_eq!(known.tokens.total, 100);
        let mut ambiguous = None;
        let unknown = record("one", 100, 1150)
            .observe(&mut ambiguous, Some(usage(1000)))
            .unwrap();
        assert!(!unknown.complete);
        assert_eq!(unknown.tokens.total, 0);
        assert_eq!(
            record("two", 50, 1200)
                .observe(&mut ambiguous, Some(usage(1100)))
                .unwrap()
                .tokens
                .total,
            50
        );
    }

    #[test]
    fn malformed_response_arithmetic_and_saved_state_fail_closed() {
        assert!(ResponseCursor::decode(r#"{"response_id":"one","total":{},"usage":{}}"#).is_err());
        let mut value = json!({"timestamp":"2026-09-22T23:59:59Z","type":"token_usage_record", "payload": {
            "thread_id":"task","turn_id":"turn","session_id":"session","root_turn_id":"turn",
            "response_id":"one","usage":usage(100),"turn_token_usage":usage(100),"thread_token_usage":usage(100)
        }});
        assert!(ResponseRecord::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
        value["payload"]["usage"]["total_tokens"] = json!(101);
        assert!(ResponseRecord::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
