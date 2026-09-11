//! Protected transport for bounded Daily Usage Snapshot synchronization.

use std::{collections::BTreeMap, io::Read, time::Duration};

use convex::{ConvexClient, FunctionResult, Value};
use serde::{Deserialize, Deserializer};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::diagnostics::{self, Failure, Provider, SyncCode, SyncContext, SyncReason, SyncStage};
use crate::profile::{Secret, is_exact_authority_rejection};
use crate::providers::failure_capture;

use super::{
    AcknowledgementOutcome, PendingUsageBatch, ProviderSettingsAcknowledgement,
    UsageSyncAcknowledgements, parse_provider_settings_acknowledgement,
    parse_usage_acknowledgements,
};

const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const DAILY_USAGE_MUTATION: &str = "sync:dailyUsage";
const PROVIDER_SETTINGS_MUTATION: &str = "sync:providerSettings";
const MAX_TOKEN_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_CONVEX_JWT_BYTES: usize = 8 * 1_024;
const MAX_ACKNOWLEDGEMENT_BYTES: usize = 64 * 1_024;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const SYNC_MUTATION_TIMEOUT: Duration = Duration::from_secs(30);

/// The only outcomes that the native coordinator can use.
#[derive(Debug, PartialEq)]
pub(crate) enum UsageSyncTransportOutcome {
    Committed(UsageSyncAcknowledgements),
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
        let jwt = match self.fetch_convex_jwt(session, &args.diagnostics) {
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

    fn fetch_convex_jwt(
        &self,
        session: &Secret,
        diagnostics: &SyncDiagnostics,
    ) -> TokenFetchOutcome {
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
            Err(_) => {
                diagnostics.before_mutation(SyncReason::TransportFailed, None);
                return TokenFetchOutcome::Offline;
            }
        };
        let status = response.status().as_u16();
        match classify_token_http_status(status) {
            TokenHttpOutcome::Success => {}
            TokenHttpOutcome::SessionRejected => {
                // The runtime first tries the normal expired-session refresh.
                return TokenFetchOutcome::SessionRejected;
            }
            TokenHttpOutcome::Deferred => {
                diagnostics.before_mutation(SyncReason::ServerRejected, Some(u64::from(status)));
                return TokenFetchOutcome::Deferred;
            }
        }

        let mut body = Zeroizing::new(Vec::with_capacity(MAX_TOKEN_RESPONSE_BYTES));
        if response
            .take((MAX_TOKEN_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .is_err()
        {
            diagnostics.before_mutation(SyncReason::TransportFailed, Some(u64::from(status)));
            return TokenFetchOutcome::Offline;
        }
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            diagnostics.before_mutation(SyncReason::InvalidResponse, Some(u64::from(status)));
            return TokenFetchOutcome::Deferred;
        }
        let response: ConvexTokenResponse = match serde_json::from_slice(body.as_slice()) {
            Ok(response) => response,
            Err(_) => {
                diagnostics.before_mutation(SyncReason::InvalidResponse, Some(u64::from(status)));
                return TokenFetchOutcome::Deferred;
            }
        };
        if response.token.is_empty() || response.token.len() > MAX_CONVEX_JWT_BYTES {
            diagnostics.before_mutation(SyncReason::InvalidResponse, Some(u64::from(status)));
            return TokenFetchOutcome::Deferred;
        }
        TokenFetchOutcome::Jwt(response.token)
    }
}

/// Report only after the runtime cannot recover a rejected current session.
pub(super) fn capture_terminal_session_failure(batch: &PendingUsageBatch) {
    SyncDiagnostics::from_batch(batch).before_mutation(SyncReason::ServerRejected, None);
}

fn send_with_runtime(
    convex_url: &str,
    jwt: Zeroizing<String>,
    args: BatchMutationArguments,
) -> UsageSyncTransportOutcome {
    let diagnostics = args.diagnostics.clone();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            diagnostics.before_mutation(SyncReason::TransportFailed, None);
            return UsageSyncTransportOutcome::Deferred;
        }
    };
    runtime.block_on(async move {
        match tokio::time::timeout(SYNC_MUTATION_TIMEOUT, send_mutation(convex_url, jwt, args))
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => {
                diagnostics.before_mutation(SyncReason::TransportFailed, None);
                UsageSyncTransportOutcome::Offline
            }
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
    args: BatchMutationArguments,
) -> UsageSyncTransportOutcome {
    let mut client = match ConvexClient::new(convex_url).await {
        Ok(client) => client,
        Err(_) => {
            args.diagnostics
                .before_mutation(SyncReason::TransportFailed, None);
            return UsageSyncTransportOutcome::Offline;
        }
    };
    client.set_auth(Some(jwt.as_str().to_owned())).await;
    let outcome = send_authenticated_mutations(&mut client, args).await;
    client.set_auth(None).await;
    outcome
}

struct BatchMutationArguments {
    diagnostics: SyncDiagnostics,
    provider_settings: Option<BTreeMap<String, Value>>,
    usage: Option<BTreeMap<String, Value>>,
}

#[derive(Clone, Default)]
struct SyncDiagnostics {
    settings: Option<SyncContext>,
    usage: Vec<(Option<Provider>, SyncContext)>,
    active_stage: std::sync::Arc<std::sync::atomic::AtomicU8>,
    capture_epoch: u64,
}

impl SyncDiagnostics {
    fn from_batch(batch: &PendingUsageBatch) -> Self {
        let context = |stage, pending_count| SyncContext {
            stage,
            reason: SyncReason::TransportFailed,
            status_code: None,
            ranking_day: None,
            attempted_revision: None,
            last_acknowledged_revision: None,
            pending_count: Some(pending_count),
        };
        let settings = batch
            .provider_settings
            .as_ref()
            .map(|settings| SyncContext {
                attempted_revision: Some(settings.revision),
                ..context(SyncStage::ProviderSettings, 1)
            });
        let mut usage: Vec<_> = batch
            .snapshots
            .iter()
            .map(|snapshot| snapshot.provider)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|provider| {
                let mut snapshots = batch
                    .snapshots
                    .iter()
                    .filter(|snapshot| snapshot.provider == provider);
                let first = snapshots.next()?;
                let count = 1 + snapshots.count() as u64;
                let mut context = context(SyncStage::DailyUsage, count);
                if count == 1 {
                    context.ranking_day = Some(first.ranking_day.clone());
                    context.attempted_revision = Some(first.revision);
                }
                Some((Some(provider), context))
            })
            .collect();
        if usage.is_empty() && batch.requires_usage_mutation() {
            usage.push((None, context(SyncStage::DailyUsage, 0)));
        }
        let active_stage = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(u8::from(
            settings.is_none(),
        )));
        Self {
            settings,
            usage,
            active_stage,
            capture_epoch: failure_capture::capture_epoch(),
        }
    }

    fn before_mutation(&self, reason: SyncReason, status: Option<u64>) {
        self.capture(
            if self.active_stage.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                SyncStage::ProviderSettings
            } else {
                SyncStage::DailyUsage
            },
            reason,
            status,
        );
    }

    fn capture(&self, stage: SyncStage, reason: SyncReason, status: Option<u64>) {
        let emit = |provider, context: &SyncContext| {
            diagnostics::capture_at_epoch(
                Failure::Sync {
                    code: SyncCode::SyncRequestFailed,
                    provider,
                    context: SyncContext {
                        reason,
                        status_code: status,
                        ..context.clone()
                    },
                },
                self.capture_epoch,
            )
        };
        match stage {
            SyncStage::ProviderSettings => {
                if let Some(context) = &self.settings {
                    emit(None, context);
                }
            }
            SyncStage::DailyUsage => {
                for (provider, context) in &self.usage {
                    emit(*provider, context);
                }
            }
        }
    }
}

fn mutation_failure_reason(result: &FunctionResult) -> SyncReason {
    match result {
        FunctionResult::Value(_) => SyncReason::InvalidResponse,
        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
            SyncReason::ServerRejected
        }
    }
}

async fn send_authenticated_mutations(
    client: &mut ConvexClient,
    args: BatchMutationArguments,
) -> UsageSyncTransportOutcome {
    let provider_settings = if let Some(settings_args) = args.provider_settings {
        let result = match client
            .mutation(PROVIDER_SETTINGS_MUTATION, settings_args)
            .await
        {
            Ok(result) => result,
            Err(_) => {
                args.diagnostics.capture(
                    SyncStage::ProviderSettings,
                    SyncReason::TransportFailed,
                    None,
                );
                return UsageSyncTransportOutcome::Offline;
            }
        };
        let failure_reason = mutation_failure_reason(&result);
        match classify_provider_settings_result(result) {
            ParsedMutation::Value(acknowledgement) => Some(acknowledgement),
            ParsedMutation::AuthorityRejected => {
                return UsageSyncTransportOutcome::AuthorityRejected;
            }
            ParsedMutation::Deferred => {
                args.diagnostics
                    .capture(SyncStage::ProviderSettings, failure_reason, None);
                return UsageSyncTransportOutcome::Deferred;
            }
        }
    } else {
        None
    };

    if provider_settings
        .as_ref()
        .is_some_and(|acknowledgement| acknowledgement.outcome == AcknowledgementOutcome::Stale)
    {
        return UsageSyncTransportOutcome::Committed(UsageSyncAcknowledgements {
            provider_settings,
            usage: Vec::new(),
            usage_mutation_completed: false,
        });
    }

    let (usage, usage_mutation_completed) = if let Some(usage_args) = args.usage {
        args.diagnostics
            .active_stage
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let result = match client.mutation(DAILY_USAGE_MUTATION, usage_args).await {
            Ok(result) => result,
            Err(_) => {
                args.diagnostics
                    .capture(SyncStage::DailyUsage, SyncReason::TransportFailed, None);
                return UsageSyncTransportOutcome::Offline;
            }
        };
        let failure_reason = mutation_failure_reason(&result);
        match classify_usage_result(result) {
            ParsedMutation::Value(acknowledgements) => (acknowledgements, true),
            ParsedMutation::AuthorityRejected => {
                return UsageSyncTransportOutcome::AuthorityRejected;
            }
            ParsedMutation::Deferred => {
                args.diagnostics
                    .capture(SyncStage::DailyUsage, failure_reason, None);
                return UsageSyncTransportOutcome::Deferred;
            }
        }
    } else {
        (Vec::new(), false)
    };

    UsageSyncTransportOutcome::Committed(UsageSyncAcknowledgements {
        provider_settings,
        usage,
        usage_mutation_completed,
    })
}

fn mutation_arguments(
    batch: &PendingUsageBatch,
    installation_credential: &str,
    now: OffsetDateTime,
) -> Result<BatchMutationArguments, ()> {
    let provider_settings = batch
        .provider_settings_mutation_args(installation_credential)
        .map_err(|_| ())?
        .map(|allowed| {
            let serialized = serde_json::to_value(allowed).map_err(|_| ())?;
            let Value::Object(args) = request_json_to_convex(serialized)? else {
                return Err(());
            };
            Ok(args)
        })
        .transpose()?;
    let usage = if batch.requires_usage_mutation() {
        let allowed = batch
            .mutation_args(installation_credential, now)
            .map_err(|_| ())?;
        let serialized = serde_json::to_value(allowed).map_err(|_| ())?;
        let Value::Object(args) = request_json_to_convex(serialized)? else {
            return Err(());
        };
        Some(args)
    } else {
        None
    };
    if provider_settings.is_none() && usage.is_none() {
        return Err(());
    }
    Ok(BatchMutationArguments {
        diagnostics: SyncDiagnostics::from_batch(batch),
        provider_settings,
        usage,
    })
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

enum ParsedMutation<T> {
    Value(T),
    AuthorityRejected,
    Deferred,
}

fn classify_usage_result(
    result: FunctionResult,
) -> ParsedMutation<Vec<super::UsageSyncAcknowledgement>> {
    match result {
        FunctionResult::Value(value) => parse_success_value(value, parse_usage_acknowledgements)
            .map(ParsedMutation::Value)
            .unwrap_or(ParsedMutation::Deferred),
        FunctionResult::ConvexError(error) if is_exact_authority_rejection(&error.data) => {
            ParsedMutation::AuthorityRejected
        }
        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
            ParsedMutation::Deferred
        }
    }
}

fn classify_provider_settings_result(
    result: FunctionResult,
) -> ParsedMutation<ProviderSettingsAcknowledgement> {
    match result {
        FunctionResult::Value(value) => {
            parse_success_value(value, parse_provider_settings_acknowledgement)
                .map(ParsedMutation::Value)
                .unwrap_or(ParsedMutation::Deferred)
        }
        FunctionResult::ConvexError(error) if is_exact_authority_rejection(&error.data) => {
            ParsedMutation::AuthorityRejected
        }
        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
            ParsedMutation::Deferred
        }
    }
}

fn parse_success_value<T>(
    value: Value,
    parse: impl FnOnce(&[u8]) -> Result<T, super::UsageSyncError>,
) -> Result<T, ()> {
    let mut budget = MAX_ACKNOWLEDGEMENT_BYTES;
    let json = response_convex_to_json(value, &mut budget)?;
    let encoded = serde_json::to_vec(&json).map_err(|_| ())?;
    if encoded.len() > MAX_ACKNOWLEDGEMENT_BYTES {
        return Err(());
    }
    parse(&encoded).map_err(|_| ())
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
    use std::collections::{BTreeMap, BTreeSet};

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
            queue_provider_settings,
        },
    };

    const NOW: &str = "2026-08-08T12:34:56Z";
    const DAY_START_MILLIS: u64 = 1_786_147_200_000;
    const INSTALLATION_CREDENTIAL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse(NOW, &Rfc3339).unwrap()
    }

    #[test]
    fn token_response_failure_captures_http_status_without_body_or_credentials() {
        crate::install_tls_crypto_provider();
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nPRIVATE-BODY").unwrap();
        });
        let transport = HttpUsageSyncTransport {
            auth_site_url: Some(Box::leak(format!("http://{address}").into_boxed_str())),
            convex_url: None,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
        };
        let diagnostics = SyncDiagnostics::from_batch(&batch());
        let failures = crate::diagnostics::collect_failures_for_test(|| {
            assert!(matches!(
                transport
                    .fetch_convex_jwt(&Secret::new("PRIVATE-SESSION".to_owned()), &diagnostics),
                TokenFetchOutcome::Deferred
            ));
        });
        server.join().unwrap();
        assert_eq!(failures.len(), 2);
        for failure in &failures {
            let Failure::Sync {
                context, provider, ..
            } = failure
            else {
                panic!("sync failure");
            };
            assert!(provider.is_some());
            assert_eq!(context.stage, SyncStage::DailyUsage);
            assert_eq!(context.reason, SyncReason::InvalidResponse);
            assert_eq!(context.status_code, Some(200));
        }
        assert!(
            !serde_json::to_string(&failures)
                .unwrap()
                .contains("PRIVATE")
        );
    }

    #[test]
    fn expired_session_http_status_is_quiet_before_runtime_refresh() {
        crate::install_tls_crypto_provider();
        use std::{io::Write, net::TcpListener};
        for status in [401, 403] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 {status} Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let transport = HttpUsageSyncTransport {
                auth_site_url: Some(Box::leak(format!("http://{address}").into_boxed_str())),
                convex_url: None,
                client: reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap(),
            };
            let failures = crate::diagnostics::collect_failures_for_test(|| {
                assert!(matches!(
                    transport.fetch_convex_jwt(
                        &Secret::test_only(),
                        &SyncDiagnostics::from_batch(&batch())
                    ),
                    TokenFetchOutcome::SessionRejected
                ));
            });
            server.join().unwrap();
            assert!(failures.is_empty());
        }
    }

    #[test]
    fn empty_backfill_failure_has_no_invented_provider_or_revision() {
        let mut pending = batch();
        pending.snapshots.clear();
        pending.provider_settings = None;
        pending.profile_backfill_anchor = Some("2026-08-08".to_owned());
        assert!(pending.requires_usage_mutation());
        let context = SyncDiagnostics::from_batch(&pending);
        let failures = crate::diagnostics::collect_failures_for_test(|| {
            context.before_mutation(SyncReason::TransportFailed, None);
        });
        let [
            Failure::Sync {
                provider, context, ..
            },
        ] = failures.as_slice()
        else {
            panic!("one batch failure");
        };
        assert_eq!(*provider, None);
        assert_eq!(context.stage, SyncStage::DailyUsage);
        assert_eq!(context.pending_count, Some(0));
        assert_eq!(context.ranking_day, None);
        assert_eq!(context.attempted_revision, None);
        assert_eq!(context.last_acknowledged_revision, None);
    }

    #[test]
    fn mutation_timeout_is_created_inside_the_owned_runtime() {
        let outcome = send_with_runtime(
            "not-a-convex-url",
            Zeroizing::new("test-jwt".to_owned()),
            BatchMutationArguments {
                diagnostics: SyncDiagnostics::default(),
                provider_settings: None,
                usage: Some(BTreeMap::new()),
            },
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

    fn provider_settings_batch() -> PendingUsageBatch {
        let mut connection = Connection::open_in_memory().unwrap();
        install_usage_sync_schema(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        queue_provider_settings(&transaction, 4, &BTreeSet::from([CodingProvider::Codex])).unwrap();
        transaction.commit().unwrap();
        load_pending_usage_batch(&connection, 4).unwrap().unwrap()
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
        assert!(args.provider_settings.is_none());
        let value = JsonValue::from(Value::Object(args.usage.unwrap()));

        assert_eq!(
            value,
            json!({
                "activeMacGeneration": 4.0,
                "installationCredential": INSTALLATION_CREDENTIAL,
                "profileBackfillAnchor": null,
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
    fn converts_the_exact_provider_setting_request_to_convex_values() {
        let args =
            mutation_arguments(&provider_settings_batch(), INSTALLATION_CREDENTIAL, now()).unwrap();
        assert!(args.usage.is_none());
        let value = JsonValue::from(Value::Object(args.provider_settings.unwrap()));

        assert_eq!(
            value,
            json!({
                "activeMacGeneration": 4.0,
                "enabledProviders": ["codex"],
                "installationCredential": INSTALLATION_CREDENTIAL,
                "revision": 1.0
            })
        );
    }

    #[test]
    fn converts_the_exact_correction_pair_to_convex_values() {
        let args = mutation_arguments(&corrected_batch(), INSTALLATION_CREDENTIAL, now()).unwrap();
        assert!(args.provider_settings.is_none());
        let value = JsonValue::from(Value::Object(args.usage.unwrap()));

        assert_eq!(
            value,
            json!({
                "activeMacGeneration": 4.0,
                "installationCredential": INSTALLATION_CREDENTIAL,
                "profileBackfillAnchor": null,
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
            classify_usage_result(exact),
            ParsedMutation::AuthorityRejected
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
            classify_usage_result(with_extra_data),
            ParsedMutation::Deferred
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
            classify_usage_result(FunctionResult::Value(malformed)),
            ParsedMutation::Deferred
        ));
        assert!(matches!(
            classify_usage_result(FunctionResult::Value(Value::Int64(1))),
            ParsedMutation::Deferred
        ));
    }

    #[test]
    fn provider_setting_acknowledgement_is_exact() {
        let valid = Value::Object(BTreeMap::from([
            ("outcome".to_owned(), Value::String("committed".to_owned())),
            ("revision".to_owned(), Value::Float64(2.0)),
        ]));
        assert!(matches!(
            classify_provider_settings_result(FunctionResult::Value(valid)),
            ParsedMutation::Value(ProviderSettingsAcknowledgement {
                revision: 2,
                outcome: AcknowledgementOutcome::Committed
            })
        ));
        let hostile = Value::Object(BTreeMap::from([
            ("outcome".to_owned(), Value::String("committed".to_owned())),
            ("revision".to_owned(), Value::Float64(2.0)),
            (
                "installationId".to_owned(),
                Value::String("private".to_owned()),
            ),
        ]));
        assert!(matches!(
            classify_provider_settings_result(FunctionResult::Value(hostile)),
            ParsedMutation::Deferred
        ));
    }

    #[test]
    fn valid_acknowledgements_commit_without_private_error_detail() {
        let outcome = classify_usage_result(FunctionResult::Value(acknowledgement_value()));
        assert!(matches!(
            outcome,
            ParsedMutation::Value(ref values) if values.len() == 1
        ));

        let private_detail = "session-private-value";
        assert!(matches!(
            classify_usage_result(FunctionResult::ErrorMessage(private_detail.to_owned())),
            ParsedMutation::Deferred
        ));
    }
}
