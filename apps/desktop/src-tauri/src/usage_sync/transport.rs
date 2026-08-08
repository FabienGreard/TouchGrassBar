//! Protected transport for current-day Daily Usage Aggregate synchronization.

use std::{collections::BTreeMap, io::Read, time::Duration};

use convex::{ConvexClient, FunctionResult, Value};
use serde::{Deserialize, Deserializer};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::profile::Secret;

use super::{PendingUsageBatch, UsageSyncAcknowledgement, parse_usage_acknowledgements};

const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const DAILY_USAGE_MUTATION: &str = "sync:dailyUsage";
const MAX_TOKEN_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_CONVEX_JWT_BYTES: usize = 8 * 1_024;
const MAX_ACKNOWLEDGEMENT_BYTES: usize = 64 * 1_024;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const SYNC_MUTATION_TIMEOUT: Duration = Duration::from_secs(30);

/// The only outcomes that the native coordinator can use.
#[derive(Debug, PartialEq)]
pub(crate) enum UsageSyncTransportOutcome {
    Committed(Vec<UsageSyncAcknowledgement>),
    Offline,
    SessionRejected,
    AuthorityRejected,
    Deferred,
}

pub(crate) struct HttpUsageSyncTransport {
    auth_site_url: Option<&'static str>,
    convex_url: Option<&'static str>,
    client: reqwest::blocking::Client,
}

impl HttpUsageSyncTransport {
    pub(crate) fn from_build_configuration() -> Self {
        Self {
            auth_site_url: option_env!("CONVEX_SITE_URL").filter(|value| !value.is_empty()),
            convex_url: option_env!("CONVEX_URL").filter(|value| !value.is_empty()),
            client: crate::native_https_client(),
        }
    }

    /// Fetch one fresh Convex JWT and send one exact outbox batch.
    pub(crate) fn send(
        &self,
        session: &Secret,
        installation_credential: &Secret,
        batch: &PendingUsageBatch,
        now: OffsetDateTime,
    ) -> UsageSyncTransportOutcome {
        let args = match mutation_arguments(batch, installation_credential.expose(), now) {
            Ok(args) => args,
            Err(()) => return UsageSyncTransportOutcome::Deferred,
        };
        let jwt = match self.fetch_convex_jwt(session) {
            TokenFetchOutcome::Jwt(jwt) => jwt,
            TokenFetchOutcome::Offline => return UsageSyncTransportOutcome::Offline,
            TokenFetchOutcome::SessionRejected => {
                return UsageSyncTransportOutcome::SessionRejected;
            }
            TokenFetchOutcome::Deferred => return UsageSyncTransportOutcome::Deferred,
        };
        let Some(convex_url) = self.convex_url else {
            return UsageSyncTransportOutcome::Deferred;
        };
        send_with_runtime(convex_url, jwt, args)
    }

    fn fetch_convex_jwt(&self, session: &Secret) -> TokenFetchOutcome {
        let Some(auth_site_url) = self.auth_site_url else {
            return TokenFetchOutcome::Deferred;
        };
        let endpoint = format!("{}{CONVEX_TOKEN_PATH}", auth_site_url.trim_end_matches('/'));
        let response = match self
            .client
            .get(endpoint)
            .bearer_auth(session.expose())
            .send()
        {
            Ok(response) => response,
            Err(_) => return TokenFetchOutcome::Offline,
        };
        match classify_token_http_status(response.status().as_u16()) {
            TokenHttpOutcome::Success => {}
            TokenHttpOutcome::SessionRejected => {
                return TokenFetchOutcome::SessionRejected;
            }
            TokenHttpOutcome::Deferred => return TokenFetchOutcome::Deferred,
        }

        let mut body = Zeroizing::new(Vec::with_capacity(MAX_TOKEN_RESPONSE_BYTES));
        if response
            .take((MAX_TOKEN_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .is_err()
        {
            return TokenFetchOutcome::Offline;
        }
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            return TokenFetchOutcome::Deferred;
        }
        let response: ConvexTokenResponse = match serde_json::from_slice(body.as_slice()) {
            Ok(response) => response,
            Err(_) => return TokenFetchOutcome::Deferred,
        };
        if response.token.is_empty() || response.token.len() > MAX_CONVEX_JWT_BYTES {
            return TokenFetchOutcome::Deferred;
        }
        TokenFetchOutcome::Jwt(response.token)
    }
}

fn send_with_runtime(
    convex_url: &str,
    jwt: Zeroizing<String>,
    args: BTreeMap<String, Value>,
) -> UsageSyncTransportOutcome {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => return UsageSyncTransportOutcome::Deferred,
    };
    runtime.block_on(async move {
        match tokio::time::timeout(SYNC_MUTATION_TIMEOUT, send_mutation(convex_url, jwt, args))
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => UsageSyncTransportOutcome::Offline,
        }
    })
}

enum TokenFetchOutcome {
    Jwt(Zeroizing<String>),
    Offline,
    SessionRejected,
    Deferred,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenHttpOutcome {
    Success,
    SessionRejected,
    Deferred,
}

fn classify_token_http_status(status: u16) -> TokenHttpOutcome {
    match status {
        200..=299 => TokenHttpOutcome::Success,
        401 | 403 => TokenHttpOutcome::SessionRejected,
        _ => TokenHttpOutcome::Deferred,
    }
}

#[derive(Deserialize)]
struct ConvexTokenResponse {
    #[serde(deserialize_with = "deserialize_zeroizing_string")]
    token: Zeroizing<String>,
}

fn deserialize_zeroizing_string<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

async fn send_mutation(
    convex_url: &str,
    jwt: Zeroizing<String>,
    args: BTreeMap<String, Value>,
) -> UsageSyncTransportOutcome {
    let mut client = match ConvexClient::new(convex_url).await {
        Ok(client) => client,
        Err(_) => return UsageSyncTransportOutcome::Offline,
    };
    client.set_auth(Some(jwt.as_str().to_owned())).await;
    let result = client.mutation(DAILY_USAGE_MUTATION, args).await;
    client.set_auth(None).await;
    match result {
        Ok(result) => classify_function_result(result),
        Err(_) => UsageSyncTransportOutcome::Offline,
    }
}

fn mutation_arguments(
    batch: &PendingUsageBatch,
    installation_credential: &str,
    now: OffsetDateTime,
) -> Result<BTreeMap<String, Value>, ()> {
    let allowed = batch
        .mutation_args(installation_credential, now)
        .map_err(|_| ())?;
    let serialized = serde_json::to_value(allowed).map_err(|_| ())?;
    let Value::Object(args) = request_json_to_convex(serialized)? else {
        return Err(());
    };
    Ok(args)
}

fn request_json_to_convex(value: JsonValue) -> Result<Value, ()> {
    match value {
        JsonValue::Null => Ok(Value::Null),
        JsonValue::Bool(value) => Ok(Value::Boolean(value)),
        JsonValue::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Value::Float64)
            .ok_or(()),
        JsonValue::String(value) => Ok(Value::String(value)),
        JsonValue::Array(values) => values
            .into_iter()
            .map(request_json_to_convex)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        JsonValue::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key, request_json_to_convex(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Object),
    }
}

fn classify_function_result(result: FunctionResult) -> UsageSyncTransportOutcome {
    match result {
        FunctionResult::Value(value) => parse_success_value(value)
            .map(UsageSyncTransportOutcome::Committed)
            .unwrap_or(UsageSyncTransportOutcome::Deferred),
        FunctionResult::ConvexError(error) if is_exact_authority_rejection(&error.data) => {
            UsageSyncTransportOutcome::AuthorityRejected
        }
        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
            UsageSyncTransportOutcome::Deferred
        }
    }
}

fn is_exact_authority_rejection(data: &Value) -> bool {
    let Value::Object(fields) = data else {
        return false;
    };
    fields.len() == 1
        && matches!(
            fields.get("code"),
            Some(Value::String(code)) if code == "authority-rejected"
        )
}

fn parse_success_value(value: Value) -> Result<Vec<UsageSyncAcknowledgement>, ()> {
    let mut budget = MAX_ACKNOWLEDGEMENT_BYTES;
    let json = response_convex_to_json(value, &mut budget)?;
    let encoded = serde_json::to_vec(&json).map_err(|_| ())?;
    if encoded.len() > MAX_ACKNOWLEDGEMENT_BYTES {
        return Err(());
    }
    parse_usage_acknowledgements(&encoded).map_err(|_| ())
}

fn response_convex_to_json(value: Value, budget: &mut usize) -> Result<JsonValue, ()> {
    consume_budget(budget, 1)?;
    match value {
        Value::Null => Ok(JsonValue::Null),
        Value::Boolean(value) => Ok(JsonValue::Bool(value)),
        Value::Float64(value) => response_number(value).map(JsonValue::Number),
        Value::String(value) => {
            consume_budget(budget, value.len())?;
            Ok(JsonValue::String(value))
        }
        Value::Array(values) => values
            .into_iter()
            .map(|value| response_convex_to_json(value, budget))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        Value::Object(values) => {
            let mut output = JsonMap::new();
            for (key, value) in values {
                consume_budget(budget, key.len())?;
                output.insert(key, response_convex_to_json(value, budget)?);
            }
            Ok(JsonValue::Object(output))
        }
        Value::Int64(_) | Value::Bytes(_) => Err(()),
    }
}

fn response_number(value: f64) -> Result<JsonNumber, ()> {
    if !value.is_finite() {
        return Err(());
    }
    if (0.0..=MAX_SAFE_INTEGER).contains(&value) && value.fract() == 0.0 {
        return Ok(JsonNumber::from(value as u64));
    }
    JsonNumber::from_f64(value).ok_or(())
}

fn consume_budget(budget: &mut usize, amount: usize) -> Result<(), ()> {
    *budget = budget.checked_sub(amount).ok_or(())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use convex::{ConvexError, FunctionResult, Value};
    use rusqlite::Connection;
    use serde_json::json;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::{
        sanitized::CodingProvider,
        usage_sync::{
            CorrectionReason, DailyUsageAggregate, SyncCoverage, SyncEvidenceBasis,
            install_usage_sync_schema, load_pending_usage_batch, queue_daily_aggregate,
        },
    };

    const NOW: &str = "2026-08-08T12:34:56Z";
    const DAY_START_MILLIS: u64 = 1_786_147_200_000;
    const INSTALLATION_CREDENTIAL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse(NOW, &Rfc3339).unwrap()
    }

    #[test]
    fn mutation_timeout_is_created_inside_the_owned_runtime() {
        let outcome = send_with_runtime(
            "not-a-convex-url",
            Zeroizing::new("test-jwt".to_owned()),
            BTreeMap::new(),
        );

        assert_eq!(outcome, UsageSyncTransportOutcome::Offline);
    }

    fn batch() -> PendingUsageBatch {
        let mut connection = Connection::open_in_memory().unwrap();
        install_usage_sync_schema(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            4,
            DailyUsageAggregate {
                provider: CodingProvider::Codex,
                ranking_day: "2026-08-08".to_owned(),
                evidence_basis: SyncEvidenceBasis::ProviderReported,
                coverage: SyncCoverage::Complete,
                observed_at: DAY_START_MILLIS + 1_000,
                observed_tokens: 12,
                api_equivalent_cost: None,
                correction_reason: None,
            },
            now(),
        )
        .unwrap();
        queue_daily_aggregate(
            &transaction,
            4,
            DailyUsageAggregate {
                provider: CodingProvider::Claude,
                ranking_day: "2026-08-08".to_owned(),
                evidence_basis: SyncEvidenceBasis::LocallyDerived,
                coverage: SyncCoverage::Complete,
                observed_at: DAY_START_MILLIS + 2_000,
                observed_tokens: 13,
                api_equivalent_cost: None,
                correction_reason: None,
            },
            now(),
        )
        .unwrap();
        transaction.commit().unwrap();
        load_pending_usage_batch(&connection, 4).unwrap().unwrap()
    }

    fn acknowledgement_value() -> Value {
        Value::Array(vec![Value::Object(BTreeMap::from([
            ("outcome".to_owned(), Value::String("committed".to_owned())),
            ("provider".to_owned(), Value::String("codex".to_owned())),
            (
                "rankingDay".to_owned(),
                Value::String("2026-08-08".to_owned()),
            ),
            ("revision".to_owned(), Value::Float64(1.0)),
        ]))])
    }

    fn corrected_batch() -> PendingUsageBatch {
        let mut connection = Connection::open_in_memory().unwrap();
        install_usage_sync_schema(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            4,
            DailyUsageAggregate {
                provider: CodingProvider::Claude,
                ranking_day: "2026-08-08".to_owned(),
                evidence_basis: SyncEvidenceBasis::LocallyDerived,
                coverage: SyncCoverage::Complete,
                observed_at: DAY_START_MILLIS + 1_000,
                observed_tokens: 100,
                api_equivalent_cost: None,
                correction_reason: None,
            },
            now(),
        )
        .unwrap();
        transaction.commit().unwrap();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            4,
            DailyUsageAggregate {
                provider: CodingProvider::Claude,
                ranking_day: "2026-08-08".to_owned(),
                evidence_basis: SyncEvidenceBasis::LocallyDerived,
                coverage: SyncCoverage::Complete,
                observed_at: DAY_START_MILLIS + 2_000,
                observed_tokens: 80,
                api_equivalent_cost: None,
                correction_reason: None,
            }
            .with_correction(CorrectionReason::ParserCorrection),
            now(),
        )
        .unwrap();
        transaction.commit().unwrap();
        load_pending_usage_batch(&connection, 4).unwrap().unwrap()
    }

    #[test]
    fn converts_the_exact_allowlisted_request_to_convex_values() {
        let args = mutation_arguments(&batch(), INSTALLATION_CREDENTIAL, now()).unwrap();
        let value = JsonValue::from(Value::Object(args));

        assert_eq!(
            value,
            json!({
                "activeMacGeneration": 4.0,
                "installationCredential": INSTALLATION_CREDENTIAL,
                "snapshots": [
                    {
                        "apiEquivalentCost": null,
                        "correctionReason": null,
                        "correctionRevision": null,
                        "coverage": "complete",
                        "evidenceBasis": "locally-derived",
                        "observedAt": (DAY_START_MILLIS + 2_000) as f64,
                        "observedTokens": 13.0,
                        "provider": "claude",
                        "rankingDay": "2026-08-08",
                        "revision": 1.0
                    },
                    {
                        "apiEquivalentCost": null,
                        "correctionReason": null,
                        "correctionRevision": null,
                        "coverage": "complete",
                        "evidenceBasis": "provider-reported",
                        "observedAt": (DAY_START_MILLIS + 1_000) as f64,
                        "observedTokens": 12.0,
                        "provider": "codex",
                        "rankingDay": "2026-08-08",
                        "revision": 1.0
                    }
                ]
            })
        );
    }

    #[test]
    fn converts_the_exact_correction_pair_to_convex_values() {
        let args = mutation_arguments(&corrected_batch(), INSTALLATION_CREDENTIAL, now()).unwrap();
        let value = JsonValue::from(Value::Object(args));

        assert_eq!(
            value,
            json!({
                "activeMacGeneration": 4.0,
                "installationCredential": INSTALLATION_CREDENTIAL,
                "snapshots": [
                    {
                        "apiEquivalentCost": null,
                        "correctionReason": "parser-correction",
                        "correctionRevision": 2.0,
                        "coverage": "complete",
                        "evidenceBasis": "locally-derived",
                        "observedAt": (DAY_START_MILLIS + 2_000) as f64,
                        "observedTokens": 80.0,
                        "provider": "claude",
                        "rankingDay": "2026-08-08",
                        "revision": 2.0
                    }
                ]
            })
        );
    }

    #[test]
    fn accepts_only_the_exact_structured_authority_rejection() {
        let exact = FunctionResult::ConvexError(ConvexError {
            message: "redacted by this boundary".to_owned(),
            data: Value::Object(BTreeMap::from([(
                "code".to_owned(),
                Value::String("authority-rejected".to_owned()),
            )])),
        });
        assert!(matches!(
            classify_function_result(exact),
            UsageSyncTransportOutcome::AuthorityRejected
        ));

        let with_extra_data = FunctionResult::ConvexError(ConvexError {
            message: "not used".to_owned(),
            data: Value::Object(BTreeMap::from([
                (
                    "code".to_owned(),
                    Value::String("authority-rejected".to_owned()),
                ),
                ("detail".to_owned(), Value::String("extra".to_owned())),
            ])),
        });
        assert!(matches!(
            classify_function_result(with_extra_data),
            UsageSyncTransportOutcome::Deferred
        ));
    }

    #[test]
    fn token_rejection_requires_session_refresh_not_active_mac_rejection() {
        assert_eq!(
            classify_token_http_status(401),
            TokenHttpOutcome::SessionRejected
        );
        assert_eq!(
            classify_token_http_status(403),
            TokenHttpOutcome::SessionRejected
        );
        for status in [400, 404, 409, 429, 500] {
            assert_eq!(
                classify_token_http_status(status),
                TokenHttpOutcome::Deferred
            );
        }
        assert_eq!(classify_token_http_status(200), TokenHttpOutcome::Success);
    }

    #[test]
    fn malformed_success_values_are_deferred() {
        let malformed = Value::Array(vec![Value::Object(BTreeMap::from([
            ("outcome".to_owned(), Value::String("committed".to_owned())),
            ("provider".to_owned(), Value::String("codex".to_owned())),
        ]))]);
        assert!(matches!(
            classify_function_result(FunctionResult::Value(malformed)),
            UsageSyncTransportOutcome::Deferred
        ));
        assert!(matches!(
            classify_function_result(FunctionResult::Value(Value::Int64(1))),
            UsageSyncTransportOutcome::Deferred
        ));
    }

    #[test]
    fn valid_acknowledgements_commit_without_private_error_detail() {
        let outcome = classify_function_result(FunctionResult::Value(acknowledgement_value()));
        assert!(matches!(
            outcome,
            UsageSyncTransportOutcome::Committed(ref values) if values.len() == 1
        ));

        let private_detail = "session-private-value";
        let deferred =
            classify_function_result(FunctionResult::ErrorMessage(private_detail.to_owned()));
        assert_eq!(format!("{deferred:?}"), "Deferred");
        assert!(!format!("{deferred:?}").contains(private_detail));
    }
}
