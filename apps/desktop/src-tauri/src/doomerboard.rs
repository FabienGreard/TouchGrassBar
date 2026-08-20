#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

use convex::{ConvexClient, FunctionResult, Value};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::profile::{
    ProfileCoordinator, Secret, is_exact_authority_rejection, valid_touch_grass_id,
};
use crate::updater::OnlineFeatureGate;

pub const DOOMERBOARD_CONTRACT_VERSION: u8 = 1;
const CURRENT_GLOBAL_QUERY: &str = "doomerboards:currentGlobal";
const CURRENT_MY_TOKENMAXXERS_QUERY: &str = "doomerboards:currentMyTokenmaxxers";
const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const MAX_ROWS: usize = 100;
const MAX_TOKEN_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_JWT_BYTES: usize = 8 * 1_024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DoomerboardAudienceV1 {
    Global,
    Mine,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DoomerboardScopeV1 {
    Claude,
    Codex,
    Combined,
}

impl DoomerboardScopeV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Combined => "combined",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DoomerboardQueryV1 {
    audience: DoomerboardAudienceV1,
    scope: DoomerboardScopeV1,
    window_days: u8,
}

impl DoomerboardQueryV1 {
    fn is_valid(self) -> bool {
        matches!(self.window_days, 1 | 7 | 30)
    }
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoomerboardRowV1 {
    #[schemars(length(min = 1, max = 40))]
    pub display_name: String,
    #[schemars(regex(pattern = r"^TG-[23456789ABCDEFGHJKLMNPQRSTUVWXYZ]{6}$"))]
    pub touch_grass_id: String,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub rank: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub token_score: u64,
    pub api_equivalent_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    tag = "status"
)]
pub enum DoomerboardViewV1 {
    Ready {
        contract_version: u8,
        #[schemars(length(max = 100))]
        rows: Vec<DoomerboardRowV1>,
    },
    Unavailable {
        contract_version: u8,
    },
}

impl DoomerboardViewV1 {
    fn ready(rows: Vec<DoomerboardRowV1>) -> Self {
        Self::Ready {
            contract_version: DOOMERBOARD_CONTRACT_VERSION,
            rows,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self::Unavailable {
            contract_version: DOOMERBOARD_CONTRACT_VERSION,
        }
    }
}

pub fn doomerboard_view_schema() -> Schema {
    schema_for!(DoomerboardViewV1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportError {
    AuthorityRejected,
    Unavailable,
}

trait DoomerboardTransport: Send + Sync {
    fn read(
        &self,
        session: &Secret,
        query: DoomerboardQueryV1,
    ) -> Result<Vec<DoomerboardRowV1>, TransportError>;
}

#[derive(Clone)]
struct HttpDoomerboardTransport {
    auth_site_url: Option<&'static str>,
    convex_url: Option<&'static str>,
    client: reqwest::blocking::Client,
}

impl HttpDoomerboardTransport {
    #[cfg(target_os = "macos")]
    fn from_build_configuration() -> Self {
        Self {
            auth_site_url: option_env!("CONVEX_SITE_URL").filter(|value| !value.is_empty()),
            convex_url: option_env!("CONVEX_URL").filter(|value| !value.is_empty()),
            client: crate::native_https_client(),
        }
    }

    fn endpoint(&self, path: &str) -> Result<String, TransportError> {
        let base = self.auth_site_url.ok_or(TransportError::Unavailable)?;
        Ok(format!("{}{path}", base.trim_end_matches('/')))
    }

    fn fetch_convex_token(&self, session: &Secret) -> Result<Zeroizing<String>, TransportError> {
        let response = self
            .client
            .get(self.endpoint(CONVEX_TOKEN_PATH)?)
            .bearer_auth(session.expose())
            .send()
            .map_err(|_| TransportError::Unavailable)?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(TransportError::AuthorityRejected);
        }
        let response = response
            .error_for_status()
            .map_err(|_| TransportError::Unavailable)?;
        let mut body = Zeroizing::new(Vec::with_capacity(MAX_TOKEN_RESPONSE_BYTES));
        response
            .take((MAX_TOKEN_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|_| TransportError::Unavailable)?;
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            return Err(TransportError::Unavailable);
        }
        let response: ConvexTokenResponse =
            serde_json::from_slice(body.as_slice()).map_err(|_| TransportError::Unavailable)?;
        if response.token.is_empty() || response.token.len() > MAX_JWT_BYTES {
            return Err(TransportError::Unavailable);
        }
        Ok(Zeroizing::new(response.token))
    }
}

impl DoomerboardTransport for HttpDoomerboardTransport {
    fn read(
        &self,
        session: &Secret,
        query: DoomerboardQueryV1,
    ) -> Result<Vec<DoomerboardRowV1>, TransportError> {
        let convex_url = self
            .convex_url
            .ok_or(TransportError::Unavailable)?
            .to_owned();
        let jwt = self.fetch_convex_token(session)?;
        let result = tokio::runtime::Runtime::new()
            .map_err(|_| TransportError::Unavailable)?
            .block_on(async move {
                tokio::time::timeout(QUERY_TIMEOUT, async move {
                    let mut client = ConvexClient::new(&convex_url)
                        .await
                        .map_err(|_| TransportError::Unavailable)?;
                    client.set_auth(Some(jwt.as_str().to_owned())).await;
                    let result = client
                        .query(
                            doomerboard_query_name(query),
                            doomerboard_query_arguments(query, OffsetDateTime::now_utc()),
                        )
                        .await;
                    client.set_auth(None).await;
                    result.map_err(|_| TransportError::Unavailable)
                })
                .await
                .map_err(|_| TransportError::Unavailable)?
            })?;
        match result {
            FunctionResult::Value(value) => parse_selected_rows(query, value),
            FunctionResult::ConvexError(error) if is_exact_authority_rejection(&error.data) => {
                Err(TransportError::AuthorityRejected)
            }
            FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
                Err(TransportError::Unavailable)
            }
        }
    }
}

fn doomerboard_query_name(query: DoomerboardQueryV1) -> &'static str {
    match query.audience {
        DoomerboardAudienceV1::Global => CURRENT_GLOBAL_QUERY,
        DoomerboardAudienceV1::Mine => CURRENT_MY_TOKENMAXXERS_QUERY,
    }
}

fn doomerboard_query_arguments(
    query: DoomerboardQueryV1,
    now: OffsetDateTime,
) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "rankingDay".to_owned(),
            Value::String(now.date().to_string()),
        ),
        (
            "scope".to_owned(),
            Value::String(query.scope.as_str().to_owned()),
        ),
        (
            "windowDays".to_owned(),
            Value::Float64(f64::from(query.window_days)),
        ),
    ])
}

#[derive(Deserialize)]
struct ConvexTokenResponse {
    token: String,
}

#[derive(Clone)]
pub(crate) struct DoomerboardRuntime {
    coordinator: Arc<Mutex<ProfileCoordinator>>,
    online_gate: OnlineFeatureGate,
    transport: Arc<dyn DoomerboardTransport>,
}

impl DoomerboardRuntime {
    fn new(
        coordinator: Arc<Mutex<ProfileCoordinator>>,
        transport: Arc<dyn DoomerboardTransport>,
        online_gate: OnlineFeatureGate,
    ) -> Self {
        Self {
            coordinator,
            online_gate,
            transport,
        }
    }

    pub(crate) fn read(&self, query: DoomerboardQueryV1) -> DoomerboardViewV1 {
        if self.online_gate.is_paused() || !query.is_valid() {
            return DoomerboardViewV1::unavailable();
        }
        let session = match self
            .coordinator
            .lock()
            .ok()
            .and_then(|coordinator| coordinator.active_sync_credentials().ok())
            .flatten()
        {
            Some(credentials) => credentials.session,
            None => return DoomerboardViewV1::unavailable(),
        };
        match self.transport.read(&session, query) {
            Ok(rows) => DoomerboardViewV1::ready(rows),
            Err(TransportError::AuthorityRejected) => {
                let refreshed = self
                    .coordinator
                    .lock()
                    .ok()
                    .and_then(|coordinator| coordinator.refresh_active_sync_session(&session).ok())
                    .flatten();
                match refreshed
                    .as_ref()
                    .and_then(|fresh| self.transport.read(fresh, query).ok())
                {
                    Some(rows) => DoomerboardViewV1::ready(rows),
                    None => DoomerboardViewV1::unavailable(),
                }
            }
            Err(TransportError::Unavailable) => DoomerboardViewV1::unavailable(),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn production_runtime(
    coordinator: Arc<Mutex<ProfileCoordinator>>,
    online_gate: OnlineFeatureGate,
) -> DoomerboardRuntime {
    DoomerboardRuntime::new(
        coordinator,
        Arc::new(HttpDoomerboardTransport::from_build_configuration()),
        online_gate,
    )
}

fn nonnegative_safe_integer(value: &Value) -> Option<u64> {
    match value {
        Value::Int64(value) => u64::try_from(*value)
            .ok()
            .filter(|value| *value <= MAX_SAFE_INTEGER),
        Value::Float64(value)
            if value.is_finite()
                && value.fract() == 0.0
                && (0.0..=MAX_SAFE_INTEGER as f64).contains(value) =>
        {
            Some(*value as u64)
        }
        _ => None,
    }
}

fn exact_keys<const N: usize>(object: &BTreeMap<String, Value>, expected: [&str; N]) -> bool {
    let expected = expected.into_iter().collect::<BTreeSet<_>>();
    object.len() == expected.len() && object.keys().all(|key| expected.contains(key.as_str()))
}

fn parse_cost(value: &Value) -> Option<Option<f64>> {
    if matches!(value, Value::Null) {
        return Some(None);
    }
    let Value::Object(object) = value else {
        return None;
    };
    if !exact_keys(
        object,
        ["coveragePercent", "micros", "pricingBasis", "quality"],
    ) {
        return None;
    }
    let micros = object.get("micros").and_then(nonnegative_safe_integer)?;
    Some(Some(micros as f64 / 1_000_000.0))
}

fn parse_row(value: Value) -> Option<DoomerboardRowV1> {
    let Value::Object(object) = value else {
        return None;
    };
    if !exact_keys(
        &object,
        [
            "apiEquivalentCost",
            "displayName",
            "rank",
            "tokenScore",
            "touchGrassId",
        ],
    ) {
        return None;
    }
    let display_name = match object.get("displayName") {
        Some(Value::String(value)) if (1..=40).contains(&value.chars().count()) => value.clone(),
        _ => return None,
    };
    let touch_grass_id = match object.get("touchGrassId") {
        Some(Value::String(value)) if valid_touch_grass_id(value) => value.clone(),
        _ => return None,
    };
    let rank = object.get("rank").and_then(nonnegative_safe_integer)?;
    if rank == 0 {
        return None;
    }
    let token_score = object
        .get("tokenScore")
        .and_then(nonnegative_safe_integer)?;
    let api_equivalent_cost_usd = parse_cost(object.get("apiEquivalentCost")?)?;
    Some(DoomerboardRowV1 {
        display_name,
        touch_grass_id,
        rank,
        token_score,
        api_equivalent_cost_usd,
    })
}

fn valid_order(rows: &[DoomerboardRowV1]) -> bool {
    let mut identities = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        if !identities.insert(row.touch_grass_id.as_str()) {
            return false;
        }
        if index == 0 {
            if row.rank != 1 {
                return false;
            }
            continue;
        }
        let previous = &rows[index - 1];
        if row.token_score > previous.token_score
            || (row.token_score == previous.token_score
                && row.touch_grass_id <= previous.touch_grass_id)
        {
            return false;
        }
        let expected_rank = if row.token_score == previous.token_score {
            previous.rank
        } else {
            index as u64 + 1
        };
        if row.rank != expected_rank {
            return false;
        }
    }
    true
}

fn parse_rows(value: Value) -> Result<Vec<DoomerboardRowV1>, TransportError> {
    let Value::Array(values) = value else {
        return Err(TransportError::Unavailable);
    };
    if values.len() > MAX_ROWS {
        return Err(TransportError::Unavailable);
    }
    let rows = values
        .into_iter()
        .map(parse_row)
        .collect::<Option<Vec<_>>>()
        .ok_or(TransportError::Unavailable)?;
    valid_order(&rows)
        .then_some(rows)
        .ok_or(TransportError::Unavailable)
}

fn parse_selected_rows(
    query: DoomerboardQueryV1,
    value: Value,
) -> Result<Vec<DoomerboardRowV1>, TransportError> {
    if query.audience == DoomerboardAudienceV1::Global {
        return parse_rows(value);
    }
    let Value::Object(mut object) = value else {
        return Err(TransportError::Unavailable);
    };
    if !exact_keys(&object, ["rows", "savedTokenmaxxerCount"]) {
        return Err(TransportError::Unavailable);
    }
    let saved_tokenmaxxer_count = object
        .remove("savedTokenmaxxerCount")
        .as_ref()
        .and_then(nonnegative_safe_integer)
        .filter(|count| *count <= MAX_ROWS as u64)
        .ok_or(TransportError::Unavailable)?;
    let rows = parse_rows(object.remove("rows").ok_or(TransportError::Unavailable)?)?;
    if rows.len() as u64 == saved_tokenmaxxer_count {
        Ok(rows)
    } else {
        Err(TransportError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(target_os = "macos")]
    #[derive(Default)]
    struct CountingTransport {
        calls: AtomicUsize,
    }

    #[cfg(target_os = "macos")]
    impl DoomerboardTransport for CountingTransport {
        fn read(
            &self,
            _session: &Secret,
            _query: DoomerboardQueryV1,
        ) -> Result<Vec<DoomerboardRowV1>, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        }
    }

    fn cost(micros: i64) -> Value {
        Value::Object(BTreeMap::from([
            ("coveragePercent".to_owned(), Value::Null),
            ("micros".to_owned(), Value::Int64(micros)),
            (
                "pricingBasis".to_owned(),
                Value::String("approved-basis".to_owned()),
            ),
            ("quality".to_owned(), Value::String("reconciled".to_owned())),
        ]))
    }

    fn row(public_id: &str, rank: i64, score: i64) -> Value {
        Value::Object(BTreeMap::from([
            ("apiEquivalentCost".to_owned(), cost(12_500_000)),
            (
                "displayName".to_owned(),
                Value::String(format!("Tokenmaxxer {public_id}")),
            ),
            ("rank".to_owned(), Value::Int64(rank)),
            ("tokenScore".to_owned(), Value::Int64(score)),
            (
                "touchGrassId".to_owned(),
                Value::String(public_id.to_owned()),
            ),
        ]))
    }

    fn query(
        audience: DoomerboardAudienceV1,
        scope: DoomerboardScopeV1,
        window_days: u8,
    ) -> DoomerboardQueryV1 {
        DoomerboardQueryV1 {
            audience,
            scope,
            window_days,
        }
    }

    fn saved_rows(saved_tokenmaxxer_count: i64, rows: Vec<Value>) -> Value {
        Value::Object(BTreeMap::from([
            (
                "savedTokenmaxxerCount".to_owned(),
                Value::Int64(saved_tokenmaxxer_count),
            ),
            ("rows".to_owned(), Value::Array(rows)),
        ]))
    }

    #[test]
    fn accepts_only_ordered_bounded_public_rows() {
        let parsed = parse_rows(Value::Array(vec![
            row("TG-234567", 1, 500),
            row("TG-234568", 1, 500),
            row("TG-234569", 3, 400),
        ]))
        .expect("parse public board");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].api_equivalent_cost_usd, Some(12.5));
        assert_eq!(parsed[1].rank, 1);
        assert_eq!(parsed[2].rank, 3);
    }

    #[test]
    fn selected_query_uses_the_native_utc_day_provider_and_period() {
        let now =
            OffsetDateTime::from_unix_timestamp(1_775_819_696).expect("valid fixture timestamp");
        let selected = query(DoomerboardAudienceV1::Mine, DoomerboardScopeV1::Claude, 30);
        assert_eq!(
            doomerboard_query_name(selected),
            CURRENT_MY_TOKENMAXXERS_QUERY
        );
        assert_eq!(
            doomerboard_query_arguments(selected, now),
            BTreeMap::from([
                (
                    "rankingDay".to_owned(),
                    Value::String("2026-04-10".to_owned()),
                ),
                ("scope".to_owned(), Value::String("claude".to_owned()),),
                ("windowDays".to_owned(), Value::Float64(30.0)),
            ]),
        );
    }

    #[test]
    fn saved_profiles_without_current_scores_are_unavailable_not_empty() {
        let selected = query(DoomerboardAudienceV1::Mine, DoomerboardScopeV1::Combined, 1);
        assert_eq!(
            parse_selected_rows(selected, saved_rows(0, Vec::new())),
            Ok(Vec::new())
        );
        assert_eq!(
            parse_selected_rows(selected, saved_rows(1, Vec::new())),
            Err(TransportError::Unavailable)
        );
        assert_eq!(
            parse_selected_rows(selected, saved_rows(1, vec![row("TG-234567", 1, 500)]),)
                .expect("parse saved board")
                .len(),
            1
        );
        assert_eq!(
            parse_selected_rows(selected, saved_rows(2, vec![row("TG-234567", 1, 500)]),),
            Err(TransportError::Unavailable)
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn paused_online_gate_blocks_profile_and_transport_work() {
        let transport = Arc::new(CountingTransport::default());
        let runtime = DoomerboardRuntime::new(
            Arc::new(Mutex::new(crate::profile::production_coordinator(
                crate::lifecycle::DesktopLifecycle::unavailable(),
            ))),
            transport.clone(),
            OnlineFeatureGate::paused(),
        );

        assert_eq!(
            runtime.read(query(
                DoomerboardAudienceV1::Global,
                DoomerboardScopeV1::Combined,
                1,
            )),
            DoomerboardViewV1::unavailable()
        );
        assert_eq!(transport.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rejects_private_fields_malformed_ranks_and_oversized_rows() {
        let mut private_row = match row("TG-234567", 1, 500) {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        private_row.insert(
            "providerMessageId".to_owned(),
            Value::String("private".to_owned()),
        );
        assert_eq!(
            parse_rows(Value::Array(vec![Value::Object(private_row)])),
            Err(TransportError::Unavailable)
        );
        assert_eq!(
            parse_rows(Value::Array(vec![
                row("TG-234568", 1, 500),
                row("TG-234567", 1, 500),
            ])),
            Err(TransportError::Unavailable)
        );
        assert_eq!(
            parse_rows(Value::Array(
                (0..=MAX_ROWS).map(|_| row("TG-234567", 1, 500)).collect(),
            )),
            Err(TransportError::Unavailable)
        );
    }

    #[test]
    fn rejects_private_cost_material() {
        let mut hostile = match row("TG-234567", 1, 500) {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        let mut hostile_cost = match hostile.remove("apiEquivalentCost") {
            Some(Value::Object(object)) => object,
            _ => unreachable!(),
        };
        hostile_cost.insert(
            "privatePath".to_owned(),
            Value::String("/private".to_owned()),
        );
        hostile.insert("apiEquivalentCost".to_owned(), Value::Object(hostile_cost));
        assert_eq!(
            parse_rows(Value::Array(vec![Value::Object(hostile)])),
            Err(TransportError::Unavailable)
        );
    }
}
