#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose};
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
pub const ADD_TOKENMAXXER_CONTRACT_VERSION: u8 = 1;
const CURRENT_GLOBAL_QUERY: &str = "doomerboards:currentGlobal";
const CURRENT_MY_TOKENMAXXERS_QUERY: &str = "doomerboards:currentMyTokenmaxxers";
const ADD_TOKENMAXXER_MUTATION: &str = "tokenmaxxers:addToMyTokenmaxxers";
const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const MAX_ROWS: usize = 100;
const MAX_TOKEN_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_JWT_BYTES: usize = 8 * 1_024;
const JWT_REFRESH_MARGIN_SECONDS: i64 = 60;
const CONVEX_CALL_TIMEOUT: Duration = Duration::from_secs(30);
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

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddTokenmaxxerStatusV1 {
    Added,
    AlreadyAdded,
    Invalid,
    LimitReached,
    NotFound,
    #[serde(rename = "self")]
    SelfProfile,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddTokenmaxxerOutcomeV1 {
    contract_version: u8,
    status: AddTokenmaxxerStatusV1,
}

impl AddTokenmaxxerOutcomeV1 {
    fn new(status: AddTokenmaxxerStatusV1) -> Self {
        Self {
            contract_version: ADD_TOKENMAXXER_CONTRACT_VERSION,
            status,
        }
    }
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
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
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

pub fn add_tokenmaxxer_outcome_schema() -> Schema {
    schema_for!(AddTokenmaxxerOutcomeV1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportError {
    AuthorityRejected,
    Unavailable,
}

enum ConvexCall {
    Mutation {
        arguments: BTreeMap<String, Value>,
        function_name: &'static str,
    },
    Query {
        arguments: BTreeMap<String, Value>,
        function_name: &'static str,
    },
}

trait DoomerboardTransport: Send + Sync {
    fn add(
        &self,
        session: &Secret,
        touch_grass_id: &str,
    ) -> Result<AddTokenmaxxerStatusV1, TransportError>;

    fn read(
        &self,
        session: &Secret,
        query: DoomerboardQueryV1,
    ) -> Result<Vec<DoomerboardRowV1>, TransportError>;
}

struct FetchedConvexToken {
    refresh_after_unix_seconds: i64,
    token: Zeroizing<String>,
}

trait ConvexTokenProvider: Send + Sync {
    fn fetch(
        &self,
        session: &Secret,
        now_unix_seconds: i64,
    ) -> Result<FetchedConvexToken, TransportError>;
}

trait DoomerboardConnection: Send + Sync {
    fn authenticate(&self, token: Zeroizing<String>) -> Result<(), TransportError>;
    fn call(&self, call: ConvexCall) -> Result<FunctionResult, TransportError>;
}

trait DoomerboardConnectionFactory: Send + Sync {
    fn connect(&self, convex_url: &str) -> Result<Arc<dyn DoomerboardConnection>, TransportError>;
}

#[derive(Clone)]
struct HttpConvexTokenProvider {
    auth_site_url: Option<&'static str>,
    client: reqwest::blocking::Client,
}

impl HttpConvexTokenProvider {
    fn endpoint(&self, path: &str) -> Result<String, TransportError> {
        let base = self.auth_site_url.ok_or(TransportError::Unavailable)?;
        Ok(format!("{}{path}", base.trim_end_matches('/')))
    }
}

#[derive(Deserialize)]
struct ConvexJwtClaims {
    exp: i64,
}

fn convex_jwt_refresh_after(token: &str, now_unix_seconds: i64) -> i64 {
    let Some(payload) = token.split('.').nth(1) else {
        return now_unix_seconds;
    };
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| general_purpose::URL_SAFE.decode(payload));
    let Ok(decoded) = decoded else {
        return now_unix_seconds;
    };
    serde_json::from_slice::<ConvexJwtClaims>(&decoded)
        .map(|claims| claims.exp.saturating_sub(JWT_REFRESH_MARGIN_SECONDS))
        .unwrap_or(now_unix_seconds)
}

impl ConvexTokenProvider for HttpConvexTokenProvider {
    fn fetch(
        &self,
        session: &Secret,
        now_unix_seconds: i64,
    ) -> Result<FetchedConvexToken, TransportError> {
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
        let token = Zeroizing::new(response.token);
        Ok(FetchedConvexToken {
            refresh_after_unix_seconds: convex_jwt_refresh_after(token.as_str(), now_unix_seconds),
            token,
        })
    }
}

struct ReusableDoomerboardConnection {
    client: Mutex<ConvexClient>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl DoomerboardConnection for ReusableDoomerboardConnection {
    fn authenticate(&self, token: Zeroizing<String>) -> Result<(), TransportError> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| TransportError::Unavailable)?;
        self.runtime
            .block_on(client.set_auth(Some(token.as_str().to_owned())));
        Ok(())
    }

    fn call(&self, call: ConvexCall) -> Result<FunctionResult, TransportError> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| TransportError::Unavailable)?
            .clone();
        self.runtime.block_on(async move {
            tokio::time::timeout(CONVEX_CALL_TIMEOUT, async move {
                let result = match call {
                    ConvexCall::Mutation {
                        arguments,
                        function_name,
                    } => client.mutation(function_name, arguments).await,
                    ConvexCall::Query {
                        arguments,
                        function_name,
                    } => client.query(function_name, arguments).await,
                };
                result.map_err(|_| TransportError::Unavailable)
            })
            .await
            .map_err(|_| TransportError::Unavailable)?
        })
    }
}

struct ReusableDoomerboardConnectionFactory;

impl DoomerboardConnectionFactory for ReusableDoomerboardConnectionFactory {
    fn connect(&self, convex_url: &str) -> Result<Arc<dyn DoomerboardConnection>, TransportError> {
        let runtime =
            Arc::new(tokio::runtime::Runtime::new().map_err(|_| TransportError::Unavailable)?);
        let client = runtime
            .block_on(async {
                tokio::time::timeout(CONVEX_CALL_TIMEOUT, ConvexClient::new(convex_url)).await
            })
            .map_err(|_| TransportError::Unavailable)?
            .map_err(|_| TransportError::Unavailable)?;
        Ok(Arc::new(ReusableDoomerboardConnection {
            client: Mutex::new(client),
            runtime,
        }))
    }
}

struct CachedDoomerboardConnection {
    connection: Arc<dyn DoomerboardConnection>,
    refresh_after_unix_seconds: i64,
    session: Secret,
}

struct HttpDoomerboardTransport {
    connection: RwLock<Option<CachedDoomerboardConnection>>,
    connection_factory: Arc<dyn DoomerboardConnectionFactory>,
    convex_url: Option<&'static str>,
    now_unix_seconds: Arc<dyn Fn() -> i64 + Send + Sync>,
    token_provider: Arc<dyn ConvexTokenProvider>,
}

impl HttpDoomerboardTransport {
    fn new(
        convex_url: Option<&'static str>,
        token_provider: Arc<dyn ConvexTokenProvider>,
        connection_factory: Arc<dyn DoomerboardConnectionFactory>,
        now_unix_seconds: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            connection: RwLock::new(None),
            connection_factory,
            convex_url,
            now_unix_seconds,
            token_provider,
        }
    }

    #[cfg(target_os = "macos")]
    fn from_build_configuration() -> Self {
        Self::new(
            option_env!("CONVEX_URL").filter(|value| !value.is_empty()),
            Arc::new(HttpConvexTokenProvider {
                auth_site_url: option_env!("CONVEX_SITE_URL").filter(|value| !value.is_empty()),
                client: crate::native_https_client(),
            }),
            Arc::new(ReusableDoomerboardConnectionFactory),
            Arc::new(|| OffsetDateTime::now_utc().unix_timestamp()),
        )
    }

    fn can_reuse(
        cached: &CachedDoomerboardConnection,
        session: &Secret,
        now_unix_seconds: i64,
    ) -> bool {
        cached.session.expose() == session.expose()
            && now_unix_seconds < cached.refresh_after_unix_seconds
    }

    fn authenticated_call(
        &self,
        session: &Secret,
        call: ConvexCall,
    ) -> Result<FunctionResult, TransportError> {
        let convex_url = self.convex_url.ok_or(TransportError::Unavailable)?;
        let now_unix_seconds = (self.now_unix_seconds)();
        {
            let cached = self
                .connection
                .read()
                .map_err(|_| TransportError::Unavailable)?;
            if let Some(cached) = cached.as_ref()
                && Self::can_reuse(cached, session, now_unix_seconds)
            {
                return cached.connection.call(call);
            }
        }

        let mut cached = self
            .connection
            .write()
            .map_err(|_| TransportError::Unavailable)?;
        if let Some(current) = cached.as_ref()
            && Self::can_reuse(current, session, now_unix_seconds)
        {
            return current.connection.call(call);
        }

        let fetched = self.token_provider.fetch(session, now_unix_seconds)?;
        let connection = match cached.as_ref() {
            Some(current) => current.connection.clone(),
            None => self.connection_factory.connect(convex_url)?,
        };
        connection.authenticate(fetched.token)?;
        *cached = Some(CachedDoomerboardConnection {
            connection: connection.clone(),
            refresh_after_unix_seconds: fetched.refresh_after_unix_seconds,
            session: session.clone(),
        });
        connection.call(call)
    }
}

fn parse_function_result<T>(
    result: FunctionResult,
    parse_value: impl FnOnce(Value) -> Result<T, TransportError>,
) -> Result<T, TransportError> {
    match result {
        FunctionResult::Value(value) => parse_value(value),
        FunctionResult::ConvexError(error) if is_exact_authority_rejection(&error.data) => {
            Err(TransportError::AuthorityRejected)
        }
        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
            Err(TransportError::Unavailable)
        }
    }
}

impl DoomerboardTransport for HttpDoomerboardTransport {
    fn add(
        &self,
        session: &Secret,
        touch_grass_id: &str,
    ) -> Result<AddTokenmaxxerStatusV1, TransportError> {
        let result = self.authenticated_call(
            session,
            ConvexCall::Mutation {
                arguments: add_tokenmaxxer_arguments(touch_grass_id),
                function_name: ADD_TOKENMAXXER_MUTATION,
            },
        )?;
        parse_function_result(result, |value| {
            parse_add_tokenmaxxer_status(value).ok_or(TransportError::Unavailable)
        })
    }

    fn read(
        &self,
        session: &Secret,
        query: DoomerboardQueryV1,
    ) -> Result<Vec<DoomerboardRowV1>, TransportError> {
        let result = self.authenticated_call(
            session,
            ConvexCall::Query {
                arguments: doomerboard_query_arguments(query, OffsetDateTime::now_utc()),
                function_name: doomerboard_query_name(query),
            },
        )?;
        parse_function_result(result, |value| parse_selected_rows(query, value))
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

fn add_tokenmaxxer_arguments(touch_grass_id: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "touchGrassId".to_owned(),
        Value::String(touch_grass_id.to_owned()),
    )])
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

    fn with_active_session<T>(
        &self,
        operation: impl Fn(&Secret) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        let session = self
            .coordinator
            .lock()
            .ok()
            .and_then(|coordinator| coordinator.active_sync_credentials().ok())
            .flatten()
            .map(|credentials| credentials.session)
            .ok_or(TransportError::Unavailable)?;
        match operation(&session) {
            Err(TransportError::AuthorityRejected) => {
                let refreshed = self
                    .coordinator
                    .lock()
                    .ok()
                    .and_then(|coordinator| coordinator.refresh_active_sync_session(&session).ok())
                    .flatten()
                    .ok_or(TransportError::Unavailable)?;
                operation(&refreshed)
            }
            result => result,
        }
    }

    pub(crate) fn read(&self, query: DoomerboardQueryV1) -> DoomerboardViewV1 {
        if self.online_gate.is_paused() || !query.is_valid() {
            return DoomerboardViewV1::unavailable();
        }
        match self.with_active_session(|session| self.transport.read(session, query)) {
            Ok(rows) => DoomerboardViewV1::ready(rows),
            Err(TransportError::AuthorityRejected | TransportError::Unavailable) => {
                DoomerboardViewV1::unavailable()
            }
        }
    }

    pub(crate) fn add(&self, touch_grass_id: &str) -> AddTokenmaxxerOutcomeV1 {
        if !valid_touch_grass_id(touch_grass_id) {
            return AddTokenmaxxerOutcomeV1::new(AddTokenmaxxerStatusV1::Invalid);
        }
        if self.online_gate.is_paused() {
            return AddTokenmaxxerOutcomeV1::new(AddTokenmaxxerStatusV1::Unavailable);
        }
        let status =
            match self.with_active_session(|session| self.transport.add(session, touch_grass_id)) {
                Ok(status) => status,
                Err(TransportError::AuthorityRejected | TransportError::Unavailable) => {
                    AddTokenmaxxerStatusV1::Unavailable
                }
            };
        AddTokenmaxxerOutcomeV1::new(status)
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

fn parse_add_tokenmaxxer_status(value: Value) -> Option<AddTokenmaxxerStatusV1> {
    let Value::Object(object) = value else {
        return None;
    };
    if !exact_keys(&object, ["status"]) {
        return None;
    }
    match object.get("status") {
        Some(Value::String(status)) if status == "added" => Some(AddTokenmaxxerStatusV1::Added),
        Some(Value::String(status)) if status == "already-added" => {
            Some(AddTokenmaxxerStatusV1::AlreadyAdded)
        }
        Some(Value::String(status)) if status == "invalid" => Some(AddTokenmaxxerStatusV1::Invalid),
        Some(Value::String(status)) if status == "limit-reached" => {
            Some(AddTokenmaxxerStatusV1::LimitReached)
        }
        Some(Value::String(status)) if status == "not-found" => {
            Some(AddTokenmaxxerStatusV1::NotFound)
        }
        Some(Value::String(status)) if status == "self" => {
            Some(AddTokenmaxxerStatusV1::SelfProfile)
        }
        _ => None,
    }
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
    if token_score == 0 {
        return None;
    }
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
    if rows.len() as u64 > saved_tokenmaxxer_count
        || (saved_tokenmaxxer_count > 0 && rows.is_empty())
    {
        Err(TransportError::Unavailable)
    } else {
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    #[cfg(target_os = "macos")]
    #[derive(Default)]
    struct CountingTransport {
        calls: AtomicUsize,
    }

    #[cfg(target_os = "macos")]
    impl DoomerboardTransport for CountingTransport {
        fn add(
            &self,
            _session: &Secret,
            _touch_grass_id: &str,
        ) -> Result<AddTokenmaxxerStatusV1, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(AddTokenmaxxerStatusV1::Added)
        }

        fn read(
            &self,
            _session: &Secret,
            _query: DoomerboardQueryV1,
        ) -> Result<Vec<DoomerboardRowV1>, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        }
    }

    #[cfg(target_os = "macos")]
    #[derive(Default)]
    struct CountingTokenProvider {
        calls: AtomicUsize,
    }

    #[cfg(target_os = "macos")]
    impl ConvexTokenProvider for CountingTokenProvider {
        fn fetch(
            &self,
            _session: &Secret,
            _now_unix_seconds: i64,
        ) -> Result<FetchedConvexToken, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(FetchedConvexToken {
                refresh_after_unix_seconds: 2_000,
                token: Zeroizing::new("test-jwt".to_owned()),
            })
        }
    }

    #[cfg(target_os = "macos")]
    #[derive(Default)]
    struct CountingConnection {
        authentications: AtomicUsize,
        calls: AtomicUsize,
    }

    #[cfg(target_os = "macos")]
    impl DoomerboardConnection for CountingConnection {
        fn authenticate(&self, _token: Zeroizing<String>) -> Result<(), TransportError> {
            self.authentications.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn call(&self, _call: ConvexCall) -> Result<FunctionResult, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(FunctionResult::Value(Value::Null))
        }
    }

    #[cfg(target_os = "macos")]
    struct CountingConnectionFactory {
        calls: AtomicUsize,
        connection: Arc<CountingConnection>,
    }

    #[cfg(target_os = "macos")]
    impl DoomerboardConnectionFactory for CountingConnectionFactory {
        fn connect(
            &self,
            _convex_url: &str,
        ) -> Result<Arc<dyn DoomerboardConnection>, TransportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.connection.clone())
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn reuses_one_authenticated_convex_connection_for_the_same_session() {
        let token_provider = Arc::new(CountingTokenProvider::default());
        let connection = Arc::new(CountingConnection::default());
        let connection_factory = Arc::new(CountingConnectionFactory {
            calls: AtomicUsize::new(0),
            connection: connection.clone(),
        });
        let transport = HttpDoomerboardTransport::new(
            Some("https://example.convex.cloud"),
            token_provider.clone(),
            connection_factory.clone(),
            Arc::new(|| 1_000),
        );
        let session = Secret::test_only();
        let call = || ConvexCall::Query {
            arguments: BTreeMap::new(),
            function_name: CURRENT_GLOBAL_QUERY,
        };

        transport
            .authenticated_call(&session, call())
            .expect("first authenticated call");
        transport
            .authenticated_call(&session, call())
            .expect("second authenticated call");

        assert_eq!(token_provider.calls.load(Ordering::Relaxed), 1);
        assert_eq!(connection_factory.calls.load(Ordering::Relaxed), 1);
        assert_eq!(connection.authentications.load(Ordering::Relaxed), 1);
        assert_eq!(connection.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn reauthenticates_the_reused_connection_when_the_session_changes() {
        let token_provider = Arc::new(CountingTokenProvider::default());
        let connection = Arc::new(CountingConnection::default());
        let connection_factory = Arc::new(CountingConnectionFactory {
            calls: AtomicUsize::new(0),
            connection: connection.clone(),
        });
        let transport = HttpDoomerboardTransport::new(
            Some("https://example.convex.cloud"),
            token_provider.clone(),
            connection_factory.clone(),
            Arc::new(|| 1_000),
        );
        let first_session = Secret::test_only();
        let replacement_session = Secret::new("replacement-session".to_owned());
        let call = || ConvexCall::Query {
            arguments: BTreeMap::new(),
            function_name: CURRENT_GLOBAL_QUERY,
        };

        transport
            .authenticated_call(&first_session, call())
            .expect("first authenticated call");
        transport
            .authenticated_call(&replacement_session, call())
            .expect("replacement authenticated call");

        assert_eq!(token_provider.calls.load(Ordering::Relaxed), 2);
        assert_eq!(connection_factory.calls.load(Ordering::Relaxed), 1);
        assert_eq!(connection.authentications.load(Ordering::Relaxed), 2);
        assert_eq!(connection.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn refreshes_authentication_when_the_cached_jwt_reaches_its_margin() {
        let now = Arc::new(AtomicI64::new(1_000));
        let token_provider = Arc::new(CountingTokenProvider::default());
        let connection = Arc::new(CountingConnection::default());
        let connection_factory = Arc::new(CountingConnectionFactory {
            calls: AtomicUsize::new(0),
            connection: connection.clone(),
        });
        let transport = HttpDoomerboardTransport::new(
            Some("https://example.convex.cloud"),
            token_provider.clone(),
            connection_factory.clone(),
            {
                let now = now.clone();
                Arc::new(move || now.load(Ordering::Relaxed))
            },
        );
        let session = Secret::test_only();
        let call = || ConvexCall::Query {
            arguments: BTreeMap::new(),
            function_name: CURRENT_GLOBAL_QUERY,
        };

        transport
            .authenticated_call(&session, call())
            .expect("first authenticated call");
        now.store(2_000, Ordering::Relaxed);
        transport
            .authenticated_call(&session, call())
            .expect("refreshed authenticated call");

        assert_eq!(token_provider.calls.load(Ordering::Relaxed), 2);
        assert_eq!(connection_factory.calls.load(Ordering::Relaxed), 1);
        assert_eq!(connection.authentications.load(Ordering::Relaxed), 2);
        assert_eq!(connection.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn reads_the_jwt_expiry_with_a_refresh_margin() {
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":2060}"#);
        let token = format!("header.{payload}.signature");

        assert_eq!(convex_jwt_refresh_after(&token, 1_000), 2_000);
    }

    #[test]
    fn refreshes_immediately_when_the_jwt_expiry_is_not_valid() {
        assert_eq!(convex_jwt_refresh_after("not-a-jwt", 1_000), 1_000);
        assert_eq!(
            convex_jwt_refresh_after("header.not-base64.signature", 1_000),
            1_000
        );
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

    fn add_outcome(status: &str) -> Value {
        Value::Object(BTreeMap::from([(
            "status".to_owned(),
            Value::String(status.to_owned()),
        )]))
    }

    #[test]
    fn accepts_only_bounded_add_tokenmaxxer_outcomes() {
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("added")),
            Some(AddTokenmaxxerStatusV1::Added)
        );
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("already-added")),
            Some(AddTokenmaxxerStatusV1::AlreadyAdded)
        );
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("invalid")),
            Some(AddTokenmaxxerStatusV1::Invalid)
        );
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("limit-reached")),
            Some(AddTokenmaxxerStatusV1::LimitReached)
        );
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("not-found")),
            Some(AddTokenmaxxerStatusV1::NotFound)
        );
        assert_eq!(
            parse_add_tokenmaxxer_status(add_outcome("self")),
            Some(AddTokenmaxxerStatusV1::SelfProfile)
        );

        let mut private_outcome = match add_outcome("added") {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        private_outcome.insert("session".to_owned(), Value::String("private".to_owned()));
        assert_eq!(
            parse_add_tokenmaxxer_status(Value::Object(private_outcome)),
            None
        );
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
        assert_eq!(
            parse_rows(Value::Array(vec![row("TG-234567", 1, 0)])),
            Err(TransportError::Unavailable)
        );
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
    fn add_mutation_sends_only_the_public_touch_grass_id() {
        assert_eq!(
            add_tokenmaxxer_arguments("TG-234567"),
            BTreeMap::from([(
                "touchGrassId".to_owned(),
                Value::String("TG-234567".to_owned()),
            )])
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
            parse_selected_rows(selected, saved_rows(2, vec![row("TG-234567", 1, 500)]),)
                .expect("omit an unscored saved Profile")
                .len(),
            1
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
    #[cfg(target_os = "macos")]
    fn add_validates_the_public_id_and_respects_the_online_gate() {
        let transport = Arc::new(CountingTransport::default());
        let coordinator = Arc::new(Mutex::new(crate::profile::production_coordinator(
            crate::lifecycle::DesktopLifecycle::unavailable(),
        )));
        let runtime = DoomerboardRuntime::new(
            Arc::clone(&coordinator),
            transport.clone(),
            OnlineFeatureGate::default(),
        );

        assert_eq!(
            runtime.add("private-input"),
            AddTokenmaxxerOutcomeV1::new(AddTokenmaxxerStatusV1::Invalid)
        );
        let paused =
            DoomerboardRuntime::new(coordinator, transport.clone(), OnlineFeatureGate::paused());
        assert_eq!(
            paused.add("TG-234567"),
            AddTokenmaxxerOutcomeV1::new(AddTokenmaxxerStatusV1::Unavailable)
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
