use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde::de::IgnoredAny;
use time::{
    Date, Duration, Month, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339,
};

use crate::sanitized::{
    ApiEquivalentCostQuality, UsageCoverage, UsageEvidenceBasis, UsagePeriods, UsageScanStatus,
    UsageTotal,
};

const OPENAI_STANDARD_PRICING_JSON: &str = include_str!("../pricing/openai-standard.json");
const MAX_ROLLOUT_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROLLOUT_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ROLLOUT_SCAN_MILLIS: u128 = 2_000;
const RETAINED_RANKING_DAYS: i64 = 60;
const REPRICE_ROWS_PER_PASS: usize = 256;
const PRUNE_ROWS_PER_PASS: usize = 1_000;
const ROLLOUT_PARSER_VERSION: i64 = 3;
const UNKNOWN_MODEL: &str = "__unknown__";

#[derive(Clone, Copy)]
struct ScanBudget {
    max_bytes: u64,
    max_millis: u128,
}

const DEFAULT_SCAN_BUDGET: ScanBudget = ScanBudget {
    max_bytes: MAX_ROLLOUT_SCAN_BYTES,
    max_millis: MAX_ROLLOUT_SCAN_MILLIS,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountUsageObservation {
    daily_tokens: BTreeMap<Date, u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawAccountUsageResponse {
    daily_usage_buckets: Option<Vec<RawDailyUsageBucket>>,
    summary: IgnoredAny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDailyUsageBucket {
    start_date: String,
    tokens: u64,
}

pub(crate) fn parse_account_usage(payload: &str) -> Result<AccountUsageObservation, ()> {
    let response: RawAccountUsageResponse = serde_json::from_str(payload).map_err(|_| ())?;
    let _ = response.summary;
    let mut daily_tokens = BTreeMap::new();
    for bucket in response.daily_usage_buckets.unwrap_or_default() {
        let day = parse_ranking_day(&bucket.start_date)?;
        if daily_tokens.insert(day, bucket.tokens).is_some() {
            return Err(());
        }
    }
    Ok(AccountUsageObservation { daily_tokens })
}

fn parse_ranking_day(value: &str) -> Result<Date, ()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(());
    }
    let parse = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .and_then(|part| part.parse::<u16>().ok())
            .ok_or(())
    };
    let year = i32::from(parse(0..4)?);
    let month = Month::try_from(u8::try_from(parse(5..7)?).map_err(|_| ())?).map_err(|_| ())?;
    let day = u8::try_from(parse(8..10)?).map_err(|_| ())?;
    Date::from_calendar_date(year, month, day).map_err(|_| ())
}

fn utc_ranking_day(timestamp: OffsetDateTime) -> Date {
    timestamp.to_offset(UtcOffset::UTC).date()
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct TokenUsage {
    #[serde(rename = "input_tokens")]
    input: u64,
    #[serde(rename = "cached_input_tokens")]
    cached_input: u64,
    #[serde(default, rename = "cache_write_input_tokens")]
    cache_write_input: u64,
    #[serde(rename = "output_tokens")]
    output: u64,
    #[serde(rename = "reasoning_output_tokens")]
    reasoning_output: u64,
    #[serde(rename = "total_tokens")]
    total: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BillableTokenUsage {
    standard_input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricingManifest {
    schema_version: u32,
    basis: String,
    models: Vec<RawPricedModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricedModel {
    name: String,
    aliases: Vec<String>,
    periods: Vec<RawPricePeriod>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricePeriod {
    effective_from: String,
    effective_until: Option<String>,
    input_usd_per_million: f64,
    cached_input_usd_per_million: f64,
    cache_write_usd_per_million: Option<f64>,
    output_usd_per_million: f64,
    long_context: RawLongContextRule,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLongContextRule {
    input_tokens_above: u64,
    input_multiplier: f64,
    output_multiplier: f64,
}

#[derive(Clone)]
struct PricingManifest {
    basis: String,
    fingerprint: String,
    models: Vec<PricedModel>,
}

#[derive(Clone)]
struct PricedModel {
    names: Vec<String>,
    periods: Vec<PriceCatalogEntry>,
}

#[derive(Clone, Copy)]
struct PriceCatalogEntry {
    effective_from: Date,
    effective_until: Option<Date>,
    input_usd_per_million: f64,
    cached_input_usd_per_million: f64,
    cache_write_usd_per_million: Option<f64>,
    output_usd_per_million: f64,
    long_context_input_tokens_above: u64,
    long_context_input_multiplier: f64,
    long_context_output_multiplier: f64,
}

impl PriceCatalogEntry {
    fn applies_to(self, day: Date) -> bool {
        day >= self.effective_from && self.effective_until.is_none_or(|until| day < until)
    }
}

fn parse_pricing_manifest(source: &str) -> Result<PricingManifest, ()> {
    let raw: RawPricingManifest = serde_json::from_str(source).map_err(|_| ())?;
    if raw.schema_version != 1 || raw.basis.is_empty() || raw.basis.len() > 64 {
        return Err(());
    }
    let mut known_names = BTreeSet::new();
    let mut models = Vec::new();
    for model in raw.models {
        let mut names = Vec::with_capacity(model.aliases.len() + 1);
        names.push(model.name);
        names.extend(model.aliases);
        for name in &names {
            if !valid_model_name(name) || !known_names.insert(name.clone()) {
                return Err(());
            }
        }
        if model.periods.is_empty() {
            return Err(());
        }
        let mut periods = model
            .periods
            .into_iter()
            .map(|period| {
                let effective_from = parse_ranking_day(&period.effective_from)?;
                let effective_until = period
                    .effective_until
                    .as_deref()
                    .map(parse_ranking_day)
                    .transpose()?;
                if effective_until.is_some_and(|until| until <= effective_from)
                    || period.long_context.input_tokens_above == 0
                    || ![
                        period.input_usd_per_million,
                        period.cached_input_usd_per_million,
                        period.output_usd_per_million,
                        period.long_context.input_multiplier,
                        period.long_context.output_multiplier,
                    ]
                    .into_iter()
                    .all(|value| value.is_finite() && value >= 0.0)
                    || period
                        .cache_write_usd_per_million
                        .is_some_and(|value| !value.is_finite() || value < 0.0)
                    || period.long_context.input_multiplier <= 0.0
                    || period.long_context.output_multiplier <= 0.0
                {
                    return Err(());
                }
                Ok(PriceCatalogEntry {
                    effective_from,
                    effective_until,
                    input_usd_per_million: period.input_usd_per_million,
                    cached_input_usd_per_million: period.cached_input_usd_per_million,
                    cache_write_usd_per_million: period.cache_write_usd_per_million,
                    output_usd_per_million: period.output_usd_per_million,
                    long_context_input_tokens_above: period.long_context.input_tokens_above,
                    long_context_input_multiplier: period.long_context.input_multiplier,
                    long_context_output_multiplier: period.long_context.output_multiplier,
                })
            })
            .collect::<Result<Vec<_>, ()>>()?;
        periods.sort_by_key(|period| period.effective_from);
        if periods.windows(2).any(|pair| {
            pair[0]
                .effective_until
                .is_none_or(|until| pair[1].effective_from < until)
        }) {
            return Err(());
        }
        models.push(PricedModel { names, periods });
    }
    if models.is_empty() {
        return Err(());
    }
    let fingerprint = pricing_manifest_fingerprint(&models);
    Ok(PricingManifest {
        basis: raw.basis,
        fingerprint,
        models,
    })
}

fn pricing_manifest_fingerprint(models: &[PricedModel]) -> String {
    let mut model_parts = models
        .iter()
        .map(|model| {
            let mut names = model.names.clone();
            names.sort();
            let periods = model
                .periods
                .iter()
                .map(|period| {
                    format!(
                        "{}|{}|{:016x}|{:016x}|{}|{:016x}|{}|{:016x}|{:016x}",
                        period.effective_from,
                        period
                            .effective_until
                            .map_or_else(|| "open".to_owned(), |until| until.to_string()),
                        period.input_usd_per_million.to_bits(),
                        period.cached_input_usd_per_million.to_bits(),
                        period.cache_write_usd_per_million.map_or_else(
                            || "none".to_owned(),
                            |price| format!("{:016x}", price.to_bits())
                        ),
                        period.output_usd_per_million.to_bits(),
                        period.long_context_input_tokens_above,
                        period.long_context_input_multiplier.to_bits(),
                        period.long_context_output_multiplier.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            format!("{}::{periods}", names.join(","))
        })
        .collect::<Vec<_>>();
    model_parts.sort();
    let canonical = model_parts.join("||");
    let hash = canonical
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("fnv1a64:{hash:016x}")
}

fn pricing_manifest() -> Option<&'static PricingManifest> {
    static MANIFEST: OnceLock<Result<PricingManifest, ()>> = OnceLock::new();
    MANIFEST
        .get_or_init(|| parse_pricing_manifest(OPENAI_STANDARD_PRICING_JSON))
        .as_ref()
        .ok()
}

fn catalog_entry(manifest: &PricingManifest, model: &str, day: Date) -> Option<PriceCatalogEntry> {
    manifest
        .models
        .iter()
        .find(|entry| entry.names.iter().any(|name| name == model))?
        .periods
        .iter()
        .copied()
        .find(|entry| entry.applies_to(day))
}

#[cfg(test)]
fn price_usage(model: &str, day: Date, usage: TokenUsage) -> Option<f64> {
    price_usage_tier(model, day, usage, usage.input)
}

#[cfg(test)]
fn price_usage_tier(
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
) -> Option<f64> {
    price_usage_tier_with_manifest(pricing_manifest()?, model, day, usage, pricing_input_tokens)
}

fn price_usage_tier_with_manifest(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
) -> Option<f64> {
    let entry = catalog_entry(manifest, model, day)?;
    let billable = usage.billable().ok()?;
    let long_context = pricing_input_tokens > entry.long_context_input_tokens_above;
    let input_multiplier = if long_context {
        entry.long_context_input_multiplier
    } else {
        1.0
    };
    let output_multiplier = if long_context {
        entry.long_context_output_multiplier
    } else {
        1.0
    };
    let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
    let cache_write = if billable.cache_write_input == 0 {
        0.0
    } else {
        per_million(
            billable.cache_write_input,
            entry.cache_write_usd_per_million?,
        )
    };
    let cost = input_multiplier
        * (per_million(billable.standard_input, entry.input_usd_per_million)
            + per_million(billable.cached_input, entry.cached_input_usd_per_million)
            + cache_write)
        + output_multiplier * per_million(billable.output, entry.output_usd_per_million);
    cost.is_finite().then_some(cost)
}

#[derive(Clone, Debug, PartialEq)]
struct LocalUsageDay {
    observed_tokens: u64,
    api_equivalent_cost_usd: Option<f64>,
    complete: bool,
    observed_through: Option<OffsetDateTime>,
}

impl Default for LocalUsageDay {
    fn default() -> Self {
        Self {
            observed_tokens: 0,
            api_equivalent_cost_usd: Some(0.0),
            complete: true,
            observed_through: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalUsageObservation {
    daily: BTreeMap<Date, LocalUsageDay>,
    scan_status: UsageScanStatus,
}

impl Default for LocalUsageObservation {
    fn default() -> Self {
        Self {
            daily: BTreeMap::new(),
            scan_status: UsageScanStatus::Unavailable,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRolloutHeader {
    timestamp: String,
    #[serde(rename = "type")]
    record_type: String,
    payload: IgnoredAny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEventLine {
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawEventPayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTurnContextLine {
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawTurnContext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessionMetaLine {
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawSessionMeta,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "type")]
enum RawEventPayload {
    TokenCount {
        info: RawTokenInfo,
        #[serde(default)]
        rate_limits: Option<IgnoredAny>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTokenInfo {
    last_token_usage: TokenUsage,
    model_context_window: u64,
    total_token_usage: TokenUsage,
}

#[derive(Deserialize)]
struct RawTurnContext {
    model: String,
}

#[derive(Deserialize)]
struct RawSessionMeta {
    cli_version: String,
    #[serde(default)]
    forked_from_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct RolloutScanState {
    active_model: Option<String>,
    baseline_is_inherited: Option<bool>,
    previous: Option<TokenUsage>,
    schema_supported: bool,
}

fn is_supported_cli_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some("0") = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u16>().ok()) else {
        return false;
    };
    let remainder = parts.collect::<Vec<_>>().join(".");
    (130..=146).contains(&minor)
        && !remainder.is_empty()
        && remainder
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn valid_model_name(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 64
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn parse_rollout_timestamp(timestamp: &str) -> Result<OffsetDateTime, ()> {
    OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| ())
}

#[cfg(test)]
fn mark_incomplete(days: &mut BTreeMap<Date, LocalUsageDay>, day: Date) {
    days.entry(day).or_default().complete = false;
}

#[cfg(test)]
fn add_delta(
    days: &mut BTreeMap<Date, LocalUsageDay>,
    observed_at: OffsetDateTime,
    model: Option<&str>,
    delta: TokenUsage,
) -> Result<(), ()> {
    if delta.total == 0 {
        return Ok(());
    }
    let day = utc_ranking_day(observed_at);
    let entry = days.entry(day).or_default();
    entry.observed_tokens = entry.observed_tokens.checked_add(delta.total).ok_or(())?;
    let priced = model.and_then(|model| price_usage(model, day, delta));
    entry.api_equivalent_cost_usd = entry
        .api_equivalent_cost_usd
        .zip(priced)
        .map(|(current, added)| current + added);
    if model.is_none() {
        entry.complete = false;
    }
    entry.observed_through = Some(
        entry
            .observed_through
            .map_or(observed_at, |current| current.max(observed_at)),
    );
    Ok(())
}

#[cfg(test)]
fn scan_rollout_reader(
    reader: impl BufRead,
    cutoff: Date,
    days: &mut BTreeMap<Date, LocalUsageDay>,
) -> bool {
    let mut state = RolloutScanState::default();
    let mut complete = true;
    for line in reader.split(b'\n') {
        let Ok(line) = line else {
            return false;
        };
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_ROLLOUT_LINE_BYTES {
            complete = false;
            continue;
        }
        let header: RawRolloutHeader = match serde_json::from_slice(&line) {
            Ok(header) => header,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let _ = header.payload;
        let timestamp = match parse_rollout_timestamp(&header.timestamp) {
            Ok(timestamp) => timestamp,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let day = utc_ranking_day(timestamp);
        if day < cutoff {
            continue;
        }
        match header.record_type.as_str() {
            "session_meta" => {
                let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(&line) else {
                    mark_incomplete(days, day);
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                let meta = line.payload;
                state.schema_supported = is_supported_cli_version(&meta.cli_version);
                state.baseline_is_inherited = Some(meta.forked_from_id.is_some());
                if !state.schema_supported {
                    mark_incomplete(days, day);
                    complete = false;
                }
            }
            "turn_context" => {
                let Ok(line) = serde_json::from_slice::<RawTurnContextLine>(&line) else {
                    mark_incomplete(days, day);
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                let context = line.payload;
                state.active_model = valid_model_name(&context.model).then_some(context.model);
                if state.active_model.is_none() {
                    mark_incomplete(days, day);
                }
            }
            "event_msg" => {
                let Ok(line) = serde_json::from_slice::<RawEventLine>(&line) else {
                    mark_incomplete(days, day);
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                let RawEventPayload::TokenCount { info, rate_limits } = line.payload else {
                    continue;
                };
                let _ = (
                    info.last_token_usage,
                    info.model_context_window,
                    rate_limits,
                );
                if !state.schema_supported {
                    mark_incomplete(days, day);
                    complete = false;
                    continue;
                }
                let current = info.total_token_usage;
                let delta = match state.previous {
                    Some(previous) => current.delta_from(previous),
                    None if state.baseline_is_inherited == Some(false) => {
                        current.delta_from(TokenUsage::default())
                    }
                    None if state.baseline_is_inherited == Some(true) => {
                        state.previous = Some(current);
                        continue;
                    }
                    None => {
                        state.previous = Some(current);
                        mark_incomplete(days, day);
                        continue;
                    }
                };
                state.previous = Some(current);
                match delta.and_then(|delta| {
                    add_delta(days, timestamp, state.active_model.as_deref(), delta)
                }) {
                    Ok(()) => {}
                    Err(()) => {
                        mark_incomplete(days, day);
                        complete = false;
                    }
                }
            }
            _ => {}
        }
    }
    complete
}

fn collect_rollout_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
    started: Instant,
    max_millis: u128,
) -> Result<(), ()> {
    for entry in fs::read_dir(root).map_err(|_| ())? {
        if started.elapsed().as_millis() >= max_millis {
            return Err(());
        }
        let entry = entry.map_err(|_| ())?;
        let file_type = entry.file_type().map_err(|_| ())?;
        if file_type.is_dir() {
            collect_rollout_files(&entry.path(), files, started, max_millis)?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn codex_data_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

#[derive(Clone, Debug)]
struct FileCursor {
    identity: String,
    size: u64,
    modified_ns: i64,
    parsed_offset: u64,
    completion_state: String,
    parser_version: i64,
    parser_state: RolloutScanState,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ModelDayKey {
    day: Date,
    model: String,
    pricing_input_tokens: u64,
}

#[derive(Clone, Debug)]
struct ModelDayDelta {
    usage: TokenUsage,
    complete: bool,
    observed_through: OffsetDateTime,
}

fn checked_add_usage(current: TokenUsage, delta: TokenUsage) -> Result<TokenUsage, ()> {
    let usage = TokenUsage {
        input: current.input.checked_add(delta.input).ok_or(())?,
        cached_input: current
            .cached_input
            .checked_add(delta.cached_input)
            .ok_or(())?,
        cache_write_input: current
            .cache_write_input
            .checked_add(delta.cache_write_input)
            .ok_or(())?,
        output: current.output.checked_add(delta.output).ok_or(())?,
        reasoning_output: current
            .reasoning_output
            .checked_add(delta.reasoning_output)
            .ok_or(())?,
        total: current.total.checked_add(delta.total).ok_or(())?,
    };
    usage.validate()?;
    Ok(usage)
}

fn add_model_day_delta(
    rows: &mut BTreeMap<ModelDayKey, ModelDayDelta>,
    timestamp: OffsetDateTime,
    model: Option<&str>,
    delta: TokenUsage,
) -> Result<(), ()> {
    if delta.total == 0 {
        return Ok(());
    }
    let key = ModelDayKey {
        day: utc_ranking_day(timestamp),
        model: model.unwrap_or(UNKNOWN_MODEL).to_owned(),
        pricing_input_tokens: delta.input,
    };
    let row = rows.entry(key).or_insert(ModelDayDelta {
        usage: TokenUsage::default(),
        complete: model.is_some(),
        observed_through: timestamp,
    });
    row.usage = checked_add_usage(row.usage, delta)?;
    row.complete &= model.is_some();
    row.observed_through = row.observed_through.max(timestamp);
    Ok(())
}

fn mark_model_day_incomplete(
    rows: &mut BTreeMap<ModelDayKey, ModelDayDelta>,
    timestamp: OffsetDateTime,
    model: Option<&str>,
) {
    let key = ModelDayKey {
        day: utc_ranking_day(timestamp),
        model: model.unwrap_or(UNKNOWN_MODEL).to_owned(),
        pricing_input_tokens: 0,
    };
    let row = rows.entry(key).or_insert(ModelDayDelta {
        usage: TokenUsage::default(),
        complete: false,
        observed_through: timestamp,
    });
    row.complete = false;
    row.observed_through = row.observed_through.max(timestamp);
}

fn process_index_line(
    line: &[u8],
    cutoff: Date,
    state: &mut RolloutScanState,
    rows: &mut BTreeMap<ModelDayKey, ModelDayDelta>,
) -> bool {
    if line.len() > MAX_ROLLOUT_LINE_BYTES {
        return false;
    }
    let header: RawRolloutHeader = match serde_json::from_slice(line) {
        Ok(header) => header,
        Err(_) => return false,
    };
    let timestamp = match parse_rollout_timestamp(&header.timestamp) {
        Ok(timestamp) => timestamp,
        Err(_) => return false,
    };
    let _ = header.payload;
    if utc_ranking_day(timestamp) < cutoff {
        return true;
    }
    match header.record_type.as_str() {
        "session_meta" => {
            let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(line) else {
                mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                return false;
            };
            let _ = (line.timestamp, line.record_type);
            state.schema_supported = is_supported_cli_version(&line.payload.cli_version);
            state.baseline_is_inherited = Some(line.payload.forked_from_id.is_some());
            if !state.schema_supported {
                mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
            }
            state.schema_supported
        }
        "turn_context" => {
            let Ok(line) = serde_json::from_slice::<RawTurnContextLine>(line) else {
                mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                return false;
            };
            let _ = (line.timestamp, line.record_type);
            state.active_model =
                valid_model_name(&line.payload.model).then_some(line.payload.model);
            if state.active_model.is_none() {
                mark_model_day_incomplete(rows, timestamp, None);
                return false;
            }
            true
        }
        "event_msg" => {
            let Ok(line) = serde_json::from_slice::<RawEventLine>(line) else {
                mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                return false;
            };
            let _ = (line.timestamp, line.record_type);
            let RawEventPayload::TokenCount { info, rate_limits } = line.payload else {
                return true;
            };
            let _ = (
                info.last_token_usage,
                info.model_context_window,
                rate_limits,
            );
            if !state.schema_supported {
                mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                return false;
            }
            let current = info.total_token_usage;
            let delta = match state.previous {
                Some(previous) => current.delta_from(previous),
                None if state.baseline_is_inherited == Some(false) => {
                    current.delta_from(TokenUsage::default())
                }
                None if state.baseline_is_inherited == Some(true) => {
                    state.previous = Some(current);
                    return true;
                }
                None => {
                    state.previous = Some(current);
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                    return false;
                }
            };
            state.previous = Some(current);
            match delta.and_then(|delta| {
                add_model_day_delta(rows, timestamp, state.active_model.as_deref(), delta)
            }) {
                Ok(()) => true,
                Err(()) => {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                    false
                }
            }
        }
        _ => true,
    }
}

fn ensure_index_schema(connection: &Connection) -> Result<(), ()> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS codex_usage_index_meta (
               key TEXT PRIMARY KEY NOT NULL,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_usage_files (
               path TEXT PRIMARY KEY NOT NULL,
               file_identity TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               modified_ns INTEGER NOT NULL,
               parsed_offset INTEGER NOT NULL,
               parser_version INTEGER NOT NULL,
               completion_state TEXT NOT NULL,
               active_model TEXT,
               baseline_is_inherited INTEGER,
               schema_supported INTEGER NOT NULL,
               previous_input INTEGER,
               previous_cached_input INTEGER,
               previous_cache_write_input INTEGER,
               previous_output INTEGER,
               previous_reasoning_output INTEGER,
               previous_total INTEGER
             );
             CREATE TABLE IF NOT EXISTS codex_usage_file_model_days (
               path TEXT NOT NULL,
               day TEXT NOT NULL,
               model TEXT NOT NULL,
               pricing_input_tokens INTEGER NOT NULL,
               input_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               cache_write_input_tokens INTEGER NOT NULL,
               output_tokens INTEGER NOT NULL,
               reasoning_output_tokens INTEGER NOT NULL,
               observed_tokens INTEGER NOT NULL,
               cost_usd REAL,
               pricing_basis TEXT,
               pricing_fingerprint TEXT,
               complete INTEGER NOT NULL,
               observed_through TEXT NOT NULL,
               PRIMARY KEY (path, day, model, pricing_input_tokens),
               FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
             );",
        )
        .map_err(|_| ())?;
    let has_pricing_fingerprint = connection
        .prepare("PRAGMA table_info(codex_usage_file_model_days)")
        .map_err(|_| ())?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?
        .iter()
        .any(|column| column == "pricing_fingerprint");
    if !has_pricing_fingerprint {
        connection
            .execute(
                "ALTER TABLE codex_usage_file_model_days
                 ADD COLUMN pricing_fingerprint TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

fn to_i64(value: u64) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

fn from_i64(value: i64) -> Result<u64, ()> {
    u64::try_from(value).map_err(|_| ())
}

fn reprice_index(connection: &Connection) -> Result<bool, ()> {
    reprice_index_batch_with_manifest(
        connection,
        pricing_manifest().ok_or(())?,
        REPRICE_ROWS_PER_PASS,
    )
}

#[cfg(test)]
fn reprice_index_with_manifest(
    connection: &Connection,
    manifest: &PricingManifest,
) -> Result<(), ()> {
    while !reprice_index_batch_with_manifest(connection, manifest, REPRICE_ROWS_PER_PASS)? {}
    Ok(())
}

fn reprice_index_batch_with_manifest(
    connection: &Connection,
    manifest: &PricingManifest,
    max_rows: usize,
) -> Result<bool, ()> {
    let mut statement = connection
        .prepare(
            "SELECT path, day, model, pricing_input_tokens, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                    observed_tokens
             FROM codex_usage_file_model_days
             WHERE pricing_basis IS NOT ?1 OR pricing_fingerprint IS NOT ?2
             ORDER BY day, path, model, pricing_input_tokens
             LIMIT ?3",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                manifest.basis.as_str(),
                manifest.fingerprint.as_str(),
                i64::try_from(max_rows).map_err(|_| ())?
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    from_i64(row.get(3)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    TokenUsage {
                        input: from_i64(row.get(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cached_input: from_i64(row.get(5)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cache_write_input: from_i64(row.get(6)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        output: from_i64(row.get(7)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        reasoning_output: from_i64(row.get(8)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        total: from_i64(row.get(9)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    },
                ))
            },
        )
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    drop(statement);
    let batch_complete = rows.len() < max_rows;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    for (path, day, model, pricing_input_tokens, usage) in rows {
        let day = parse_ranking_day(&day)?;
        let cost =
            price_usage_tier_with_manifest(manifest, &model, day, usage, pricing_input_tokens);
        transaction
            .execute(
                "UPDATE codex_usage_file_model_days
                 SET cost_usd = ?1, pricing_basis = ?2, pricing_fingerprint = ?3
                 WHERE path = ?4 AND day = ?5 AND model = ?6 AND pricing_input_tokens = ?7",
                params![
                    cost,
                    manifest.basis.as_str(),
                    manifest.fingerprint.as_str(),
                    path,
                    day.to_string(),
                    model,
                    to_i64(pricing_input_tokens)?
                ],
            )
            .map_err(|_| ())?;
    }
    transaction
        .execute(
            "INSERT INTO codex_usage_index_meta(key, value) VALUES('pricing_basis', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [manifest.basis.as_str()],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO codex_usage_index_meta(key, value)
             VALUES('pricing_manifest_fingerprint', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [manifest.fingerprint.as_str()],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())?;
    Ok(batch_complete)
}

fn prune_expired_model_days(connection: &Connection, cutoff: Date) -> Result<bool, ()> {
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).map_err(|_| ())?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_model_days
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_model_days WHERE day < ?1 LIMIT ?2
             )",
            params![
                cutoff.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let model_days_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "DELETE FROM codex_usage_files
             WHERE rowid IN (
               SELECT f.rowid FROM codex_usage_files f
               WHERE f.modified_ns < ?1
                 AND NOT EXISTS (
                   SELECT 1 FROM codex_usage_file_model_days d WHERE d.path = f.path
                 )
               LIMIT ?2
             )",
            params![
                cutoff_modified_ns,
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let files_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction.commit().map_err(|_| ())?;
    Ok(model_days_complete && files_complete)
}

fn file_modified_ns(metadata: &fs::Metadata) -> Result<i64, ()> {
    let duration = metadata
        .modified()
        .map_err(|_| ())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?;
    i64::try_from(duration.as_nanos()).map_err(|_| ())
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(metadata: &fs::Metadata) -> String {
    format!("{}", metadata.len())
}

fn load_file_cursor(connection: &Connection, path: &str) -> Result<Option<FileCursor>, ()> {
    connection
        .query_row(
            "SELECT file_identity, size_bytes, modified_ns, parsed_offset, completion_state,
                    active_model, baseline_is_inherited, schema_supported,
                    parser_version,
                    previous_input, previous_cached_input, previous_cache_write_input,
                    previous_output, previous_reasoning_output, previous_total
             FROM codex_usage_files WHERE path = ?1",
            [path],
            |row| {
                let previous_total = row.get::<_, Option<i64>>(14)?;
                let previous = previous_total
                    .map(|total| {
                        Ok::<TokenUsage, rusqlite::Error>(TokenUsage {
                            input: from_i64(row.get(9)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cached_input: from_i64(row.get(10)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_write_input: from_i64(row.get(11)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            output: from_i64(row.get(12)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            reasoning_output: from_i64(row.get(13)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            total: from_i64(total).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    })
                    .transpose()?;
                if previous.is_some_and(|usage| usage.validate().is_err()) {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(FileCursor {
                    identity: row.get(0)?,
                    size: from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    modified_ns: row.get(2)?,
                    parsed_offset: from_i64(row.get(3)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    completion_state: row.get(4)?,
                    parser_version: row.get(8)?,
                    parser_state: RolloutScanState {
                        active_model: row.get(5)?,
                        baseline_is_inherited: row.get::<_, Option<bool>>(6)?,
                        schema_supported: row.get(7)?,
                        previous,
                    },
                })
            },
        )
        .optional()
        .map_err(|_| ())
}

fn reset_file(connection: &Connection, path: &str) -> Result<(), ()> {
    connection
        .execute("DELETE FROM codex_usage_files WHERE path = ?1", [path])
        .map(|_| ())
        .map_err(|_| ())
}

fn commit_file_progress(
    connection: &Connection,
    path: &str,
    cursor: &FileCursor,
    rows: BTreeMap<ModelDayKey, ModelDayDelta>,
) -> Result<(), ()> {
    let manifest = pricing_manifest();
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO codex_usage_files(
               path, file_identity, size_bytes, modified_ns, parsed_offset, parser_version,
               completion_state, active_model, baseline_is_inherited, schema_supported,
               previous_input, previous_cached_input, previous_cache_write_input,
               previous_output, previous_reasoning_output, previous_total
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(path) DO UPDATE SET
               file_identity=excluded.file_identity, size_bytes=excluded.size_bytes,
               modified_ns=excluded.modified_ns, parsed_offset=excluded.parsed_offset,
               parser_version=excluded.parser_version, completion_state=excluded.completion_state,
               active_model=excluded.active_model,
               baseline_is_inherited=excluded.baseline_is_inherited,
               schema_supported=excluded.schema_supported, previous_input=excluded.previous_input,
               previous_cached_input=excluded.previous_cached_input,
               previous_cache_write_input=excluded.previous_cache_write_input,
               previous_output=excluded.previous_output,
               previous_reasoning_output=excluded.previous_reasoning_output,
               previous_total=excluded.previous_total",
            params![
                path,
                cursor.identity,
                to_i64(cursor.size)?,
                cursor.modified_ns,
                to_i64(cursor.parsed_offset)?,
                ROLLOUT_PARSER_VERSION,
                cursor.completion_state,
                cursor.parser_state.active_model,
                cursor.parser_state.baseline_is_inherited,
                cursor.parser_state.schema_supported,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.input))
                    .transpose()?,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.cached_input))
                    .transpose()?,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.cache_write_input))
                    .transpose()?,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.output))
                    .transpose()?,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.reasoning_output))
                    .transpose()?,
                cursor
                    .parser_state
                    .previous
                    .map(|usage| to_i64(usage.total))
                    .transpose()?,
            ],
        )
        .map_err(|_| ())?;
    for (key, delta) in rows {
        let cost = manifest.and_then(|manifest| {
            price_usage_tier_with_manifest(
                manifest,
                &key.model,
                key.day,
                delta.usage,
                key.pricing_input_tokens,
            )
        });
        transaction
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                   complete, observed_through
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(path, day, model, pricing_input_tokens) DO UPDATE SET
                   input_tokens=input_tokens + excluded.input_tokens,
                   cached_input_tokens=cached_input_tokens + excluded.cached_input_tokens,
                   cache_write_input_tokens=cache_write_input_tokens + excluded.cache_write_input_tokens,
                   output_tokens=output_tokens + excluded.output_tokens,
                   reasoning_output_tokens=reasoning_output_tokens + excluded.reasoning_output_tokens,
                   observed_tokens=observed_tokens + excluded.observed_tokens,
                   cost_usd=CASE WHEN cost_usd IS NULL OR excluded.cost_usd IS NULL THEN NULL ELSE cost_usd + excluded.cost_usd END,
                   pricing_basis=excluded.pricing_basis,
                   pricing_fingerprint=excluded.pricing_fingerprint,
                   complete=complete AND excluded.complete,
                   observed_through=MAX(observed_through, excluded.observed_through)",
                params![
                    path,
                    key.day.to_string(),
                    key.model,
                    to_i64(key.pricing_input_tokens)?,
                    to_i64(delta.usage.input)?,
                    to_i64(delta.usage.cached_input)?,
                    to_i64(delta.usage.cache_write_input)?,
                    to_i64(delta.usage.output)?,
                    to_i64(delta.usage.reasoning_output)?,
                    to_i64(delta.usage.total)?,
                    cost,
                    manifest.map(|manifest| manifest.basis.as_str()),
                    manifest.map(|manifest| manifest.fingerprint.as_str()),
                    delta.complete,
                    delta.observed_through.format(&Rfc3339).map_err(|_| ())?,
                ],
            )
            .map_err(|_| ())?;
    }
    transaction.commit().map_err(|_| ())
}

fn index_file(
    connection: &Connection,
    path: &Path,
    cutoff: Date,
    started: Instant,
    max_millis: u128,
    remaining_bytes: &mut u64,
) -> Result<bool, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    let path_value = path.to_string_lossy().into_owned();
    let identity = file_identity(&metadata);
    let size = metadata.len();
    let modified_ns = file_modified_ns(&metadata)?;
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).map_err(|_| ())?;
    if modified_ns < cutoff_modified_ns {
        reset_file(connection, &path_value)?;
        return Ok(true);
    }
    let stored = load_file_cursor(connection, &path_value)?;
    let rebuild = stored.as_ref().is_some_and(|cursor| {
        cursor.parser_version != ROLLOUT_PARSER_VERSION
            || cursor.identity != identity
            || size < cursor.size
            || size < cursor.parsed_offset
            || (size == cursor.size && modified_ns != cursor.modified_ns)
    });
    if rebuild {
        reset_file(connection, &path_value)?;
    }
    let stored = if rebuild { None } else { stored };
    if let Some(cursor) = &stored
        && cursor.parsed_offset == size
        && cursor.size == size
        && cursor.modified_ns == modified_ns
        && cursor.completion_state == "complete"
    {
        return Ok(true);
    }
    if *remaining_bytes == 0 || started.elapsed().as_millis() >= max_millis {
        return Ok(false);
    }
    let mut cursor = stored.unwrap_or(FileCursor {
        identity: identity.clone(),
        size,
        modified_ns,
        parsed_offset: 0,
        completion_state: "indexing".to_owned(),
        parser_version: ROLLOUT_PARSER_VERSION,
        parser_state: RolloutScanState::default(),
    });
    cursor.identity = identity;
    cursor.size = size;
    cursor.modified_ns = modified_ns;
    let mut file = fs::File::open(path).map_err(|_| ())?;
    file.seek(SeekFrom::Start(cursor.parsed_offset))
        .map_err(|_| ())?;
    let mut reader = BufReader::new(file);
    let mut rows = BTreeMap::new();
    let mut parser_complete =
        cursor.completion_state != "error" && cursor.completion_state != "discarding-overlong-line";
    let mut discarding_overlong_line = cursor.completion_state == "discarding-overlong-line";
    loop {
        if *remaining_bytes == 0 || started.elapsed().as_millis() >= max_millis {
            break;
        }
        let mut line = Vec::new();
        let read_limit = (*remaining_bytes).min((MAX_ROLLOUT_LINE_BYTES as u64) + 1);
        let bytes = Read::by_ref(&mut reader)
            .take(read_limit)
            .read_until(b'\n', &mut line)
            .map_err(|_| ())?;
        if bytes == 0 {
            break;
        }
        let bytes = u64::try_from(bytes).map_err(|_| ())?;
        let ends_with_newline = line.ends_with(b"\n");
        if discarding_overlong_line {
            *remaining_bytes -= bytes;
            cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
            if ends_with_newline {
                discarding_overlong_line = false;
            }
            continue;
        }
        if !ends_with_newline {
            let hit_line_limit =
                read_limit == (MAX_ROLLOUT_LINE_BYTES as u64) + 1 && bytes == read_limit;
            if hit_line_limit {
                parser_complete = false;
                discarding_overlong_line = true;
                *remaining_bytes -= bytes;
                cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
                continue;
            }
            break;
        }
        *remaining_bytes -= bytes;
        cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if !line.is_empty() {
            parser_complete &=
                process_index_line(&line, cutoff, &mut cursor.parser_state, &mut rows);
        }
    }
    cursor.completion_state = if discarding_overlong_line && cursor.parsed_offset < size {
        "discarding-overlong-line"
    } else if cursor.parsed_offset == size {
        if parser_complete { "complete" } else { "error" }
    } else {
        "indexing"
    }
    .to_owned();
    commit_file_progress(connection, &path_value, &cursor, rows)?;
    Ok(cursor.parsed_offset == size)
}

fn read_indexed_usage(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    scan_status: UsageScanStatus,
) -> Result<LocalUsageObservation, ()> {
    let mut statement = connection
        .prepare(
            "SELECT d.day, SUM(d.observed_tokens), SUM(d.cost_usd),
                    MIN(CASE WHEN f.completion_state = 'complete'
                                  AND d.complete = 1 AND d.cost_usd IS NOT NULL
                                  AND d.pricing_fingerprint = ?4
                             THEN 1 ELSE 0 END),
                    MAX(d.observed_through)
             FROM codex_usage_file_model_days d
             JOIN codex_usage_files f ON f.path = d.path
             WHERE f.parser_version = ?1 AND d.day >= ?2 AND d.day <= ?3
             GROUP BY d.day ORDER BY d.day",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string(),
                pricing_manifest().map(|manifest| manifest.fingerprint.as_str())
            ],
            |row| {
                let day = parse_ranking_day(&row.get::<_, String>(0)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let observed_tokens =
                    from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let cost = row.get::<_, Option<f64>>(2)?;
                let complete = row.get::<_, bool>(3)?;
                let observed_through = OffsetDateTime::parse(&row.get::<_, String>(4)?, &Rfc3339)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((
                    day,
                    LocalUsageDay {
                        observed_tokens,
                        api_equivalent_cost_usd: cost,
                        complete,
                        observed_through: Some(observed_through),
                    },
                ))
            },
        )
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())?;
    Ok(LocalUsageObservation {
        daily: rows,
        scan_status,
    })
}

fn index_local_usage_at(
    database_path: &Path,
    home: &Path,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    index_local_usage_with_budget(database_path, home, now, DEFAULT_SCAN_BUDGET)
}

fn index_local_usage_with_budget(
    database_path: &Path,
    home: &Path,
    now: OffsetDateTime,
    budget: ScanBudget,
) -> Option<LocalUsageObservation> {
    let started = Instant::now();
    let connection = Connection::open(database_path).ok()?;
    ensure_index_schema(&connection).ok()?;
    let today = utc_ranking_day(now);
    let cutoff = today - Duration::days(RETAINED_RANKING_DAYS - 1);
    let retention_complete = prune_expired_model_days(&connection, cutoff).ok()?;
    let pricing_complete = reprice_index(&connection).ok()?;
    if !retention_complete || !pricing_complete {
        return read_indexed_usage(&connection, cutoff, today, UsageScanStatus::Indexing).ok();
    }
    let mut files = Vec::new();
    let mut found_root = false;
    let mut traversal_complete = true;
    let max_millis = budget.max_millis.min(MAX_ROLLOUT_SCAN_MILLIS);
    for directory in ["sessions", "archived_sessions"] {
        let root = home.join(directory);
        if !root.is_dir() {
            continue;
        }
        found_root = true;
        traversal_complete &= collect_rollout_files(&root, &mut files, started, max_millis).is_ok();
    }
    if !found_root {
        return None;
    }
    let mut ordered_files = Vec::with_capacity(files.len());
    for path in files {
        if started.elapsed().as_millis() >= max_millis {
            traversal_complete = false;
            break;
        }
        match fs::metadata(&path).and_then(|metadata| metadata.modified()) {
            Ok(modified) => ordered_files.push((modified, path)),
            Err(_) => traversal_complete = false,
        }
    }
    ordered_files.sort_by(|left, right| right.0.cmp(&left.0));
    let files = ordered_files
        .into_iter()
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    let present = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    let stored_paths = connection
        .prepare("SELECT path FROM codex_usage_files")
        .ok()?
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if traversal_complete {
        for missing in stored_paths
            .into_iter()
            .filter(|path| !present.contains(path))
        {
            reset_file(&connection, &missing).ok()?;
        }
    }
    let mut remaining_bytes = budget.max_bytes.min(MAX_ROLLOUT_SCAN_BYTES);
    let mut all_complete = traversal_complete;
    let mut failed = false;
    for path in files {
        if started.elapsed().as_millis() >= max_millis {
            all_complete = false;
            break;
        }
        match index_file(
            &connection,
            &path,
            cutoff,
            started,
            max_millis,
            &mut remaining_bytes,
        ) {
            Ok(complete) => all_complete &= complete,
            Err(()) => {
                failed = true;
                all_complete = false;
            }
        }
        if remaining_bytes == 0 {
            all_complete = false;
            break;
        }
    }
    let has_errors = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM codex_usage_files
               WHERE parser_version = ?1 AND completion_state = 'error'
             )",
            [ROLLOUT_PARSER_VERSION],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(true);
    let scan_status = if failed {
        UsageScanStatus::Unavailable
    } else if !all_complete {
        UsageScanStatus::Indexing
    } else if has_errors {
        UsageScanStatus::Unavailable
    } else {
        UsageScanStatus::Complete
    };
    read_indexed_usage(&connection, cutoff, today, scan_status).ok()
}

pub(crate) fn scan_local_usage(
    database_path: Option<&Path>,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    index_local_usage_at(database_path?, &codex_data_home()?, now)
}

fn period_days(today: Date, length: i64, offset: i64) -> impl Iterator<Item = Date> {
    (0..length).map(move |index| today - Duration::days(offset + index))
}

fn checked_sum<'a>(mut values: impl Iterator<Item = &'a u64>) -> Option<u64> {
    values.try_fold(0_u64, |total, value| total.checked_add(*value))
}

fn trend_percent(current: u64, previous: u64) -> Option<f64> {
    if previous == 0 {
        return None;
    }
    let trend = ((current as f64 - previous as f64) / previous as f64) * 100.0;
    trend.is_finite().then_some(trend)
}

#[derive(Clone, Copy)]
struct CostProjection {
    usd: f64,
    quality: ApiEquivalentCostQuality,
    coverage_percent: Option<f64>,
}

fn account_cost(
    account_tokens: &BTreeMap<Date, u64>,
    days: impl Iterator<Item = Date>,
    local: Option<&LocalUsageObservation>,
    account_observed_at: OffsetDateTime,
) -> Option<CostProjection> {
    let local = local?;
    let mut usd = 0.0;
    let mut covered_tokens = 0_u64;
    let mut total_tokens = 0_u64;
    let mut modeled = false;
    for (day, account_tokens) in
        days.filter_map(|day| account_tokens.get(&day).map(|tokens| (day, *tokens)))
    {
        total_tokens = total_tokens.checked_add(account_tokens)?;
        if account_tokens == 0 {
            if local
                .daily
                .get(&day)
                .is_some_and(|detail| detail.observed_tokens > 0)
            {
                return None;
            }
            continue;
        }
        let detail = local.daily.get(&day)?;
        if !detail.complete
            || detail.observed_tokens == 0
            || detail.observed_tokens > account_tokens
            || detail.observed_through? > account_observed_at
        {
            return None;
        }
        let local_cost = detail.api_equivalent_cost_usd?;
        covered_tokens = covered_tokens.checked_add(detail.observed_tokens)?;
        if detail.observed_tokens == account_tokens {
            usd += local_cost;
        } else {
            modeled = true;
            usd += local_cost * (account_tokens as f64 / detail.observed_tokens as f64);
        }
    }
    if !usd.is_finite() {
        return None;
    }
    let coverage_percent = modeled.then(|| (covered_tokens as f64 / total_tokens as f64) * 100.0);
    Some(CostProjection {
        usd,
        quality: if modeled {
            ApiEquivalentCostQuality::Modeled
        } else {
            ApiEquivalentCostQuality::Reconciled
        },
        coverage_percent,
    })
}

fn local_cost(
    local: &LocalUsageObservation,
    days: impl Iterator<Item = Date>,
) -> Option<CostProjection> {
    let usd = days
        .filter_map(|day| local.daily.get(&day))
        .try_fold(0.0, |total, detail| {
            detail
                .complete
                .then_some(total + detail.api_equivalent_cost_usd?)
        })?;
    usd.is_finite().then_some(CostProjection {
        usd,
        quality: ApiEquivalentCostQuality::LocalOnly,
        coverage_percent: None,
    })
}

fn project_period(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
    length: i64,
) -> UsageTotal {
    let today = utc_ranking_day(now);
    let (tokens, evidence_basis, coverage, cost) = if let Some(account) = account {
        let expected = usize::try_from(length).unwrap_or(usize::MAX);
        let observed_days = period_days(today, length, 0)
            .filter(|day| account.daily_tokens.contains_key(day))
            .count();
        let Some(tokens) = checked_sum(
            period_days(today, length, 0).filter_map(|day| account.daily_tokens.get(&day)),
        ) else {
            return UsageTotal::Unavailable;
        };
        if observed_days == 0 {
            return UsageTotal::Unavailable;
        }
        (
            tokens,
            UsageEvidenceBasis::ProviderReported,
            if observed_days == expected {
                UsageCoverage::Complete
            } else {
                UsageCoverage::Partial
            },
            account_cost(
                &account.daily_tokens,
                period_days(today, length, 0),
                local,
                now,
            ),
        )
    } else if let Some(local) = local {
        let observed_days = period_days(today, length, 0)
            .filter(|day| local.daily.contains_key(day))
            .count();
        let Some(tokens) = checked_sum(
            period_days(today, length, 0)
                .filter_map(|day| local.daily.get(&day).map(|detail| &detail.observed_tokens)),
        ) else {
            return UsageTotal::Unavailable;
        };
        if observed_days == 0 {
            return UsageTotal::Unavailable;
        }
        (
            tokens,
            UsageEvidenceBasis::LocallyDerived,
            UsageCoverage::Partial,
            local_cost(local, period_days(today, length, 0)),
        )
    } else {
        return UsageTotal::Unavailable;
    };

    let trend = if coverage == UsageCoverage::Complete {
        let source = account.map(|account| &account.daily_tokens);
        source.and_then(|daily| {
            let previous_days = period_days(today, length, length).collect::<Vec<_>>();
            (previous_days.iter().all(|day| daily.contains_key(day)))
                .then(|| checked_sum(previous_days.iter().filter_map(|day| daily.get(day))))
                .flatten()
                .and_then(|previous| trend_percent(tokens, previous))
        })
    } else {
        None
    };
    let observed_at = now
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    UsageTotal::Current {
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens: tokens,
        api_equivalent_cost_usd: cost.map(|cost| cost.usd),
        trend_percent: trend,
        api_equivalent_cost_basis: cost
            .and_then(|_| pricing_manifest().map(|manifest| manifest.basis.clone())),
        api_equivalent_cost_quality: cost.map(|cost| cost.quality),
        api_equivalent_cost_coverage_percent: cost.and_then(|cost| cost.coverage_percent),
    }
}

pub(crate) fn project_usage_periods(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
) -> UsagePeriods {
    UsagePeriods {
        scan_status: local.map_or(UsageScanStatus::Unavailable, |local| local.scan_status),
        today: project_period(account, local, now, 1),
        seven_days: project_period(account, local, now, 7),
        thirty_days: project_period(account, local, now, 30),
    }
}

impl TokenUsage {
    fn validate(self) -> Result<(), ()> {
        if self.total != self.input.checked_add(self.output).ok_or(())?
            || self.input
                < self
                    .cached_input
                    .checked_add(self.cache_write_input)
                    .ok_or(())?
            || self.reasoning_output > self.output
        {
            return Err(());
        }
        Ok(())
    }

    fn delta_from(self, previous: Self) -> Result<Self, ()> {
        self.validate()?;
        previous.validate()?;
        let delta = Self {
            input: self.input.checked_sub(previous.input).ok_or(())?,
            cached_input: self
                .cached_input
                .checked_sub(previous.cached_input)
                .ok_or(())?,
            cache_write_input: self
                .cache_write_input
                .checked_sub(previous.cache_write_input)
                .ok_or(())?,
            output: self.output.checked_sub(previous.output).ok_or(())?,
            reasoning_output: self
                .reasoning_output
                .checked_sub(previous.reasoning_output)
                .ok_or(())?,
            total: self.total.checked_sub(previous.total).ok_or(())?,
        };
        delta.validate()?;
        Ok(delta)
    }

    fn billable(self) -> Result<BillableTokenUsage, ()> {
        self.validate()?;
        Ok(BillableTokenUsage {
            standard_input: self
                .input
                .checked_sub(self.cached_input)
                .and_then(|value| value.checked_sub(self.cache_write_input))
                .ok_or(())?,
            cached_input: self.cached_input,
            cache_write_input: self.cache_write_input,
            output: self.output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    struct TempUsage {
        root: PathBuf,
        database: PathBuf,
        rollout: PathBuf,
    }

    impl TempUsage {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "touchgrassbar-codex-usage-{}-{}",
                std::process::id(),
                OffsetDateTime::now_utc().unix_timestamp_nanos()
            ));
            let sessions = root.join("sessions");
            fs::create_dir_all(&sessions).unwrap();
            Self {
                database: root.join("touchgrassbar.sqlite3"),
                rollout: sessions.join("rollout.jsonl"),
                root,
            }
        }
    }

    impl Drop for TempUsage {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn root_rollout(total: u64) -> String {
        let input = total * 7 / 10;
        let output = total - input;
        [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total},"model_context_window":1050000,"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n"
    }

    fn appended_total(total: u64) -> String {
        let input = total * 7 / 10;
        let output = total - input;
        json!({"timestamp":"2026-08-06T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total},"model_context_window":1050000,"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total}},"rate_limits":null}}).to_string()
            + "\n"
    }

    fn changed_pricing_manifest(basis: &str, output_rate: f64) -> PricingManifest {
        let mut value: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        value["basis"] = json!(basis);
        value["models"][1]["periods"][0]["outputUsdPerMillion"] = json!(output_rate);
        parse_pricing_manifest(&value.to_string()).unwrap()
    }

    #[test]
    fn account_usage_accepts_only_unique_utc_daily_buckets() {
        let observation = parse_account_usage(
            r#"{
              "dailyUsageBuckets": [
                { "startDate": "2026-08-05", "tokens": 120 },
                { "startDate": "2026-08-06", "tokens": 340 }
              ],
              "summary": {
                "currentStreakDays": 2,
                "lifetimeTokens": 460,
                "longestRunningTurnSec": 30,
                "longestStreakDays": 2,
                "peakDailyTokens": 340
              }
            }"#,
        )
        .expect("valid account usage");

        assert_eq!(observation.daily_tokens.len(), 2);
        assert_eq!(
            observation
                .daily_tokens
                .get(&Date::from_calendar_date(2026, time::Month::August, 6).unwrap()),
            Some(&340)
        );
    }

    #[test]
    fn account_usage_rejects_duplicate_days_and_unknown_bucket_fields() {
        let duplicate = r#"{
          "dailyUsageBuckets": [
            { "startDate": "2026-08-06", "tokens": 1 },
            { "startDate": "2026-08-06", "tokens": 2 }
          ],
          "summary": {}
        }"#;
        let changed = r#"{
          "dailyUsageBuckets": [
            { "startDate": "2026-08-06", "tokens": 1, "providerId": "sentinel" }
          ],
          "summary": {}
        }"#;

        assert!(parse_account_usage(duplicate).is_err());
        assert!(parse_account_usage(changed).is_err());
    }

    #[test]
    fn account_usage_accepts_a_null_daily_bucket_list_as_available_without_days() {
        let observation =
            parse_account_usage(r#"{"dailyUsageBuckets":null,"summary":{"lifetimeTokens":0}}"#)
                .expect("available account usage");

        assert!(observation.daily_tokens.is_empty());
    }

    #[test]
    fn sqlite_index_skips_unchanged_files_and_resumes_appends() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(first.daily[&now.date()].observed_tokens, 100);
        let connection = Connection::open(&fixture.database).unwrap();
        let first_cursor: (i64, i64, i64, String, i64) = connection
            .query_row(
                "SELECT size_bytes, modified_ns, parsed_offset, completion_state, parser_version FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        drop(connection);

        let unchanged = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(unchanged.daily[&now.date()].observed_tokens, 100);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        file.write_all(appended_total(300).as_bytes()).unwrap();
        drop(file);

        let appended = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(appended.daily[&now.date()].observed_tokens, 300);
        let connection = Connection::open(&fixture.database).unwrap();
        let cursor: (i64, i64, i64, String, i64) = connection
            .query_row(
                "SELECT size_bytes, modified_ns, parsed_offset, completion_state, parser_version FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(first_cursor.0, first_cursor.2);
        assert_eq!(first_cursor.3, "complete");
        assert_eq!(first_cursor.4, ROLLOUT_PARSER_VERSION);
        assert!(cursor.0 > first_cursor.0);
        assert_eq!(cursor.0, cursor.2);
        assert_eq!(cursor.3, "complete");
    }

    #[test]
    fn sqlite_index_rebuilds_a_truncated_file_instead_of_adding_old_tokens() {
        let fixture = TempUsage::new();
        fs::write(
            &fixture.rollout,
            format!("{}{}", root_rollout(100), appended_total(300)),
        )
        .unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        assert_eq!(
            index_local_usage_at(&fixture.database, &fixture.root, now)
                .unwrap()
                .daily[&now.date()]
                .observed_tokens,
            300
        );

        fs::write(&fixture.rollout, root_rollout(50)).unwrap();
        let rebuilt = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(rebuilt.daily[&now.date()].observed_tokens, 50);

        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .execute("UPDATE codex_usage_files SET parser_version = 0", [])
            .unwrap();
        drop(connection);
        let reparsed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(reparsed.daily[&now.date()].observed_tokens, 50);
        let connection = Connection::open(&fixture.database).unwrap();
        let parser_version: i64 = connection
            .query_row("SELECT parser_version FROM codex_usage_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(parser_version, ROLLOUT_PARSER_VERSION);
    }

    #[test]
    fn sqlite_index_commits_usage_delta_and_cursor_in_one_transaction() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let before_offset: i64 = connection
            .query_row("SELECT parsed_offset FROM codex_usage_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_usage_delta BEFORE INSERT ON codex_usage_file_model_days
                 BEGIN SELECT RAISE(ABORT, 'test rollback'); END;",
            )
            .unwrap();
        drop(connection);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        file.write_all(appended_total(300).as_bytes()).unwrap();
        drop(file);

        let failed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(failed.daily[&now.date()].observed_tokens, 100);
        let connection = Connection::open(&fixture.database).unwrap();
        let after_offset: i64 = connection
            .query_row("SELECT parsed_offset FROM codex_usage_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after_offset, before_offset);
        connection
            .execute("DROP TRIGGER reject_usage_delta", [])
            .unwrap();
        drop(connection);

        let resumed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(resumed.daily[&now.date()].observed_tokens, 300);
    }

    #[test]
    fn sqlite_index_applies_byte_and_time_limits_and_keeps_prior_rows() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        file.write_all(appended_total(300).as_bytes()).unwrap();
        drop(file);

        let time_limited = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: MAX_ROLLOUT_SCAN_BYTES,
                max_millis: 0,
            },
        )
        .unwrap();
        assert_eq!(time_limited.scan_status, UsageScanStatus::Indexing);
        assert_eq!(time_limited.daily[&now.date()].observed_tokens, 100);
        let byte_limited = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: 1,
                max_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert_eq!(byte_limited.scan_status, UsageScanStatus::Indexing);
        assert_eq!(byte_limited.daily[&now.date()].observed_tokens, 100);

        let complete = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(complete.daily[&now.date()].observed_tokens, 300);
    }

    #[test]
    fn pricing_basis_change_changes_only_cost_not_tokens_or_ranking_input() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let before: (i64, i64, i64, i64, i64, i64, f64) = connection
            .query_row(
                "SELECT f.parsed_offset, d.input_tokens, d.cached_input_tokens,
                        d.cache_write_input_tokens, d.output_tokens, d.observed_tokens, d.cost_usd
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        let changed = changed_pricing_manifest("test-price-basis-v2", 60.0);
        reprice_index_with_manifest(&connection, &changed).unwrap();
        let after: (i64, i64, i64, i64, i64, i64, f64, String) = connection
            .query_row(
                "SELECT f.parsed_offset, d.input_tokens, d.cached_input_tokens,
                        d.cache_write_input_tokens, d.output_tokens, d.observed_tokens,
                        d.cost_usd, d.pricing_basis
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(before.0, after.0, "rollout cursor must not move");
        assert_eq!(before.1, after.1, "input tokens are ranking evidence");
        assert_eq!(before.2, after.2, "cached tokens remain subordinate");
        assert_eq!(before.3, after.3, "cache-write tokens do not change");
        assert_eq!(before.4, after.4, "output tokens are ranking evidence");
        assert_eq!(
            before.5, after.5,
            "Observed Tokens and Token Score input do not change"
        );
        assert_ne!(before.6, after.6, "only cost must be repriced");
        assert_eq!(after.7, "test-price-basis-v2");
    }

    #[test]
    fn semantic_price_change_with_same_basis_reprices_without_rescanning() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let before: (i64, i64, f64) = connection
            .query_row(
                "SELECT f.parsed_offset, d.observed_tokens, d.cost_usd
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let changed = changed_pricing_manifest("openai-standard-2026-08-06-v1", 60.0);
        reprice_index_with_manifest(&connection, &changed).unwrap();
        let after: (i64, i64, f64) = connection
            .query_row(
                "SELECT f.parsed_offset, d.observed_tokens, d.cost_usd
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(before.0, after.0, "rollout cursor must not move");
        assert_eq!(before.1, after.1, "Observed Tokens must not change");
        assert_ne!(before.2, after.2, "semantic price changes must reprice");
    }

    #[test]
    fn index_prunes_private_model_days_outside_the_retention_window() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let path: String = connection
            .query_row("SELECT path FROM codex_usage_files", [], |row| row.get(0))
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, complete, observed_through
                 ) VALUES (?1, '2026-01-01', 'gpt-5.6-sol', 7, 7, 0, 0, 3, 0, 10,
                           0.000125, 'old-basis', 1, '2026-01-01T12:00:00Z')",
                [path],
            )
            .unwrap();
        drop(connection);

        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let expired_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM codex_usage_file_model_days WHERE day < '2026-06-08'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expired_rows, 0);
    }

    #[test]
    fn ranking_days_are_always_utc() {
        let timestamp = OffsetDateTime::parse("2026-08-06T00:30:00+02:00", &Rfc3339).unwrap();
        assert_eq!(
            utc_ranking_day(timestamp),
            Date::from_calendar_date(2026, Month::August, 5).unwrap()
        );
    }

    #[test]
    fn bundled_pricing_manifest_is_strict_and_validated() {
        let manifest = parse_pricing_manifest(OPENAI_STANDARD_PRICING_JSON).unwrap();
        assert_eq!(manifest.basis, "openai-standard-2026-08-06-v1");
        assert!(
            catalog_entry(
                &manifest,
                "gpt-5.6",
                Date::from_calendar_date(2026, Month::August, 6).unwrap()
            )
            .is_some()
        );

        let mut unknown: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        unknown["unexpected"] = json!(true);
        assert!(parse_pricing_manifest(&unknown.to_string()).is_err());

        let mut duplicate: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        duplicate["models"][1]["aliases"] = json!(["gpt-5.5"]);
        assert!(parse_pricing_manifest(&duplicate.to_string()).is_err());

        let mut overlap: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        let period = overlap["models"][0]["periods"][0].clone();
        overlap["models"][0]["periods"]
            .as_array_mut()
            .unwrap()
            .push(period);
        assert!(parse_pricing_manifest(&overlap.to_string()).is_err());

        let mut bad_date: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        bad_date["models"][0]["periods"][0]["effectiveFrom"] = json!("2026-02-30");
        assert!(parse_pricing_manifest(&bad_date.to_string()).is_err());

        let mut negative: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        negative["models"][0]["periods"][0]["inputUsdPerMillion"] = json!(-1.0);
        assert!(parse_pricing_manifest(&negative.to_string()).is_err());

        let mut multiplier: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        multiplier["models"][0]["periods"][0]["longContext"]["inputMultiplier"] = json!(0.0);
        assert!(parse_pricing_manifest(&multiplier.to_string()).is_err());
    }

    #[test]
    fn cumulative_delta_does_not_double_count_cached_or_reasoning_tokens() {
        let previous = TokenUsage {
            input: 1_000,
            cached_input: 400,
            output: 200,
            reasoning_output: 80,
            total: 1_200,
            ..TokenUsage::default()
        };
        let current = TokenUsage {
            input: 1_700,
            cached_input: 700,
            output: 500,
            reasoning_output: 200,
            total: 2_200,
            ..TokenUsage::default()
        };

        let delta = current.delta_from(previous).expect("monotonic delta");
        assert_eq!(delta.total, 1_000);
        assert_eq!(
            delta.billable().unwrap(),
            BillableTokenUsage {
                standard_input: 400,
                cached_input: 300,
                cache_write_input: 0,
                output: 300,
            }
        );
    }

    #[test]
    fn parser_accepts_only_the_reviewed_codex_cli_version_range() {
        assert!(is_supported_cli_version("0.130.0-alpha.5"));
        assert!(is_supported_cli_version("0.145.0"));
        assert!(is_supported_cli_version("0.146.0-alpha.9.2"));
        assert!(!is_supported_cli_version("0.129.9"));
        assert!(!is_supported_cli_version("0.147.0"));
        assert!(!is_supported_cli_version("1.0.0"));
        assert!(!is_supported_cli_version("private value"));
    }

    #[test]
    fn invalid_token_arithmetic_fails_closed() {
        let cached_exceeds_input = TokenUsage {
            input: 10,
            cached_input: 11,
            output: 2,
            total: 12,
            ..TokenUsage::default()
        };
        let reasoning_exceeds_output = TokenUsage {
            input: 10,
            output: 2,
            reasoning_output: 3,
            total: 12,
            ..TokenUsage::default()
        };

        assert!(cached_exceeds_input.billable().is_err());
        assert!(reasoning_exceeds_output.billable().is_err());
    }

    #[test]
    fn pricing_is_effective_dated_and_unknown_prices_are_unavailable() {
        let before_release = Date::from_calendar_date(2026, Month::June, 25).unwrap();
        let after_release = Date::from_calendar_date(2026, Month::July, 10).unwrap();
        let usage = TokenUsage {
            input: 100_000,
            cached_input: 40_000,
            output: 10_000,
            reasoning_output: 5_000,
            total: 110_000,
            ..TokenUsage::default()
        };

        assert!(price_usage("gpt-5.6-sol", before_release, usage).is_none());
        assert!(price_usage("future-model", after_release, usage).is_none());
        assert_eq!(price_usage("gpt-5.6-sol", after_release, usage), Some(0.62));
    }

    #[test]
    fn pricing_uses_cache_write_and_long_context_modifiers() {
        let day = Date::from_calendar_date(2026, Month::July, 10).unwrap();
        let usage = TokenUsage {
            input: 300_000,
            cached_input: 100_000,
            cache_write_input: 50_000,
            output: 100_000,
            reasoning_output: 50_000,
            total: 400_000,
        };

        let cost = price_usage("gpt-5.6-terra", day, usage).unwrap();
        assert!((cost - 3.3625).abs() < f64::EPSILON);
        assert!(price_usage("gpt-5.5", day, usage).is_none());
    }

    #[test]
    fn rollout_scan_counts_root_cumulative_deltas_and_ignores_breakdowns() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":70,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":140,"cached_input_tokens":40,"cache_write_input_tokens":0,"output_tokens":60,"reasoning_output_tokens":20,"total_tokens":200},"model_context_window":1050000,"total_token_usage":{"input_tokens":210,"cached_input_tokens":60,"cache_write_input_tokens":0,"output_tokens":90,"reasoning_output_tokens":30,"total_tokens":300}},"rate_limits":null}}"#,
            "\n",
        );
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, &mut days));
        assert_eq!(days[&day].observed_tokens, 300);
        assert!(days[&day].complete);
        assert!(days[&day].api_equivalent_cost_usd.is_some());
    }

    #[test]
    fn rollout_scan_uses_the_first_fork_total_as_a_baseline() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0","forked_from_id":"private-parent"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}"#,
            "\n",
        );
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
        assert!(days[&day].complete);
    }

    #[test]
    fn rollout_scan_fails_closed_for_an_unknown_token_schema() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"new_total":100},"rate_limits":null}}"#,
            "\n",
        );
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(!scan_rollout_reader(fixture.as_bytes(), day, &mut days));
        assert!(!days[&day].complete);
        assert_eq!(days[&day].observed_tokens, 0);
    }

    #[test]
    fn account_days_are_authoritative_and_local_detail_is_never_added() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let day = now.date();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(day, 1_000)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                day,
                LocalUsageDay {
                    observed_tokens: 600,
                    api_equivalent_cost_usd: Some(1.0),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            evidence_basis,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 1_000);
        assert_eq!(api_equivalent_cost_usd, Some(5.0 / 3.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(60.0));
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
    }

    #[test]
    fn exact_local_reconciliation_supplies_complete_cost_for_account_tokens() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let day = now.date();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(day, 1_000)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                day,
                LocalUsageDay {
                    observed_tokens: 1_000,
                    api_equivalent_cost_usd: Some(1.25),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Complete,
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(api_equivalent_cost_usd, Some(1.25));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Reconciled)
        );
        assert_eq!(
            api_equivalent_cost_basis.as_deref(),
            Some(pricing_manifest().unwrap().basis.as_str())
        );
    }

    #[test]
    fn missing_account_days_remain_partial_and_do_not_invent_a_trend() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 1_000)]),
        };

        let projected = project_usage_periods(Some(&account), None, now);
        let UsageTotal::Current {
            coverage,
            observed_tokens,
            trend_percent,
            ..
        } = projected.seven_days
        else {
            panic!("expected current usage");
        };
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 1_000);
        assert_eq!(trend_percent, None);
    }

    #[test]
    fn complete_account_windows_get_equal_length_trends() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let mut daily_tokens = BTreeMap::new();
        for offset in 0..14 {
            daily_tokens.insert(
                now.date() - Duration::days(offset),
                if offset < 7 { 200 } else { 100 },
            );
        }
        let account = AccountUsageObservation { daily_tokens };

        let projected = project_usage_periods(Some(&account), None, now);
        let UsageTotal::Current {
            coverage,
            trend_percent,
            ..
        } = projected.seven_days
        else {
            panic!("expected current usage");
        };
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(trend_percent, Some(100.0));
    }

    #[test]
    fn local_rollouts_are_a_partial_fallback_only_when_account_usage_is_unavailable() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 600,
                    api_equivalent_cost_usd: Some(1.0),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
        };

        let projected = project_usage_periods(None, Some(&local), now);
        let UsageTotal::Current {
            coverage,
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_quality,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 600);
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::LocalOnly)
        );
    }

    #[test]
    fn local_tokens_above_account_or_after_account_observation_make_cost_unavailable() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100)]),
        };
        for local in [
            LocalUsageDay {
                observed_tokens: 101,
                api_equivalent_cost_usd: Some(1.0),
                complete: true,
                observed_through: Some(now - Duration::minutes(1)),
            },
            LocalUsageDay {
                observed_tokens: 80,
                api_equivalent_cost_usd: Some(1.0),
                complete: true,
                observed_through: Some(now + Duration::minutes(1)),
            },
        ] {
            let local = LocalUsageObservation {
                daily: BTreeMap::from([(now.date(), local)]),
                scan_status: UsageScanStatus::Indexing,
            };
            let projected = project_usage_periods(Some(&account), Some(&local), now);
            let UsageTotal::Current {
                observed_tokens,
                api_equivalent_cost_usd,
                ..
            } = projected.today
            else {
                panic!("expected account usage");
            };
            assert_eq!(observed_tokens, 100);
            assert_eq!(api_equivalent_cost_usd, None);
        }
    }

    #[test]
    fn period_uses_token_weighted_modeled_coverage() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let yesterday = now.date() - Duration::days(1);
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100), (yesterday, 900)]),
        };
        let detail = |tokens, cost| LocalUsageDay {
            observed_tokens: tokens,
            api_equivalent_cost_usd: Some(cost),
            complete: true,
            observed_through: Some(now - Duration::minutes(1)),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([
                (now.date(), detail(100, 1.0)),
                (yesterday, detail(450, 4.5)),
            ]),
            scan_status: UsageScanStatus::Indexing,
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = projected.seven_days
        else {
            panic!("expected account usage");
        };
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert!((api_equivalent_cost_coverage_percent.unwrap() - 55.0).abs() < 1e-9);
    }
}
