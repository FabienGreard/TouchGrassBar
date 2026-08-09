use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};

#[cfg(debug_assertions)]
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde::de::IgnoredAny;
use time::{
    Date, Duration, Month, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339,
};

use crate::daily_usage_aggregate::{
    DailyCostEvidence, DailyUsageEvidence, ProviderUsageEvidence, calculate_usage_periods,
    checked_sum, period_days,
};
use crate::sanitized::{
    ApiEquivalentCostQuality, TopModelUsage, UsageCoverage, UsagePeriods, UsageScanStatus,
    UsageTotal,
};

#[cfg(test)]
use crate::daily_usage_aggregate::preserve_best_known_costs;
#[cfg(test)]
use crate::sanitized::UsageEvidenceBasis;

const OPENAI_STANDARD_PRICING_JSON: &str = include_str!("../../../pricing/openai-standard.json");
const MAX_ROLLOUT_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROLLOUT_FILE_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ROLLOUT_SCAN_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ROLLOUT_SCAN_MILLIS: u128 = 2_000;
const PREFIX_ANCHOR_SAMPLE_BYTES: u64 = 1_024;
const LOCAL_USAGE_RETENTION_DAYS: i64 = 30;
const REPRICE_ROWS_PER_PASS: usize = 256;
const PRUNE_ROWS_PER_PASS: usize = 1_000;
const ROLLOUT_PARSER_VERSION: i64 = 8;
const UNKNOWN_MODEL: &str = "__unknown__";
pub(crate) const USAGE_INDEX_SCHEMA_MODULE: &str = "codex-usage-index";
pub(crate) const USAGE_INDEX_SCHEMA_VERSION: i64 = 3;

#[derive(Clone, Copy)]
struct ScanBudget {
    max_bytes: u64,
    max_file_bytes: u64,
    max_millis: u128,
}

const DEFAULT_SCAN_BUDGET: ScanBudget = ScanBudget {
    max_bytes: MAX_ROLLOUT_SCAN_BYTES,
    max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
    max_millis: MAX_ROLLOUT_SCAN_MILLIS,
};

#[cfg(debug_assertions)]
fn debug_usage_event(event: &str) {
    eprintln!("[TouchGrassBar][codex-usage] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_usage_event(_event: &str) {}

#[cfg(debug_assertions)]
fn debug_parser_failure(reason: &str, day: Option<Date>) {
    static REPORTED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let day = day.map_or_else(|| "unknown".to_owned(), |day| day.to_string());
    let key = format!("{reason}:{day}");
    let mut reported = REPORTED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if reported.insert(key) {
        eprintln!("[TouchGrassBar][codex-usage] parser_unavailable reason={reason} day={day}");
    }
}

#[cfg(not(debug_assertions))]
fn debug_parser_failure(_reason: &str, _day: Option<Date>) {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountUsageObservation {
    daily_tokens: BTreeMap<Date, u64>,
}

impl AccountUsageObservation {
    pub(crate) fn day_count(&self) -> usize {
        self.daily_tokens.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CachedAccountUsageObservation {
    pub(crate) observation: AccountUsageObservation,
    pub(crate) observed_at: OffsetDateTime,
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

pub(crate) fn load_cached_account_usage(
    database_path: Option<&Path>,
) -> Option<CachedAccountUsageObservation> {
    let database_path = database_path?;
    let mut connection = Connection::open(database_path).ok()?;
    ensure_index_schema(&mut connection, Some(database_path)).ok()?;
    let observed_at = connection
        .query_row(
            "SELECT observed_at FROM codex_account_usage_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()??;
    let observed_at = OffsetDateTime::parse(&observed_at, &Rfc3339).ok()?;
    let daily_tokens = connection
        .prepare("SELECT day, tokens FROM codex_account_usage_days ORDER BY day")
        .ok()?
        .query_map([], |row| {
            let day = parse_ranking_day(&row.get::<_, String>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let tokens = from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((day, tokens))
        })
        .ok()?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .ok()?;
    Some(CachedAccountUsageObservation {
        observation: AccountUsageObservation { daily_tokens },
        observed_at,
    })
}

pub(crate) fn store_cached_account_usage(
    database_path: Option<&Path>,
    observation: &AccountUsageObservation,
    observed_at: OffsetDateTime,
) -> Result<(), ()> {
    let database_path = database_path.ok_or(())?;
    let mut connection = Connection::open(database_path).map_err(|_| ())?;
    ensure_index_schema(&mut connection, Some(database_path))?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute("DELETE FROM codex_account_usage_days", [])
        .map_err(|_| ())?;
    for (day, tokens) in &observation.daily_tokens {
        transaction
            .execute(
                "INSERT INTO codex_account_usage_days(day, tokens) VALUES(?1, ?2)",
                params![day.to_string(), to_i64(*tokens)?],
            )
            .map_err(|_| ())?;
    }
    transaction
        .execute(
            "INSERT INTO codex_account_usage_meta(singleton, observed_at) VALUES(1, ?1)
             ON CONFLICT(singleton) DO UPDATE SET observed_at=excluded.observed_at",
            [observed_at.format(&Rfc3339).map_err(|_| ())?],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
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

impl PricingManifest {
    fn canonical_model_name(&self, model_name: &str) -> Option<&str> {
        self.models
            .iter()
            .find(|model| model.names.iter().any(|name| name == model_name))
            .and_then(|model| model.names.first())
            .map(String::as_str)
    }
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
    let fingerprint = pricing_manifest_fingerprint(&raw.basis, &models);
    Ok(PricingManifest {
        basis: raw.basis,
        fingerprint,
        models,
    })
}

fn pricing_manifest_fingerprint(basis: &str, models: &[PricedModel]) -> String {
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
    stable_pricing_fingerprint(&format!("{basis}||{}", model_parts.join("||")))
}

fn stable_pricing_fingerprint(canonical: &str) -> String {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PricingLookupFailure {
    MissingApplicablePrice,
    MissingCacheWritePrice,
    UnknownModel,
}

fn pricing_catalog_entry(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
) -> Result<PriceCatalogEntry, PricingLookupFailure> {
    let model = manifest
        .models
        .iter()
        .find(|entry| entry.names.iter().any(|name| name == model))
        .ok_or(PricingLookupFailure::UnknownModel)?;
    model
        .periods
        .iter()
        .copied()
        .find(|entry| entry.applies_to(day))
        .ok_or(PricingLookupFailure::MissingApplicablePrice)
}

#[cfg(test)]
fn catalog_entry(manifest: &PricingManifest, model: &str, day: Date) -> Option<PriceCatalogEntry> {
    pricing_catalog_entry(manifest, model, day).ok()
}

#[cfg(debug_assertions)]
fn debug_pricing_lookup_failure(model: &str, day: Date, failure: PricingLookupFailure) {
    static REPORTED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let reason = if model == UNKNOWN_MODEL {
        "model_not_observed"
    } else {
        match failure {
            PricingLookupFailure::MissingApplicablePrice => "missing_applicable_price",
            PricingLookupFailure::MissingCacheWritePrice => "missing_cache_write_price",
            PricingLookupFailure::UnknownModel => "unknown_model",
        }
    };
    let key = format!("{reason}:{model}");
    let mut reported = REPORTED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if reported.insert(key) {
        eprintln!(
            "[TouchGrassBar][codex-usage] pricing_unavailable reason={reason} model={model} day={day}"
        );
    }
}

#[cfg(not(debug_assertions))]
fn debug_pricing_lookup_failure(_model: &str, _day: Date, _failure: PricingLookupFailure) {}

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
    let entry = match pricing_catalog_entry(manifest, model, day) {
        Ok(entry) => entry,
        Err(failure) => {
            debug_pricing_lookup_failure(model, day, failure);
            return None;
        }
    };
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
        let Some(rate) = entry.cache_write_usd_per_million else {
            debug_pricing_lookup_failure(model, day, PricingLookupFailure::MissingCacheWritePrice);
            return None;
        };
        per_million(billable.cache_write_input, rate)
    };
    let cost = input_multiplier
        * (per_million(billable.standard_input, entry.input_usd_per_million)
            + per_million(billable.cached_input, entry.cached_input_usd_per_million)
            + cache_write)
        + output_multiplier * per_million(billable.output, entry.output_usd_per_million);
    cost.is_finite().then_some(cost)
}

fn pricing_rule_fingerprint(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
) -> String {
    let billable = match usage.billable() {
        Ok(billable) => billable,
        Err(()) => return stable_pricing_fingerprint("unavailable:invalid-token-arithmetic"),
    };
    let entry = match pricing_catalog_entry(manifest, model, day) {
        Ok(entry) => entry,
        Err(PricingLookupFailure::UnknownModel) => {
            return stable_pricing_fingerprint(&format!("unavailable:unknown-model:{model}"));
        }
        Err(PricingLookupFailure::MissingApplicablePrice) => {
            return stable_pricing_fingerprint(&format!(
                "unavailable:missing-applicable-price:{model}:{day}"
            ));
        }
        Err(PricingLookupFailure::MissingCacheWritePrice) => {
            return stable_pricing_fingerprint(&format!(
                "unavailable:missing-cache-write-price:{model}:{day}"
            ));
        }
    };
    if billable.cache_write_input > 0 && entry.cache_write_usd_per_million.is_none() {
        return stable_pricing_fingerprint(&format!(
            "unavailable:missing-cache-write-price:{model}:{day}"
        ));
    }
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
    let applicable_rate = |tokens: u64, rate: f64, multiplier: f64| {
        (tokens > 0).then(|| format!("{:016x}", (rate * multiplier).to_bits()))
    };
    stable_pricing_fingerprint(&format!(
        "priced:standard={}:cached={}:cache-write={}:output={}",
        applicable_rate(
            billable.standard_input,
            entry.input_usd_per_million,
            input_multiplier
        )
        .unwrap_or_else(|| "unused".to_owned()),
        applicable_rate(
            billable.cached_input,
            entry.cached_input_usd_per_million,
            input_multiplier
        )
        .unwrap_or_else(|| "unused".to_owned()),
        entry
            .cache_write_usd_per_million
            .and_then(|rate| applicable_rate(billable.cache_write_input, rate, input_multiplier))
            .unwrap_or_else(|| "unused".to_owned()),
        applicable_rate(
            billable.output,
            entry.output_usd_per_million,
            output_multiplier
        )
        .unwrap_or_else(|| "unused".to_owned()),
    ))
}

type LocalUsageDay = DailyCostEvidence;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalUsageObservation {
    daily: BTreeMap<Date, LocalUsageDay>,
    pub(crate) top_model_usage: Option<TopModelUsage>,
    pricing_basis: Option<String>,
    scan_status: UsageScanStatus,
    latest_pending_modified_at: Option<OffsetDateTime>,
    latest_error_modified_at: Option<OffsetDateTime>,
    scan_scope_known: bool,
}

fn period_scan_status(
    scan_status: UsageScanStatus,
    latest_pending_modified_at: Option<OffsetDateTime>,
    latest_error_modified_at: Option<OffsetDateTime>,
    period_start: OffsetDateTime,
    scan_scope_known: bool,
) -> UsageScanStatus {
    if !scan_scope_known {
        return scan_status;
    }
    if latest_pending_modified_at.is_some_and(|modified_at| modified_at >= period_start) {
        return UsageScanStatus::Indexing;
    }
    if latest_error_modified_at.is_some_and(|modified_at| modified_at >= period_start) {
        return UsageScanStatus::Unavailable;
    }
    UsageScanStatus::Complete
}

impl Default for LocalUsageObservation {
    fn default() -> Self {
        Self {
            daily: BTreeMap::new(),
            top_model_usage: None,
            pricing_basis: None,
            scan_status: UsageScanStatus::Unavailable,
            latest_pending_modified_at: None,
            latest_error_modified_at: None,
            scan_scope_known: false,
        }
    }
}

impl LocalUsageObservation {
    fn period_scan_status(&self, today: Date, length: i64) -> UsageScanStatus {
        let period_start = (today - Duration::days(length - 1)).midnight().assume_utc();
        period_scan_status(
            self.scan_status,
            self.latest_pending_modified_at,
            self.latest_error_modified_at,
            period_start,
            self.scan_scope_known,
        )
    }

    fn suppress_cost_evidence(&mut self) {
        for detail in self.daily.values_mut() {
            detail.priced_tokens = 0;
            detail.api_equivalent_cost_usd = None;
            detail.complete = false;
            detail.priced_observed_through = None;
        }
        self.pricing_basis = None;
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
    #[serde(default)]
    thread_source: Option<RawThreadSource>,
    #[serde(default)]
    source: Option<RawThreadSource>,
    #[serde(default)]
    subagent_history_start_ordinal: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawThreadSource {
    Name(String),
    Details(RawThreadSourceDetails),
    Other(IgnoredAny),
}

#[derive(Deserialize)]
struct RawThreadSourceDetails {
    #[serde(default)]
    subagent: Option<IgnoredAny>,
}

impl RawThreadSource {
    fn is_subagent(&self) -> bool {
        match self {
            Self::Name(name) => name == "subagent",
            Self::Details(details) => details.subagent.is_some(),
            Self::Other(value) => {
                let _ = value;
                false
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RolloutScanState {
    active_model: Option<String>,
    baseline_is_inherited: Option<bool>,
    history_start_ordinal: Option<u64>,
    record_ordinal: u64,
    exclude_usage: bool,
    previous: Option<TokenUsage>,
    schema_supported: bool,
}

fn apply_session_metadata(
    state: &mut RolloutScanState,
    metadata: RawSessionMeta,
) -> Result<(), ()> {
    state.schema_supported = is_supported_cli_version(&metadata.cli_version);
    if state.baseline_is_inherited.is_none() {
        let is_subagent = metadata
            .thread_source
            .as_ref()
            .into_iter()
            .chain(metadata.source.as_ref())
            .any(RawThreadSource::is_subagent);
        let is_inherited = metadata.forked_from_id.is_some() || is_subagent;
        state.baseline_is_inherited = Some(is_inherited);
        state.history_start_ordinal = metadata.subagent_history_start_ordinal;
        state.exclude_usage = is_inherited && state.history_start_ordinal.is_none();
    }
    // An unresolved inherited rollout is safe to exclude without reading its
    // version-specific token records.
    (state.schema_supported || state.exclude_usage)
        .then_some(())
        .ok_or(())
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
    (130..=147).contains(&minor)
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

fn oversized_record_is_ignorable(prefix: &[u8]) -> bool {
    const PREFIX_LIMIT: usize = 4 * 1024;
    const IGNORED_TYPES: [&[u8]; 2] = [b"\"type\":\"compacted\"", b"\"type\":\"response_item\""];
    let prefix = &prefix[..prefix.len().min(PREFIX_LIMIT)];
    IGNORED_TYPES.iter().any(|record_type| {
        prefix
            .windows(record_type.len())
            .any(|part| part == *record_type)
    })
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
    today: Date,
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
        let record_ordinal = state.record_ordinal;
        state.record_ordinal = state.record_ordinal.saturating_add(1);
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
        if day > today {
            continue;
        }
        let in_retention = day >= cutoff && day <= today;
        match header.record_type.as_str() {
            "session_meta" => {
                let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(&line) else {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                if apply_session_metadata(&mut state, line.payload).is_err() {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                }
            }
            "turn_context" => {
                let Ok(line) = serde_json::from_slice::<RawTurnContextLine>(&line) else {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                let context = line.payload;
                state.active_model = valid_model_name(&context.model).then_some(context.model);
                if in_retention && state.active_model.is_none() {
                    mark_incomplete(days, day);
                }
            }
            "event_msg" => {
                let Ok(line) = serde_json::from_slice::<RawEventLine>(&line) else {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
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
                    if in_retention {
                        mark_incomplete(days, day);
                    }
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
                        if in_retention {
                            mark_incomplete(days, day);
                        }
                        continue;
                    }
                };
                state.previous = Some(current);
                if !in_retention
                    || state.exclude_usage
                    || state
                        .history_start_ordinal
                        .is_some_and(|history_start| record_ordinal < history_start)
                {
                    continue;
                }
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
    parsed_prefix_anchor: Option<String>,
    completion_state: FileCompletionState,
    deferred_until_day: Option<Date>,
    parser_version: i64,
    parser_state: RolloutScanState,
}

#[derive(Clone, Debug)]
struct StoredFileSummary {
    identity: String,
    size: u64,
    modified_ns: i64,
    parsed_offset: u64,
    completion_state: FileCompletionState,
    deferred_until_day: Option<Date>,
    parser_version: i64,
}

impl StoredFileSummary {
    fn needs_work(&self, identity: &str, size: u64, modified_ns: i64, today: Date) -> bool {
        let position_is_settled = (self.parsed_offset == size
            && self.completion_state.is_terminal())
            || (self.parsed_offset < size
                && self.completion_state.is_deferred()
                && self.deferred_until_day.is_some_and(|day| day > today));
        self.parser_version != ROLLOUT_PARSER_VERSION
            || self.identity != identity
            || self.size != size
            || self.modified_ns != modified_ns
            || !position_is_settled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileCompletionState {
    Complete,
    Error,
    Indexing,
    DiscardingOverlongLine,
    Deferred,
    DeferredError,
    Unknown,
}

impl FileCompletionState {
    fn from_stored(value: &str) -> Self {
        match value {
            "complete" => Self::Complete,
            "error" => Self::Error,
            "indexing" => Self::Indexing,
            "discarding-overlong-line" => Self::DiscardingOverlongLine,
            "deferred" => Self::Deferred,
            "deferred-error" => Self::DeferredError,
            _ => Self::Unknown,
        }
    }

    fn as_stored(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Indexing => "indexing",
            Self::DiscardingOverlongLine => "discarding-overlong-line",
            Self::Deferred => "deferred",
            Self::DeferredError => "deferred-error",
            Self::Unknown => "unknown",
        }
    }

    fn has_parser_error(self) -> bool {
        matches!(self, Self::Error | Self::DeferredError)
    }

    fn is_deferred(self) -> bool {
        matches!(self, Self::Deferred | Self::DeferredError)
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Error)
    }
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

#[derive(Clone, Debug)]
struct FileDayDelta {
    observed_tokens: u64,
    priced_tokens: u64,
    cost_usd: f64,
    complete: bool,
    observed_through: OffsetDateTime,
    priced_observed_through: Option<OffsetDateTime>,
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
    today: Date,
    record_ordinal: u64,
    state: &mut RolloutScanState,
    rows: &mut BTreeMap<ModelDayKey, ModelDayDelta>,
) -> IndexLineOutcome {
    if line.len() > MAX_ROLLOUT_LINE_BYTES {
        debug_parser_failure("line_too_large", None);
        return IndexLineOutcome::Processed(false);
    }
    let header: RawRolloutHeader = match serde_json::from_slice(line) {
        Ok(header) => header,
        Err(_) => {
            debug_parser_failure("header_schema", None);
            return IndexLineOutcome::Processed(false);
        }
    };
    let timestamp = match parse_rollout_timestamp(&header.timestamp) {
        Ok(timestamp) => timestamp,
        Err(_) => {
            debug_parser_failure("timestamp", None);
            return IndexLineOutcome::Processed(false);
        }
    };
    let _ = header.payload;
    let day = utc_ranking_day(timestamp);
    if day > today {
        return IndexLineOutcome::DeferredUntil(day);
    }
    let in_retention = day >= cutoff && day <= today;
    let processed = match header.record_type.as_str() {
        "session_meta" => {
            let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(line) else {
                if in_retention {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                }
                debug_parser_failure("session_meta_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = (line.timestamp, line.record_type);
            if apply_session_metadata(state, line.payload).is_err() {
                if in_retention {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                }
                debug_parser_failure("session_metadata", in_retention.then_some(day));
            }
            state.schema_supported || state.exclude_usage
        }
        "turn_context" => {
            let Ok(line) = serde_json::from_slice::<RawTurnContextLine>(line) else {
                if in_retention {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                }
                debug_parser_failure("turn_context_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = (line.timestamp, line.record_type);
            state.active_model =
                valid_model_name(&line.payload.model).then_some(line.payload.model);
            if in_retention && state.active_model.is_none() {
                mark_model_day_incomplete(rows, timestamp, None);
                debug_parser_failure("model_name", Some(day));
                return IndexLineOutcome::Processed(false);
            }
            true
        }
        "event_msg" => {
            let Ok(line) = serde_json::from_slice::<RawEventLine>(line) else {
                if in_retention {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                }
                debug_parser_failure("event_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = (line.timestamp, line.record_type);
            let RawEventPayload::TokenCount { info, rate_limits } = line.payload else {
                return IndexLineOutcome::Processed(true);
            };
            let _ = (
                info.last_token_usage,
                info.model_context_window,
                rate_limits,
            );
            if !state.schema_supported {
                if in_retention {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                }
                debug_parser_failure("schema_not_initialized", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            }
            let current = info.total_token_usage;
            let delta = match state.previous {
                Some(previous) => current.delta_from(previous),
                None if state.baseline_is_inherited == Some(false) => {
                    current.delta_from(TokenUsage::default())
                }
                None if state.baseline_is_inherited == Some(true) => {
                    state.previous = Some(current);
                    return IndexLineOutcome::Processed(true);
                }
                None => {
                    state.previous = Some(current);
                    if in_retention {
                        mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                    }
                    debug_parser_failure("baseline", in_retention.then_some(day));
                    return IndexLineOutcome::Processed(false);
                }
            };
            state.previous = Some(current);
            if !in_retention
                || state.exclude_usage
                || state
                    .history_start_ordinal
                    .is_some_and(|history_start| record_ordinal < history_start)
            {
                return IndexLineOutcome::Processed(true);
            }
            match delta.and_then(|delta| {
                add_model_day_delta(rows, timestamp, state.active_model.as_deref(), delta)
            }) {
                Ok(()) => true,
                Err(()) => {
                    mark_model_day_incomplete(rows, timestamp, state.active_model.as_deref());
                    debug_parser_failure("token_arithmetic", Some(day));
                    false
                }
            }
        }
        _ => true,
    };
    IndexLineOutcome::Processed(processed)
}

enum IndexLineOutcome {
    Processed(bool),
    DeferredUntil(Date),
}

pub(crate) fn usage_index_schema_version(connection: &Connection) -> Result<i64, ()> {
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
            [USAGE_INDEX_SCHEMA_MODULE],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|version| version.unwrap_or(0))
        .map_err(|_| ())
}

fn usage_index_backup_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!("sqlite3.codex-usage-v{source_version}.backup"))
}

fn usage_index_backup_partial_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!(
        "sqlite3.codex-usage-v{source_version}.backup.partial"
    ))
}

fn usage_index_backup_is_valid(connection: &Connection, source_version: i64) -> Result<bool, ()> {
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    Ok(integrity == "ok" && usage_index_schema_version(connection)? == source_version)
}

fn backup_usage_index_before_migration(
    connection: &Connection,
    path: &Path,
    source_version: i64,
) -> Result<(), ()> {
    let backup_path = usage_index_backup_path(path, source_version);
    if backup_path.exists() {
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| ())?;
        return usage_index_backup_is_valid(&backup, source_version)?
            .then_some(())
            .ok_or(());
    }

    let partial_path = usage_index_backup_partial_path(path, source_version);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| ())?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &partial_path, None)
        .map_err(|_| ())?;
    let backup =
        Connection::open_with_flags(&partial_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| ())?;
    if !usage_index_backup_is_valid(&backup, source_version)? {
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

fn ensure_index_schema(
    connection: &mut Connection,
    database_path: Option<&Path>,
) -> Result<(), ()> {
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|_| ())?;
    let source_version = usage_index_schema_version(connection)?;
    if source_version > USAGE_INDEX_SCHEMA_VERSION {
        return Err(());
    }
    if source_version == USAGE_INDEX_SCHEMA_VERSION {
        return Ok(());
    }
    if let Some(database_path) = database_path {
        backup_usage_index_before_migration(connection, database_path, source_version)?;
    }

    let transaction = connection.transaction().map_err(|_| ())?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
               module TEXT PRIMARY KEY,
               version INTEGER NOT NULL CHECK (version >= 1)
             );
             CREATE TABLE IF NOT EXISTS codex_usage_index_meta (
               key TEXT PRIMARY KEY NOT NULL,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_account_usage_meta (
               singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
               observed_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_account_usage_days (
               day TEXT PRIMARY KEY NOT NULL,
               tokens INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_usage_files (
               path TEXT PRIMARY KEY NOT NULL,
               file_identity TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               modified_ns INTEGER NOT NULL,
               parsed_offset INTEGER NOT NULL,
               parsed_prefix_anchor TEXT,
               parser_version INTEGER NOT NULL,
               completion_state TEXT NOT NULL,
               deferred_until_day TEXT,
               active_model TEXT,
               baseline_is_inherited INTEGER,
               history_start_ordinal INTEGER,
               record_ordinal INTEGER NOT NULL DEFAULT 0,
               usage_excluded INTEGER NOT NULL DEFAULT 0,
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
             );
             CREATE TABLE IF NOT EXISTS codex_usage_file_days (
               path TEXT NOT NULL,
               day TEXT NOT NULL,
               observed_tokens INTEGER NOT NULL,
               priced_tokens INTEGER NOT NULL,
               cost_usd REAL NOT NULL,
               complete INTEGER NOT NULL,
               observed_through TEXT NOT NULL,
               priced_observed_through TEXT,
               pricing_fingerprint TEXT,
               PRIMARY KEY (path, day),
               FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
             );",
        )
        .map_err(|_| ())?;
    let file_columns = table_columns(&transaction, "codex_usage_files")?;
    if !file_columns
        .iter()
        .any(|column| column == "history_start_ordinal")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN history_start_ordinal INTEGER",
                [],
            )
            .map_err(|_| ())?;
    }
    if !file_columns.iter().any(|column| column == "record_ordinal") {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN record_ordinal INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|_| ())?;
    }
    if !file_columns.iter().any(|column| column == "usage_excluded") {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN usage_excluded INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|_| ())?;
    }
    if !file_columns
        .iter()
        .any(|column| column == "parsed_prefix_anchor")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN parsed_prefix_anchor TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
    if !file_columns
        .iter()
        .any(|column| column == "deferred_until_day")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN deferred_until_day TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
    let has_pricing_fingerprint = table_columns(&transaction, "codex_usage_file_model_days")?
        .iter()
        .any(|column| column == "pricing_fingerprint");
    if !has_pricing_fingerprint {
        transaction
            .execute(
                "ALTER TABLE codex_usage_file_model_days
                 ADD COLUMN pricing_fingerprint TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
    let file_day_columns = table_columns(&transaction, "codex_usage_file_days")?;
    if !file_day_columns
        .iter()
        .any(|column| column == "pricing_fingerprint")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_file_days ADD COLUMN pricing_fingerprint TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
    transaction
        .execute(
            "CREATE INDEX IF NOT EXISTS codex_usage_model_days_by_day
             ON codex_usage_file_model_days(day)",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "CREATE INDEX IF NOT EXISTS codex_usage_unpriced_model_days
             ON codex_usage_file_model_days(day, model, cache_write_input_tokens)
             WHERE cost_usd IS NULL",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_index_meta
             WHERE key IN (
               'file_day_summary_version',
               'pricing_complete_fingerprint',
               'pricing_reprice_cursor',
               'pricing_reprice_target_fingerprint'
             )",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO touchgrassbar_schema_versions(module, version) VALUES(?1, ?2)
             ON CONFLICT(module) DO UPDATE SET version = excluded.version",
            params![USAGE_INDEX_SCHEMA_MODULE, USAGE_INDEX_SCHEMA_VERSION],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

pub(crate) fn prepare_database(database_path: &Path) -> Result<(), ()> {
    let mut connection = Connection::open(database_path).map_err(|_| ())?;
    ensure_index_schema(&mut connection, Some(database_path))
}

fn to_i64(value: u64) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

fn from_i64(value: i64) -> Result<u64, ()> {
    u64::try_from(value).map_err(|_| ())
}

fn reprice_index(connection: &Connection, cutoff: Date, today: Date) -> Result<bool, ()> {
    reprice_index_batch_with_manifest(
        connection,
        pricing_manifest().ok_or(())?,
        cutoff,
        today,
        REPRICE_ROWS_PER_PASS,
    )
}

#[cfg(test)]
fn reprice_index_with_manifest(
    connection: &Connection,
    manifest: &PricingManifest,
    cutoff: Date,
    today: Date,
) -> Result<(), ()> {
    while !reprice_index_batch_with_manifest(
        connection,
        manifest,
        cutoff,
        today,
        REPRICE_ROWS_PER_PASS,
    )? {}
    Ok(())
}

fn rebuild_file_day_summary(connection: &Connection, path: &str, day: Date) -> Result<(), ()> {
    connection
        .execute(
            "DELETE FROM codex_usage_file_days WHERE path = ?1 AND day = ?2",
            params![path, day.to_string()],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO codex_usage_file_days(
               path, day, observed_tokens, priced_tokens, cost_usd, complete,
               observed_through, priced_observed_through, pricing_fingerprint
             )
             SELECT path, day, SUM(observed_tokens),
                    SUM(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN observed_tokens ELSE 0 END),
                    SUM(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN cost_usd ELSE 0.0 END),
                    MIN(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN 1 ELSE 0 END),
                    MAX(observed_through),
                    MAX(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN observed_through END),
                    NULL
             FROM codex_usage_file_model_days
             WHERE path = ?1 AND day = ?2
             GROUP BY path, day",
            params![path, day.to_string()],
        )
        .map_err(|_| ())?;
    Ok(())
}

fn ensure_file_day_summaries(connection: &Connection) -> Result<(), ()> {
    const SUMMARY_VERSION: &str = "2";
    let current_version = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta WHERE key = 'file_day_summary_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if current_version.as_deref() == Some(SUMMARY_VERSION) {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute("DELETE FROM codex_usage_file_days", [])
        .map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO codex_usage_file_days(
               path, day, observed_tokens, priced_tokens, cost_usd, complete,
               observed_through, priced_observed_through, pricing_fingerprint
             )
             SELECT path, day, SUM(observed_tokens),
                    SUM(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN observed_tokens ELSE 0 END),
                    SUM(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN cost_usd ELSE 0.0 END),
                    MIN(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN 1 ELSE 0 END),
                    MAX(observed_through),
                    MAX(CASE WHEN complete = 1 AND cost_usd IS NOT NULL
                             THEN observed_through END),
                    NULL
             FROM codex_usage_file_model_days GROUP BY path, day",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "INSERT INTO codex_usage_index_meta(key, value)
             VALUES('file_day_summary_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SUMMARY_VERSION],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

fn reprice_index_batch_with_manifest(
    connection: &Connection,
    manifest: &PricingManifest,
    cutoff: Date,
    today: Date,
    max_rows: usize,
) -> Result<bool, ()> {
    let completed_fingerprint = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta
             WHERE key = 'pricing_complete_fingerprint'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if completed_fingerprint.as_deref() == Some(manifest.fingerprint.as_str()) {
        return Ok(true);
    }
    if max_rows == 0 {
        return Err(());
    }
    let target_fingerprint = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta
             WHERE key = 'pricing_reprice_target_fingerprint'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    let cursor = if target_fingerprint.as_deref() == Some(manifest.fingerprint.as_str()) {
        connection
            .query_row(
                "SELECT value FROM codex_usage_index_meta WHERE key = 'pricing_reprice_cursor'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| ())?
            .map_or(Ok(0_i64), |value| value.parse::<i64>().map_err(|_| ()))?
    } else {
        0
    };
    let mut statement = connection
        .prepare(
            "SELECT rowid, path, day, model, pricing_input_tokens,
                    input_tokens, cached_input_tokens,
                    cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                    observed_tokens, pricing_basis, pricing_fingerprint
             FROM codex_usage_file_model_days
             WHERE rowid > ?1 AND day >= ?2 AND day <= ?3
             ORDER BY rowid
             LIMIT ?4",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                cursor,
                cutoff.to_string(),
                today.to_string(),
                i64::try_from(max_rows).map_err(|_| ())?
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    from_i64(row.get(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    TokenUsage {
                        input: from_i64(row.get(5)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cached_input: from_i64(row.get(6)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cache_write_input: from_i64(row.get(7)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        output: from_i64(row.get(8)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        reasoning_output: from_i64(row.get(9)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        total: from_i64(row.get(10)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    },
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    drop(statement);
    let batch_complete = rows.len() < max_rows;
    let next_cursor = rows.last().map_or(cursor, |row| row.0);
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    let mut affected_file_days = BTreeSet::new();
    for (_, path, day, model, pricing_input_tokens, usage, stored_basis, stored_rule_fingerprint) in
        rows
    {
        let day = parse_ranking_day(&day)?;
        let rule_fingerprint =
            pricing_rule_fingerprint(manifest, &model, day, usage, pricing_input_tokens);
        if stored_rule_fingerprint.as_deref() != Some(rule_fingerprint.as_str()) {
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
                        rule_fingerprint,
                        path.as_str(),
                        day.to_string(),
                        model.as_str(),
                        to_i64(pricing_input_tokens)?
                    ],
                )
                .map_err(|_| ())?;
            affected_file_days.insert((path.clone(), day));
        } else if stored_basis.as_deref() != Some(manifest.basis.as_str()) {
            transaction
                .execute(
                    "UPDATE codex_usage_file_model_days
                     SET pricing_basis = ?1
                     WHERE path = ?2 AND day = ?3 AND model = ?4 AND pricing_input_tokens = ?5",
                    params![
                        manifest.basis.as_str(),
                        path.as_str(),
                        day.to_string(),
                        model.as_str(),
                        to_i64(pricing_input_tokens)?
                    ],
                )
                .map_err(|_| ())?;
        }
    }
    for (path, day) in affected_file_days {
        rebuild_file_day_summary(&transaction, &path, day)?;
    }
    if batch_complete {
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
        transaction
            .execute(
                "INSERT INTO codex_usage_index_meta(key, value)
                 VALUES('pricing_complete_fingerprint', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [manifest.fingerprint.as_str()],
            )
            .map_err(|_| ())?;
        transaction
            .execute(
                "DELETE FROM codex_usage_index_meta
                 WHERE key IN ('pricing_reprice_cursor', 'pricing_reprice_target_fingerprint')",
                [],
            )
            .map_err(|_| ())?;
    } else {
        transaction
            .execute(
                "INSERT INTO codex_usage_index_meta(key, value)
                 VALUES('pricing_reprice_target_fingerprint', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [manifest.fingerprint.as_str()],
            )
            .map_err(|_| ())?;
        transaction
            .execute(
                "INSERT INTO codex_usage_index_meta(key, value)
                 VALUES('pricing_reprice_cursor', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [next_cursor.to_string()],
            )
            .map_err(|_| ())?;
    }
    transaction.commit().map_err(|_| ())?;
    Ok(batch_complete)
}

fn prune_expired_index(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    cutoff_modified_ns: i64,
) -> Result<bool, ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_model_days
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_model_days
               WHERE day < ?1 OR day > ?2 LIMIT ?3
             )",
            params![
                cutoff.to_string(),
                today.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let model_days_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_days
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_days
               WHERE day < ?1 OR day > ?2 LIMIT ?3
             )",
            params![
                cutoff.to_string(),
                today.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let file_days_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "DELETE FROM codex_usage_files
             WHERE path IN (
               SELECT f.path
               FROM codex_usage_files f
               WHERE f.modified_ns < ?1
                 AND NOT EXISTS (
                   SELECT 1 FROM codex_usage_file_model_days d
                   WHERE d.path = f.path AND d.day >= ?2 AND d.day <= ?3
                 )
               LIMIT ?4
             )",
            params![
                cutoff_modified_ns,
                cutoff.to_string(),
                today.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let files_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction.commit().map_err(|_| ())?;
    Ok(model_days_complete && file_days_complete && files_complete)
}

#[cfg(debug_assertions)]
fn debug_unpriced_model_days(connection: &Connection, cutoff: Date, today: Date) {
    static SCANNED: OnceLock<()> = OnceLock::new();
    if SCANNED.set(()).is_err() {
        return;
    }
    let Some(manifest) = pricing_manifest() else {
        return;
    };
    let Ok(mut statement) = connection.prepare(
        "SELECT DISTINCT model, day, cache_write_input_tokens > 0
         FROM codex_usage_file_model_days
         WHERE day >= ?1 AND day <= ?2 AND cost_usd IS NULL
         LIMIT 256",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map(params![cutoff.to_string(), today.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    }) else {
        return;
    };
    for row in rows.flatten() {
        let (model, day, has_cache_write) = row;
        let Ok(day) = parse_ranking_day(&day) else {
            continue;
        };
        if model == UNKNOWN_MODEL {
            continue;
        }
        match pricing_catalog_entry(manifest, &model, day) {
            Err(failure) => debug_pricing_lookup_failure(&model, day, failure),
            Ok(entry) if has_cache_write && entry.cache_write_usd_per_million.is_none() => {
                debug_pricing_lookup_failure(
                    &model,
                    day,
                    PricingLookupFailure::MissingCacheWritePrice,
                );
            }
            Ok(_) => {}
        }
    }
}

#[cfg(not(debug_assertions))]
fn debug_unpriced_model_days(_connection: &Connection, _cutoff: Date, _today: Date) {}

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

fn parsed_prefix_anchor(path: &Path, parsed_offset: u64) -> Result<Option<String>, ()> {
    if parsed_offset == 0 {
        return Ok(None);
    }
    let sample_length = PREFIX_ANCHOR_SAMPLE_BYTES.min(parsed_offset);
    let mut starts = BTreeSet::from([
        0,
        parsed_offset / 4,
        parsed_offset / 2,
        parsed_offset.saturating_mul(3) / 4,
        parsed_offset.saturating_sub(sample_length),
    ]);
    starts = starts
        .into_iter()
        .map(|start| start.min(parsed_offset.saturating_sub(sample_length)))
        .collect();
    let mut file = fs::File::open(path).map_err(|_| ())?;
    let mut hash = 0xcbf29ce484222325_u64;
    for start in starts {
        file.seek(SeekFrom::Start(start)).map_err(|_| ())?;
        let length = sample_length.min(parsed_offset.saturating_sub(start));
        let mut sample = vec![0; usize::try_from(length).map_err(|_| ())?];
        file.read_exact(&mut sample).map_err(|_| ())?;
        for byte in start
            .to_le_bytes()
            .into_iter()
            .chain(length.to_le_bytes())
            .chain(sample)
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(Some(format!("fnv1a64:{hash:016x}:{parsed_offset}")))
}

fn load_file_summaries(connection: &Connection) -> Result<BTreeMap<String, StoredFileSummary>, ()> {
    connection
        .prepare(
            "SELECT path, file_identity, size_bytes, modified_ns, parsed_offset,
                    completion_state, parser_version, deferred_until_day
             FROM codex_usage_files",
        )
        .map_err(|_| ())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                StoredFileSummary {
                    identity: row.get(1)?,
                    size: from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    modified_ns: row.get(3)?,
                    parsed_offset: from_i64(row.get(4)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    completion_state: FileCompletionState::from_stored(&row.get::<_, String>(5)?),
                    parser_version: row.get(6)?,
                    deferred_until_day: row
                        .get::<_, Option<String>>(7)?
                        .map(|value| parse_ranking_day(&value))
                        .transpose()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                },
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())
}

fn load_file_cursor(connection: &Connection, path: &str) -> Result<Option<FileCursor>, ()> {
    connection
        .query_row(
            "SELECT file_identity, size_bytes, modified_ns, parsed_offset, completion_state,
                    active_model, baseline_is_inherited, history_start_ordinal,
                    record_ordinal, usage_excluded, schema_supported, parser_version,
                    previous_input, previous_cached_input, previous_cache_write_input,
                    previous_output, previous_reasoning_output, previous_total,
                    parsed_prefix_anchor, deferred_until_day
             FROM codex_usage_files WHERE path = ?1",
            [path],
            |row| {
                let previous_total = row.get::<_, Option<i64>>(17)?;
                let previous = previous_total
                    .map(|total| {
                        Ok::<TokenUsage, rusqlite::Error>(TokenUsage {
                            input: from_i64(row.get(12)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cached_input: from_i64(row.get(13)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_write_input: from_i64(row.get(14)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            output: from_i64(row.get(15)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            reasoning_output: from_i64(row.get(16)?)
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
                    parsed_prefix_anchor: row.get(18)?,
                    completion_state: FileCompletionState::from_stored(&row.get::<_, String>(4)?),
                    deferred_until_day: row
                        .get::<_, Option<String>>(19)?
                        .map(|value| parse_ranking_day(&value))
                        .transpose()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    parser_version: row.get(11)?,
                    parser_state: RolloutScanState {
                        active_model: row.get(5)?,
                        baseline_is_inherited: row.get::<_, Option<bool>>(6)?,
                        history_start_ordinal: row
                            .get::<_, Option<i64>>(7)?
                            .map(from_i64)
                            .transpose()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        record_ordinal: from_i64(row.get(8)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        exclude_usage: row.get(9)?,
                        schema_supported: row.get(10)?,
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
               completion_state, active_model, baseline_is_inherited, history_start_ordinal,
               record_ordinal, usage_excluded, schema_supported,
               previous_input, previous_cached_input, previous_cache_write_input,
               previous_output, previous_reasoning_output, previous_total,
               parsed_prefix_anchor, deferred_until_day
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
             ON CONFLICT(path) DO UPDATE SET
               file_identity=excluded.file_identity, size_bytes=excluded.size_bytes,
               modified_ns=excluded.modified_ns, parsed_offset=excluded.parsed_offset,
               parser_version=excluded.parser_version, completion_state=excluded.completion_state,
               active_model=excluded.active_model,
               baseline_is_inherited=excluded.baseline_is_inherited,
               history_start_ordinal=excluded.history_start_ordinal,
               record_ordinal=excluded.record_ordinal,
               usage_excluded=excluded.usage_excluded,
               schema_supported=excluded.schema_supported, previous_input=excluded.previous_input,
               previous_cached_input=excluded.previous_cached_input,
               previous_cache_write_input=excluded.previous_cache_write_input,
               previous_output=excluded.previous_output,
               previous_reasoning_output=excluded.previous_reasoning_output,
               previous_total=excluded.previous_total,
               parsed_prefix_anchor=excluded.parsed_prefix_anchor,
               deferred_until_day=excluded.deferred_until_day",
            params![
                path,
                cursor.identity,
                to_i64(cursor.size)?,
                cursor.modified_ns,
                to_i64(cursor.parsed_offset)?,
                ROLLOUT_PARSER_VERSION,
                cursor.completion_state.as_stored(),
                cursor.parser_state.active_model,
                cursor.parser_state.baseline_is_inherited,
                cursor
                    .parser_state
                    .history_start_ordinal
                    .map(to_i64)
                    .transpose()
                    ?,
                to_i64(cursor.parser_state.record_ordinal)?,
                cursor.parser_state.exclude_usage,
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
                cursor.parsed_prefix_anchor,
                cursor.deferred_until_day.map(|day| day.to_string()),
            ],
        )
        .map_err(|_| ())?;
    let mut file_days = BTreeMap::<Date, FileDayDelta>::new();
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
        let pricing_fingerprint = manifest.map(|manifest| {
            pricing_rule_fingerprint(
                manifest,
                &key.model,
                key.day,
                delta.usage,
                key.pricing_input_tokens,
            )
        });
        let file_day = file_days.entry(key.day).or_insert(FileDayDelta {
            observed_tokens: 0,
            priced_tokens: 0,
            cost_usd: 0.0,
            complete: true,
            observed_through: delta.observed_through,
            priced_observed_through: None,
        });
        file_day.observed_tokens = file_day
            .observed_tokens
            .checked_add(delta.usage.total)
            .ok_or(())?;
        file_day.observed_through = file_day.observed_through.max(delta.observed_through);
        if delta.complete
            && let Some(cost) = cost
        {
            file_day.priced_tokens = file_day
                .priced_tokens
                .checked_add(delta.usage.total)
                .ok_or(())?;
            file_day.cost_usd += cost;
            file_day.priced_observed_through = Some(
                file_day
                    .priced_observed_through
                    .map_or(delta.observed_through, |current| {
                        current.max(delta.observed_through)
                    }),
            );
        } else {
            file_day.complete = false;
        }
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
                    pricing_fingerprint,
                    delta.complete,
                    delta.observed_through.format(&Rfc3339).map_err(|_| ())?,
                ],
            )
            .map_err(|_| ())?;
    }
    for (day, delta) in file_days {
        transaction
            .execute(
                "INSERT INTO codex_usage_file_days(
                   path, day, observed_tokens, priced_tokens, cost_usd, complete,
                   observed_through, priced_observed_through, pricing_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(path, day) DO UPDATE SET
                   observed_tokens=observed_tokens + excluded.observed_tokens,
                   priced_tokens=priced_tokens + excluded.priced_tokens,
                   cost_usd=cost_usd + excluded.cost_usd,
                   complete=complete AND excluded.complete,
                   observed_through=MAX(observed_through, excluded.observed_through),
                   priced_observed_through=CASE
                     WHEN priced_observed_through IS NULL THEN excluded.priced_observed_through
                     WHEN excluded.priced_observed_through IS NULL THEN priced_observed_through
                     ELSE MAX(priced_observed_through, excluded.priced_observed_through)
                   END,
                   pricing_fingerprint=excluded.pricing_fingerprint",
                params![
                    path,
                    day.to_string(),
                    to_i64(delta.observed_tokens)?,
                    to_i64(delta.priced_tokens)?,
                    delta.cost_usd,
                    delta.complete,
                    delta.observed_through.format(&Rfc3339).map_err(|_| ())?,
                    delta
                        .priced_observed_through
                        .map(|value| value.format(&Rfc3339))
                        .transpose()
                        .map_err(|_| ())?,
                    Option::<&str>::None,
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
    today: Date,
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
    let metadata_requires_rebuild = stored.as_ref().is_some_and(|cursor| {
        cursor.parser_version != ROLLOUT_PARSER_VERSION
            || cursor.identity != identity
            || size < cursor.size
            || size < cursor.parsed_offset
            || (size == cursor.size && modified_ns != cursor.modified_ns)
            || cursor.completion_state == FileCompletionState::Unknown
            || cursor.completion_state.is_deferred() != cursor.deferred_until_day.is_some()
    });
    let prefix_matches = if metadata_requires_rebuild {
        true
    } else if let Some(cursor) = &stored {
        parsed_prefix_anchor(path, cursor.parsed_offset)?.as_deref()
            == cursor.parsed_prefix_anchor.as_deref()
    } else {
        true
    };
    let rebuild = metadata_requires_rebuild || !prefix_matches;
    if rebuild {
        reset_file(connection, &path_value)?;
    }
    let stored = if rebuild { None } else { stored };
    if let Some(cursor) = &stored
        && cursor.parsed_offset == size
        && cursor.size == size
        && cursor.modified_ns == modified_ns
        && cursor.completion_state.is_terminal()
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
        parsed_prefix_anchor: None,
        completion_state: FileCompletionState::Indexing,
        deferred_until_day: None,
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
    let mut parser_complete = !cursor.completion_state.has_parser_error()
        && cursor.completion_state != FileCompletionState::DiscardingOverlongLine;
    let mut discarding_overlong_line =
        cursor.completion_state == FileCompletionState::DiscardingOverlongLine;
    let mut deferred_until_day = None;
    loop {
        if cursor.parser_state.exclude_usage {
            cursor.parsed_offset = size;
            break;
        }
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
                cursor.parser_state.record_ordinal =
                    cursor.parser_state.record_ordinal.saturating_add(1);
                if !oversized_record_is_ignorable(&line) {
                    parser_complete = false;
                    debug_parser_failure("line_too_large", None);
                }
                discarding_overlong_line = true;
                *remaining_bytes -= bytes;
                cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
                continue;
            }
            break;
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            *remaining_bytes -= bytes;
            cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
        } else {
            let record_ordinal = cursor.parser_state.record_ordinal;
            match process_index_line(
                &line,
                cutoff,
                today,
                record_ordinal,
                &mut cursor.parser_state,
                &mut rows,
            ) {
                IndexLineOutcome::Processed(processed) => {
                    cursor.parser_state.record_ordinal =
                        cursor.parser_state.record_ordinal.saturating_add(1);
                    parser_complete &= processed;
                    *remaining_bytes -= bytes;
                    cursor.parsed_offset = cursor.parsed_offset.checked_add(bytes).ok_or(())?;
                }
                IndexLineOutcome::DeferredUntil(day) => {
                    deferred_until_day = Some(day);
                    debug_usage_event(&format!("rollout_deferred until={day}"));
                    break;
                }
            }
        }
    }
    let is_deferred = deferred_until_day.is_some();
    cursor.completion_state = if deferred_until_day.is_some() {
        if parser_complete {
            FileCompletionState::Deferred
        } else {
            FileCompletionState::DeferredError
        }
    } else if discarding_overlong_line && cursor.parsed_offset < size {
        FileCompletionState::DiscardingOverlongLine
    } else if cursor.parsed_offset == size {
        if parser_complete {
            FileCompletionState::Complete
        } else {
            FileCompletionState::Error
        }
    } else {
        FileCompletionState::Indexing
    };
    cursor.deferred_until_day = deferred_until_day;
    cursor.parsed_prefix_anchor = parsed_prefix_anchor(path, cursor.parsed_offset)?;
    commit_file_progress(connection, &path_value, &cursor, rows)?;
    Ok(cursor.parsed_offset == size || is_deferred)
}

fn read_indexed_usage(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    scan_status: UsageScanStatus,
    scan_scope_known: bool,
    latest_pending_modified_hint: Option<OffsetDateTime>,
) -> Result<LocalUsageObservation, ()> {
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).map_err(|_| ())?;
    let mut statement = connection
        .prepare(
            "SELECT d.day, SUM(d.observed_tokens),
                    SUM(d.priced_tokens),
                    SUM(d.cost_usd),
                    MIN(CASE WHEN f.completion_state IN ('complete', 'deferred')
                                      AND d.complete = 1
                             THEN 1 ELSE 0 END),
                    MAX(d.observed_through),
                    MAX(d.priced_observed_through)
             FROM codex_usage_file_days d
             JOIN codex_usage_files f ON f.path = d.path
             WHERE f.parser_version = ?1 AND d.day >= ?2 AND d.day <= ?3
             GROUP BY d.day ORDER BY d.day",
        )
        .map_err(|_| ())?;
    let mut rows = statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| {
                let day = parse_ranking_day(&row.get::<_, String>(0)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let observed_tokens =
                    from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let priced_tokens =
                    from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
                let cost = (priced_tokens > 0)
                    .then(|| row.get::<_, f64>(3))
                    .transpose()?;
                let complete = row.get::<_, bool>(4)?;
                let observed_through = OffsetDateTime::parse(&row.get::<_, String>(5)?, &Rfc3339)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let priced_observed_through = row
                    .get::<_, Option<String>>(6)?
                    .map(|value| OffsetDateTime::parse(&value, &Rfc3339))
                    .transpose()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((
                    day,
                    LocalUsageDay {
                        observed_tokens,
                        priced_tokens,
                        api_equivalent_cost_usd: cost,
                        complete,
                        observed_through: Some(observed_through),
                        priced_observed_through,
                    },
                ))
            },
        )
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())?;
    let (latest_pending_modified_ns, latest_error_modified_ns, has_excluded_files) = connection
        .query_row(
            "SELECT
               MAX(CASE WHEN completion_state NOT IN (
                                'complete', 'error', 'deferred', 'deferred-error'
                              )
                        THEN modified_ns END),
               MAX(CASE WHEN completion_state IN ('error', 'deferred-error')
                        THEN modified_ns END),
               COALESCE(MAX(usage_excluded), 0)
             FROM codex_usage_files
             WHERE modified_ns >= ?1",
            [cutoff_modified_ns],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .map_err(|_| ())?;
    if has_excluded_files {
        for detail in rows.values_mut() {
            detail.complete = false;
        }
    }
    let parse_modified_at = |value: Option<i64>| {
        value
            .map(|value| OffsetDateTime::from_unix_timestamp_nanos(i128::from(value)))
            .transpose()
            .map_err(|_| ())
    };
    let pricing_basis = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta WHERE key = 'pricing_basis'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    let top_model_usage = read_top_model_usage(connection, cutoff, today)?;
    Ok(LocalUsageObservation {
        daily: rows,
        top_model_usage,
        pricing_basis,
        scan_status,
        latest_pending_modified_at: parse_modified_at(latest_pending_modified_ns)?
            .into_iter()
            .chain(latest_pending_modified_hint)
            .max(),
        latest_error_modified_at: parse_modified_at(latest_error_modified_ns)?,
        scan_scope_known,
    })
}

fn read_top_model_usage(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<Option<TopModelUsage>, ()> {
    let manifest = pricing_manifest();
    let mut statement = connection
        .prepare(
            "SELECT d.model, SUM(d.observed_tokens)
             FROM codex_usage_file_model_days d
             JOIN codex_usage_files f ON f.path = d.path
             WHERE f.parser_version = ?1 AND d.day >= ?2 AND d.day <= ?3
             GROUP BY d.model",
        )
        .map_err(|_| ())?;
    let entries = statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|_| ())?
        .map(|row| {
            let (model, observed_tokens) = row.map_err(|_| ())?;
            let display_name = manifest
                .and_then(|manifest| manifest.canonical_model_name(&model))
                .and_then(crate::providers::normalized_model_display_name);
            let grouping_key = display_name.clone().unwrap_or_else(|| model.clone());
            Ok((
                grouping_key,
                display_name,
                from_i64(observed_tokens).map_err(|_| ())?,
            ))
        })
        .collect::<Result<Vec<_>, ()>>()?;
    Ok(crate::providers::select_top_model_usage(entries))
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
    let max_bytes = budget.max_bytes.min(MAX_ROLLOUT_SCAN_BYTES);
    let max_file_bytes = budget.max_file_bytes.min(MAX_ROLLOUT_FILE_SCAN_BYTES);
    let max_millis = budget.max_millis.min(MAX_ROLLOUT_SCAN_MILLIS);
    debug_usage_event(&format!(
        "scan_pass_started max_bytes={max_bytes} max_file_bytes={max_file_bytes} max_millis={max_millis}"
    ));
    let mut connection = Connection::open(database_path).ok()?;
    ensure_index_schema(&mut connection, Some(database_path)).ok()?;
    let today = utc_ranking_day(now);
    let cutoff = today - Duration::days(LOCAL_USAGE_RETENTION_DAYS - 1);
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).ok()?;
    let retention_complete =
        prune_expired_index(&connection, cutoff, today, cutoff_modified_ns).ok()?;
    let pricing_complete = retention_complete && reprice_index(&connection, cutoff, today).ok()?;
    let summaries_complete = pricing_complete && ensure_file_day_summaries(&connection).is_ok();
    if !retention_complete || !pricing_complete || !summaries_complete {
        debug_usage_event(&format!(
            "scan_pass_completed stop=maintenance elapsed_ms={} retention_complete={retention_complete} pricing_complete={pricing_complete} summaries_complete={summaries_complete}",
            started.elapsed().as_millis()
        ));
        let mut local = read_indexed_usage(
            &connection,
            cutoff,
            today,
            UsageScanStatus::Indexing,
            false,
            None,
        )
        .ok()?;
        if !pricing_complete {
            local.suppress_cost_evidence();
        }
        return Some(local);
    }
    let mut files = Vec::new();
    let mut found_root = false;
    let mut traversal_complete = true;
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
    let stored_files = load_file_summaries(&connection).ok()?;
    let mut ordered_files = Vec::with_capacity(files.len());
    for path in files {
        if started.elapsed().as_millis() >= max_millis {
            traversal_complete = false;
            break;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            traversal_complete = false;
            continue;
        };
        let Ok(modified_ns) = file_modified_ns(&metadata) else {
            traversal_complete = false;
            continue;
        };
        let identity = file_identity(&metadata);
        let path_value = path.to_string_lossy().into_owned();
        let needs_work = modified_ns >= cutoff_modified_ns
            && stored_files.get(&path_value).is_none_or(|stored| {
                stored.needs_work(&identity, metadata.len(), modified_ns, today)
            });
        ordered_files.push((needs_work, identity, modified_ns, metadata.len(), path));
    }
    ordered_files.sort_by_key(|entry| (!entry.0, std::cmp::Reverse(entry.2)));
    let present = ordered_files
        .iter()
        .map(|(_, _, _, _, path)| path.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    if traversal_complete {
        for missing in stored_files.keys().filter(|path| !present.contains(*path)) {
            reset_file(&connection, missing).ok()?;
        }
    }
    let files = ordered_files
        .into_iter()
        .filter(|(needs_work, _, _, _, _)| *needs_work)
        .collect::<Vec<_>>();
    let mut remaining_bytes = max_bytes;
    let mut all_complete = traversal_complete;
    let mut failed = false;
    let mut visited_files = 0_u64;
    let mut completed_files = 0_u64;
    for (_, _, _, _, path) in &files {
        if started.elapsed().as_millis() >= max_millis {
            all_complete = false;
            break;
        }
        let file_allowance = remaining_bytes.min(max_file_bytes);
        if file_allowance == 0 {
            all_complete = false;
            break;
        }
        let mut file_remaining_bytes = file_allowance;
        visited_files = visited_files.saturating_add(1);
        match index_file(
            &connection,
            path,
            cutoff,
            today,
            started,
            max_millis,
            &mut file_remaining_bytes,
        ) {
            Ok(complete) => {
                all_complete &= complete;
                completed_files = completed_files.saturating_add(u64::from(complete));
            }
            Err(()) => {
                failed = true;
                all_complete = false;
            }
        }
        remaining_bytes = remaining_bytes.saturating_sub(file_allowance - file_remaining_bytes);
        if remaining_bytes == 0 {
            all_complete = false;
            break;
        }
    }
    let (error_files, pending_files, excluded_files) = connection
        .query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN completion_state IN ('error', 'deferred-error')
                                 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state NOT IN (
                                          'complete', 'error', 'deferred', 'deferred-error'
                                        )
                                 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN usage_excluded = 1 THEN 1 ELSE 0 END), 0)
             FROM codex_usage_files WHERE parser_version = ?1 AND modified_ns >= ?2",
            params![ROLLOUT_PARSER_VERSION, cutoff_modified_ns],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .unwrap_or((1, 1, 0));
    let scan_status = if failed {
        UsageScanStatus::Unavailable
    } else if !all_complete {
        UsageScanStatus::Indexing
    } else if error_files > 0 {
        UsageScanStatus::Unavailable
    } else {
        UsageScanStatus::Complete
    };
    let stop = if failed {
        "error"
    } else if scan_status == UsageScanStatus::Complete {
        "complete"
    } else if pending_files == 0 && error_files > 0 {
        "parser_errors"
    } else if started.elapsed().as_millis() >= max_millis {
        "time"
    } else if remaining_bytes == 0 {
        "bytes"
    } else {
        "pending"
    };
    debug_usage_event(&format!(
        "scan_pass_completed stop={stop} bytes_read={} elapsed_ms={} visited_files={visited_files} completed_files={completed_files} pending_files={pending_files} error_files={error_files} excluded_inherited_files={excluded_files} traversal_complete={traversal_complete}",
        max_bytes.saturating_sub(remaining_bytes),
        started.elapsed().as_millis()
    ));
    debug_unpriced_model_days(&connection, cutoff, today);
    let indexed_files = load_file_summaries(&connection).ok()?;
    let latest_pending_modified_at = files
        .iter()
        .filter(|(_, identity, modified_ns, size, path)| {
            let path_value = path.to_string_lossy();
            indexed_files
                .get(path_value.as_ref())
                .is_none_or(|stored| stored.needs_work(identity, *size, *modified_ns, today))
        })
        .filter_map(|(_, _, modified_ns, _, _)| {
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(*modified_ns)).ok()
        })
        .max();
    read_indexed_usage(
        &connection,
        cutoff,
        today,
        scan_status,
        traversal_complete && !failed,
        latest_pending_modified_at,
    )
    .ok()
}

pub(crate) fn scan_local_usage(
    database_path: Option<&Path>,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    index_local_usage_at(database_path?, &codex_data_home()?, now)
}

fn debug_cost_quality(quality: Option<ApiEquivalentCostQuality>) -> &'static str {
    match quality {
        Some(ApiEquivalentCostQuality::Reconciled) => "reconciled",
        Some(ApiEquivalentCostQuality::Modeled) => "modeled",
        Some(ApiEquivalentCostQuality::LocalOnly) => "local-only",
        None => "unavailable",
    }
}

fn debug_period_projection(
    total: &UsageTotal,
) -> (
    Option<u64>,
    Option<f64>,
    &'static str,
    Option<f64>,
    Option<f64>,
) {
    match total {
        UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            trend_percent,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        }
        | UsageTotal::Stale {
            observed_tokens,
            api_equivalent_cost_usd,
            trend_percent,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            Some(*observed_tokens),
            *api_equivalent_cost_usd,
            debug_cost_quality(*api_equivalent_cost_quality),
            *api_equivalent_cost_coverage_percent,
            *trend_percent,
        ),
        UsageTotal::Unavailable => (None, None, "unavailable", None, None),
    }
}

fn debug_period_line(
    label: &str,
    length: i64,
    today: Date,
    account: Option<&AccountUsageObservation>,
    local: &LocalUsageObservation,
    projected: &UsageTotal,
) -> String {
    let days = period_days(today, length, 0).collect::<Vec<_>>();
    let account_tokens = account.and_then(|account| {
        checked_sum(days.iter().filter_map(|day| account.daily_tokens.get(day)))
    });
    let local_tokens = checked_sum(
        days.iter()
            .filter_map(|day| local.daily.get(day).map(|detail| &detail.observed_tokens)),
    );
    let priced_tokens = checked_sum(
        days.iter()
            .filter_map(|day| local.daily.get(day).map(|detail| &detail.priced_tokens)),
    );
    let relation = match (account_tokens, local_tokens) {
        (Some(account), Some(local)) if local > account => "local-above-account",
        (Some(account), Some(local)) if local < account => "local-below-account",
        (Some(_), Some(_)) => "equal",
        (None, Some(_)) => "account-unavailable",
        (Some(_), None) => "local-unavailable",
        (None, None) => "unavailable",
    };
    let (observed_tokens, cost, quality, coverage, trend) = debug_period_projection(projected);
    format!(
        "[TouchGrassBar][codex-usage-report] period={label} account_tokens={} local_detail_tokens={} priced_local_tokens={} relation={relation} authoritative_tokens={} projected_cost_usd={} quality={quality} coverage_percent={} trend_percent={}",
        account_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        local_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        priced_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        observed_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        cost.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.6}")),
        coverage.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.2}")),
        trend.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.2}")),
    )
}

fn debug_catalog_description(
    manifest: Option<&PricingManifest>,
    model: &str,
    day: Date,
    has_cache_write: bool,
) -> String {
    if model == UNKNOWN_MODEL {
        return "status=model-not-observed".to_owned();
    }
    let Some(manifest) = manifest else {
        return "status=manifest-unavailable".to_owned();
    };
    match pricing_catalog_entry(manifest, model, day) {
        Err(PricingLookupFailure::UnknownModel) => "status=unknown-model".to_owned(),
        Err(PricingLookupFailure::MissingApplicablePrice) => {
            "status=missing-effective-price".to_owned()
        }
        Err(PricingLookupFailure::MissingCacheWritePrice) => {
            "status=missing-cache-write-price".to_owned()
        }
        Ok(entry) if has_cache_write && entry.cache_write_usd_per_million.is_none() => {
            "status=missing-cache-write-price".to_owned()
        }
        Ok(entry) => format!(
            "status=known input_usd_per_million={:.6} cached_input_usd_per_million={:.6} cache_write_usd_per_million={} output_usd_per_million={:.6} effective_from={} effective_until={} long_context_input_above={} long_context_input_multiplier={:.6} long_context_output_multiplier={:.6}",
            entry.input_usd_per_million,
            entry.cached_input_usd_per_million,
            entry
                .cache_write_usd_per_million
                .map_or_else(|| "not-published".to_owned(), |value| format!("{value:.6}")),
            entry.output_usd_per_million,
            entry.effective_from,
            entry
                .effective_until
                .map_or_else(|| "open".to_owned(), |value| value.to_string()),
            entry.long_context_input_tokens_above,
            entry.long_context_input_multiplier,
            entry.long_context_output_multiplier,
        ),
    }
}

fn render_debug_usage_report(
    connection: &Connection,
    account: Option<&CachedAccountUsageObservation>,
    local: &LocalUsageObservation,
    periods: &UsagePeriods,
    today: Date,
) -> Result<String, ()> {
    let cutoff = today - Duration::days(LOCAL_USAGE_RETENTION_DAYS - 1);
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).map_err(|_| ())?;
    let (complete_files, deferred_files, pending_files, error_files, excluded_files) = connection
        .query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN completion_state = 'complete' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state IN ('deferred', 'deferred-error')
                                 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state NOT IN (
                                          'complete', 'error', 'deferred', 'deferred-error'
                                        )
                                 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state IN ('error', 'deferred-error')
                                 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN usage_excluded = 1 THEN 1 ELSE 0 END), 0)
             FROM codex_usage_files WHERE parser_version = ?1 AND modified_ns >= ?2",
            params![ROLLOUT_PARSER_VERSION, cutoff_modified_ns],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .map_err(|_| ())?;
    let manifest = pricing_manifest();
    let mut lines = vec![format!(
        "[TouchGrassBar][codex-usage-report] retention_days={} pricing_basis={} account_observed_at={} scan={:?} today_scan={:?} seven_day_scan={:?} thirty_day_scan={:?} complete_files={complete_files} deferred_files={deferred_files} pending_files={pending_files} error_files={error_files} excluded_inherited_files={excluded_files}",
        LOCAL_USAGE_RETENTION_DAYS,
        manifest.map_or("unavailable", |manifest| manifest.basis.as_str()),
        account.map_or_else(
            || "unavailable".to_owned(),
            |account| account
                .observed_at
                .format(&Rfc3339)
                .unwrap_or_else(|_| "unavailable".to_owned())
        ),
        periods.scan_status,
        periods.today_scan_status,
        periods.seven_day_scan_status,
        periods.thirty_day_scan_status,
    )];
    let account_observation = account.map(|account| &account.observation);
    lines.push(debug_period_line(
        "today",
        1,
        today,
        account_observation,
        local,
        &periods.today,
    ));
    lines.push(debug_period_line(
        "7-day",
        7,
        today,
        account_observation,
        local,
        &periods.seven_days,
    ));
    lines.push(debug_period_line(
        "30-day",
        30,
        today,
        account_observation,
        local,
        &periods.thirty_days,
    ));

    let mut statement = connection
        .prepare(
            "SELECT d.day, d.model,
                    SUM(d.input_tokens), SUM(d.cached_input_tokens),
                    SUM(d.cache_write_input_tokens), SUM(d.output_tokens),
                    SUM(d.reasoning_output_tokens), SUM(d.observed_tokens),
                    SUM(CASE WHEN d.complete = 1 AND d.cost_usd IS NOT NULL
                             THEN d.observed_tokens ELSE 0 END),
                    SUM(CASE WHEN d.complete = 1 AND d.cost_usd IS NOT NULL
                             THEN d.cost_usd ELSE 0.0 END),
                    MIN(d.complete)
             FROM codex_usage_file_model_days d
             JOIN codex_usage_files f ON f.path = d.path
             WHERE f.parser_version = ?1 AND d.day >= ?2 AND d.day <= ?3
             GROUP BY d.day, d.model
             ORDER BY d.day DESC, SUM(d.observed_tokens) DESC",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, u64>(7)?,
                    row.get::<_, u64>(8)?,
                    row.get::<_, f64>(9)?,
                    row.get::<_, bool>(10)?,
                ))
            },
        )
        .map_err(|_| ())?;
    for row in rows {
        let (
            day,
            model,
            input,
            cached_input,
            cache_write_input,
            output,
            reasoning_output,
            observed,
            priced,
            cost,
            complete,
        ) = row.map_err(|_| ())?;
        let day = parse_ranking_day(&day)?;
        let catalog = debug_catalog_description(manifest, &model, day, cache_write_input > 0);
        lines.push(format!(
            "[TouchGrassBar][codex-usage-report] day={day} model={model} observed_tokens={observed} input_tokens={input} cached_input_subset={cached_input} cache_write_input_subset={cache_write_input} output_tokens={output} reasoning_output_subset={reasoning_output} priced_tokens={priced} local_cost_usd={:.6} detail_complete={complete} catalog_{catalog}",
            cost
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn debug_usage_pass(
    database_path: &Path,
    codex_home: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    let local = index_local_usage_at(database_path, codex_home, now).ok_or(())?;
    let account = load_cached_account_usage(Some(database_path));
    let periods = project_usage_periods_with_account_time(
        account.as_ref().map(|cached| &cached.observation),
        Some(&local),
        now,
        account.as_ref().map_or(now, |cached| cached.observed_at),
    );
    let cost_available = |total: &UsageTotal| {
        matches!(
            total,
            UsageTotal::Current {
                api_equivalent_cost_usd: Some(_),
                ..
            } | UsageTotal::Stale {
                api_equivalent_cost_usd: Some(_),
                ..
            }
        )
    };
    debug_usage_event(&format!(
        "projection_updated scan={:?} today_scan={:?} seven_day_scan={:?} thirty_day_scan={:?} account_cached={} today_cost={} seven_day_cost={} thirty_day_cost={}",
        periods.scan_status,
        periods.today_scan_status,
        periods.seven_day_scan_status,
        periods.thirty_day_scan_status,
        account.is_some(),
        cost_available(&periods.today),
        cost_available(&periods.seven_days),
        cost_available(&periods.thirty_days)
    ));
    let connection = Connection::open(database_path).map_err(|_| ())?;
    render_debug_usage_report(
        &connection,
        account.as_ref(),
        &local,
        &periods,
        utc_ranking_day(now),
    )
}

#[cfg(test)]
pub(crate) fn project_usage_periods(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
) -> UsagePeriods {
    project_usage_periods_with_account_time(account, local, now, now)
}

pub(crate) fn project_usage_periods_with_account_time(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
    account_observed_at: OffsetDateTime,
) -> UsagePeriods {
    let today = utc_ranking_day(now);
    let evidence = ProviderUsageEvidence {
        provider_reported_tokens: account.map(|account| account.daily_tokens.clone()),
        provider_observed_at: account.map(|_| account_observed_at),
        local_usage_evidence: local.map_or_else(BTreeMap::new, |local| {
            local
                .daily
                .iter()
                .map(|(day, detail)| {
                    (
                        *day,
                        DailyUsageEvidence {
                            observed_tokens: detail.observed_tokens,
                            coverage: UsageCoverage::Partial,
                            observed_through: detail.observed_through,
                        },
                    )
                })
                .collect()
        }),
        local_cost_evidence: local.map_or_else(BTreeMap::new, |local| local.daily.clone()),
        local_evidence_available: local.is_some(),
        local_observed_at: local.map(|_| now),
        pricing_basis: local
            .and_then(|local| local.pricing_basis.clone())
            .or_else(|| pricing_manifest().map(|manifest| manifest.basis.clone())),
        scan_status: local.map_or(UsageScanStatus::Unavailable, |local| local.scan_status),
        today_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 1)
        }),
        seven_day_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 7)
        }),
        thirty_day_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 30)
        }),
    };
    calculate_usage_periods(&evidence, now)
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
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempUsage {
        root: PathBuf,
        database: PathBuf,
        rollout: PathBuf,
    }

    impl TempUsage {
        fn new() -> Self {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
            let root = env::temp_dir().join(format!(
                "touchgrassbar-codex-usage-{}-{}-{}",
                std::process::id(),
                OffsetDateTime::now_utc().unix_timestamp_nanos(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
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
    fn sqlite_account_usage_cache_round_trips_private_daily_buckets() {
        let fixture = TempUsage::new();
        let observed_at = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let observation = AccountUsageObservation {
            daily_tokens: BTreeMap::from([
                (observed_at.date() - Duration::days(1), 120),
                (observed_at.date(), 340),
            ]),
        };

        store_cached_account_usage(Some(&fixture.database), &observation, observed_at).unwrap();

        assert_eq!(
            load_cached_account_usage(Some(&fixture.database)),
            Some(CachedAccountUsageObservation {
                observation,
                observed_at,
            })
        );
    }

    #[test]
    fn usage_index_migration_is_versioned_transactional_and_backed_up() {
        let fixture = TempUsage::new();
        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL
                 );
                 INSERT INTO touchgrassbar_schema_versions(module, version)
                 VALUES('codex-usage-index', 1);
                 CREATE TABLE codex_usage_index_meta (
                   key TEXT PRIMARY KEY NOT NULL,
                   value TEXT NOT NULL
                 );
                 CREATE TABLE codex_usage_files (
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
                 CREATE TABLE codex_usage_file_model_days (
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
                   complete INTEGER NOT NULL,
                   observed_through TEXT NOT NULL,
                   PRIMARY KEY (path, day, model, pricing_input_tokens),
                   FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
                 );
                 INSERT INTO codex_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   parser_version, completion_state, schema_supported
                 ) VALUES('private-rollout', '1:2', 10, 20, 10, 6, 'complete', 1);",
            )
            .unwrap();
        drop(connection);

        let mut connection = Connection::open(&fixture.database).unwrap();
        ensure_index_schema(&mut connection, Some(&fixture.database)).unwrap();

        assert_eq!(
            usage_index_schema_version(&connection).unwrap(),
            USAGE_INDEX_SCHEMA_VERSION
        );
        let file_columns = table_columns(&connection, "codex_usage_files").unwrap();
        for required in [
            "history_start_ordinal",
            "record_ordinal",
            "usage_excluded",
            "parsed_prefix_anchor",
            "deferred_until_day",
        ] {
            assert!(file_columns.iter().any(|column| column == required));
        }
        assert!(
            table_columns(&connection, "codex_usage_file_model_days")
                .unwrap()
                .iter()
                .any(|column| column == "pricing_fingerprint")
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM codex_usage_files WHERE path = 'private-rollout'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        let backup_path = usage_index_backup_path(&fixture.database, 1);
        assert!(backup_path.is_file());
        assert!(!usage_index_backup_partial_path(&fixture.database, 0).exists());
        let backup =
            Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(usage_index_schema_version(&backup).unwrap(), 1);
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM codex_usage_files", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
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
        let connection = Connection::open(&fixture.database).unwrap();
        let unchanged_cursor: (i64, i64, i64, String, i64) = connection
            .query_row(
                "SELECT size_bytes, modified_ns, parsed_offset, completion_state, parser_version FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        let unchanged_tokens: i64 = connection
            .query_row(
                "SELECT SUM(observed_tokens) FROM codex_usage_file_model_days",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unchanged_cursor, first_cursor);
        assert_eq!(unchanged_tokens, 100);
        drop(connection);
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
    fn sqlite_index_defers_a_future_record_and_resumes_it_on_its_utc_day() {
        let fixture = TempUsage::new();
        let future = json!({
            "timestamp": "2026-08-07T10:01:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 70,
                        "cached_input_tokens": 0,
                        "output_tokens": 30,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 100
                    },
                    "model_context_window": 1_050_000,
                    "total_token_usage": {
                        "input_tokens": 140,
                        "cached_input_tokens": 0,
                        "output_tokens": 60,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 200
                    }
                },
                "rate_limits": null
            }
        });
        fs::write(&fixture.rollout, format!("{}{future}\n", root_rollout(100))).unwrap();
        let first_now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = index_local_usage_at(&fixture.database, &fixture.root, first_now).unwrap();
        assert_eq!(first.scan_status, UsageScanStatus::Complete);
        assert_eq!(first.daily[&first_now.date()].observed_tokens, 100);
        assert!(first.daily[&first_now.date()].complete);
        let connection = Connection::open(&fixture.database).unwrap();
        let deferred: (i64, i64, String, Option<String>) = connection
            .query_row(
                "SELECT size_bytes, parsed_offset, completion_state, deferred_until_day
                 FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(deferred.1 < deferred.0);
        assert_eq!(deferred.2, "deferred");
        assert_eq!(deferred.3.as_deref(), Some("2026-08-07"));
        drop(connection);

        let second_now = first_now + Duration::days(1);
        let second = index_local_usage_at(&fixture.database, &fixture.root, second_now).unwrap();
        assert_eq!(second.scan_status, UsageScanStatus::Complete);
        assert_eq!(second.daily[&first_now.date()].observed_tokens, 100);
        assert_eq!(second.daily[&second_now.date()].observed_tokens, 100);
        let connection = Connection::open(&fixture.database).unwrap();
        let completed: (i64, i64, String, Option<String>) = connection
            .query_row(
                "SELECT size_bytes, parsed_offset, completion_state, deferred_until_day
                 FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(completed.0, completed.1);
        assert_eq!(completed.2, "complete");
        assert_eq!(completed.3, None);
    }

    #[test]
    fn sqlite_index_preserves_a_parser_error_while_a_future_record_is_deferred() {
        let fixture = TempUsage::new();
        let future = r#"{"timestamp":"2026-08-07T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":0,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":140,"cached_input_tokens":0,"output_tokens":60,"reasoning_output_tokens":0,"total_tokens":200}},"rate_limits":null}}"#;
        fs::write(
            &fixture.rollout,
            format!("{}not-json\n{future}\n", root_rollout(100)),
        )
        .unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(indexed.scan_status, UsageScanStatus::Unavailable);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
        assert!(!indexed.daily[&now.date()].complete);
        let connection = Connection::open(&fixture.database).unwrap();
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT completion_state, deferred_until_day FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state.0, "deferred-error");
        assert_eq!(state.1.as_deref(), Some("2026-08-07"));
    }

    #[test]
    fn sqlite_index_rebuilds_a_replaced_file_with_the_same_size_and_modified_time() {
        let fixture = TempUsage::new();
        let original = root_rollout(100);
        let replacement = root_rollout(101);
        assert_eq!(original.len(), replacement.len());
        fs::write(&fixture.rollout, original).unwrap();
        let original_modified = fs::metadata(&fixture.rollout).unwrap().modified().unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        assert_eq!(
            index_local_usage_at(&fixture.database, &fixture.root, now)
                .unwrap()
                .daily[&now.date()]
                .observed_tokens,
            100
        );

        let replacement_path = fixture.root.join("replacement.jsonl");
        fs::write(&replacement_path, replacement).unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        fs::rename(&replacement_path, &fixture.rollout).unwrap();
        assert_eq!(
            fs::metadata(&fixture.rollout).unwrap().modified().unwrap(),
            original_modified
        );

        let rebuilt = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(rebuilt.daily[&now.date()].observed_tokens, 101);
    }

    #[test]
    fn sqlite_index_rebuilds_an_in_place_rewrite_that_grows() {
        let fixture = TempUsage::new();
        let original = root_rollout(100);
        fs::write(&fixture.rollout, &original).unwrap();
        let original_metadata = fs::metadata(&fixture.rollout).unwrap();
        let original_identity = file_identity(&original_metadata);
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        assert_eq!(
            index_local_usage_at(&fixture.database, &fixture.root, now)
                .unwrap()
                .daily[&now.date()]
                .observed_tokens,
            100
        );

        let replacement = format!("{}{}", root_rollout(50), appended_total(80));
        assert!(replacement.len() > original.len());
        fs::write(&fixture.rollout, replacement).unwrap();
        #[cfg(unix)]
        assert_eq!(
            file_identity(&fs::metadata(&fixture.rollout).unwrap()),
            original_identity
        );

        let rebuilt = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(rebuilt.daily[&now.date()].observed_tokens, 80);
    }

    #[test]
    fn sqlite_index_ignores_an_oversized_compacted_record() {
        let fixture = TempUsage::new();
        let mut rollout = root_rollout(100);
        rollout.push_str(r#"{"timestamp":"2026-08-06T10:02:00Z","type":"compacted","payload":""#);
        rollout.extend(std::iter::repeat_n('x', MAX_ROLLOUT_LINE_BYTES + 1));
        rollout.push_str("\"}\n");
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let completion_state: String = connection
            .query_row(
                "SELECT completion_state FROM codex_usage_files",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(completion_state, "complete");
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
    }

    #[test]
    fn sqlite_index_cleanly_fast_forwards_an_unresolved_subagent_from_an_unreviewed_version() {
        let fixture = TempUsage::new();
        let mut rollout = json!({
            "timestamp": "2026-08-06T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "cli_version": "0.148.0-alpha.1",
                "source": { "subagent": { "thread_spawn": {} } }
            }
        })
        .to_string();
        rollout.push('\n');
        rollout.extend(std::iter::repeat_n('x', 1024 * 1024));
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let (size, offset, completion, excluded): (i64, i64, String, bool) = connection
            .query_row(
                "SELECT size_bytes, parsed_offset, completion_state, usage_excluded FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let detail_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM codex_usage_file_model_days",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(indexed.daily.is_empty());
        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(size, offset);
        assert_eq!(completion, "complete");
        assert!(excluded);
        assert_eq!(detail_rows, 0);
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
                max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
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
                max_file_bytes: 1,
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
    fn debug_report_is_sanitized_and_keeps_token_subsets_out_of_observed_tokens() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let account_observation = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100)]),
        };
        store_cached_account_usage(
            Some(&fixture.database),
            &account_observation,
            now - Duration::minutes(1),
        )
        .unwrap();
        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(
            local
                .top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("GPT 5.6 Sol")
        );
        assert_eq!(
            local
                .top_model_usage
                .as_ref()
                .map(|top| top.observed_tokens),
            Some(100)
        );
        let account = load_cached_account_usage(Some(&fixture.database)).unwrap();
        let periods = project_usage_periods_with_account_time(
            Some(&account.observation),
            Some(&local),
            now,
            account.observed_at,
        );
        let connection = Connection::open(&fixture.database).unwrap();

        let report =
            render_debug_usage_report(&connection, Some(&account), &local, &periods, now.date())
                .unwrap();

        assert!(report.contains("retention_days=30"));
        assert!(report.contains("model=gpt-5.6-sol observed_tokens=100 input_tokens=70"));
        assert!(report.contains("output_tokens=30"));
        assert!(report.contains("catalog_status=known"));
        assert!(!report.contains(fixture.root.to_string_lossy().as_ref()));
        for private_field in [
            "parsed_offset",
            "parsed_prefix_anchor",
            "parser_version",
            "active_model",
            "file_identity",
        ] {
            assert!(!report.contains(private_field));
        }
    }

    #[test]
    fn debug_catalog_distinguishes_each_missing_price_reason() {
        let manifest = pricing_manifest().unwrap();
        let current = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let before_release = Date::from_calendar_date(2026, Month::June, 1).unwrap();

        assert_eq!(
            debug_catalog_description(Some(manifest), UNKNOWN_MODEL, current, false),
            "status=model-not-observed"
        );
        assert_eq!(
            debug_catalog_description(Some(manifest), "future-model", current, false),
            "status=unknown-model"
        );
        assert_eq!(
            debug_catalog_description(Some(manifest), "gpt-5.6-sol", before_release, false),
            "status=missing-effective-price"
        );
        assert_eq!(
            debug_catalog_description(Some(manifest), "gpt-5.5", current, true),
            "status=missing-cache-write-price"
        );
        assert_eq!(
            debug_catalog_description(None, "gpt-5.6-sol", current, false),
            "status=manifest-unavailable"
        );
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
        let before_summary: (i64, f64) = connection
            .query_row(
                "SELECT observed_tokens, cost_usd FROM codex_usage_file_days",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let changed = changed_pricing_manifest("test-price-basis-v2", 60.0);
        reprice_index_with_manifest(&connection, &changed, now.date(), now.date()).unwrap();
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
        let after_summary: (i64, f64) = connection
            .query_row(
                "SELECT observed_tokens, cost_usd FROM codex_usage_file_days",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
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
        assert_eq!(before_summary.0, after_summary.0);
        assert_ne!(before_summary.1, after_summary.1);
        assert_eq!(after.7, "test-price-basis-v2");
    }

    #[test]
    fn partial_repricing_keeps_the_last_complete_published_basis() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100)]),
        };
        let previous_local = read_indexed_usage(
            &connection,
            now.date(),
            now.date(),
            UsageScanStatus::Complete,
            true,
            None,
        )
        .unwrap();
        let previous = project_usage_periods(Some(&account), Some(&previous_local), now);
        let original_basis = pricing_manifest().unwrap().basis.clone();
        let changed = changed_pricing_manifest("test-price-basis-v2", 60.0);

        assert!(
            !reprice_index_batch_with_manifest(&connection, &changed, now.date(), now.date(), 1,)
                .unwrap()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM codex_usage_index_meta WHERE key = 'pricing_basis'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            original_basis
        );

        let mut partial_local = read_indexed_usage(
            &connection,
            now.date(),
            now.date(),
            UsageScanStatus::Indexing,
            false,
            None,
        )
        .unwrap();
        partial_local.suppress_cost_evidence();
        let partial = preserve_best_known_costs(
            project_usage_periods(Some(&account), Some(&partial_local), now),
            &previous,
        );
        let UsageTotal::Current {
            api_equivalent_cost_usd: partial_cost,
            api_equivalent_cost_basis: partial_basis,
            ..
        } = partial.today
        else {
            panic!("partial projection must keep account usage");
        };
        let UsageTotal::Current {
            api_equivalent_cost_usd: previous_cost,
            api_equivalent_cost_basis: previous_basis,
            ..
        } = previous.today
        else {
            panic!("previous projection must be current");
        };
        assert_eq!(partial_cost, previous_cost);
        assert_eq!(partial_basis, previous_basis);

        assert!(
            reprice_index_batch_with_manifest(&connection, &changed, now.date(), now.date(), 1,)
                .unwrap()
        );
        let repriced_local = read_indexed_usage(
            &connection,
            now.date(),
            now.date(),
            UsageScanStatus::Complete,
            true,
            None,
        )
        .unwrap();
        let repriced = project_usage_periods(Some(&account), Some(&repriced_local), now);
        let UsageTotal::Current {
            api_equivalent_cost_usd: repriced_cost,
            api_equivalent_cost_basis: repriced_basis,
            ..
        } = repriced.today
        else {
            panic!("repriced projection must be current");
        };
        assert_ne!(repriced_cost, previous_cost);
        assert_eq!(repriced_basis.as_deref(), Some("test-price-basis-v2"));
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
        reprice_index_with_manifest(&connection, &changed, now.date(), now.date()).unwrap();
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
    fn semantic_manifest_change_reprices_only_affected_model_days() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let (path, day, pricing_input_tokens, usage, complete, observed_through) = connection
            .query_row(
                "SELECT path, day, pricing_input_tokens, input_tokens, cached_input_tokens,
                        cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                        observed_tokens, complete, observed_through
                 FROM codex_usage_file_model_days",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        TokenUsage {
                            input: from_i64(row.get(3)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cached_input: from_i64(row.get(4)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_write_input: from_i64(row.get(5)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            output: from_i64(row.get(6)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            reasoning_output: from_i64(row.get(7)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            total: from_i64(row.get(8)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        },
                        row.get::<_, bool>(9)?,
                        row.get::<_, String>(10)?,
                    ))
                },
            )
            .unwrap();
        let day = parse_ranking_day(&day).unwrap();
        let manifest = pricing_manifest().unwrap();
        let terra_cost = price_usage_tier_with_manifest(
            manifest,
            "gpt-5.6-terra",
            day,
            usage,
            pricing_input_tokens,
        )
        .unwrap();
        let terra_fingerprint =
            pricing_rule_fingerprint(manifest, "gpt-5.6-terra", day, usage, pricing_input_tokens);
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                   complete, observed_through
                 ) VALUES(?1, ?2, 'gpt-5.6-terra', ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                          ?10, ?11, ?12, ?13, ?14)",
                params![
                    path,
                    day.to_string(),
                    to_i64(pricing_input_tokens).unwrap(),
                    to_i64(usage.input).unwrap(),
                    to_i64(usage.cached_input).unwrap(),
                    to_i64(usage.cache_write_input).unwrap(),
                    to_i64(usage.output).unwrap(),
                    to_i64(usage.reasoning_output).unwrap(),
                    to_i64(usage.total).unwrap(),
                    terra_cost,
                    manifest.basis.as_str(),
                    terra_fingerprint,
                    complete,
                    observed_through,
                ],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TABLE repriced_model_days(model TEXT NOT NULL);
                 CREATE TEMP TRIGGER record_model_day_reprice
                 AFTER UPDATE OF cost_usd ON codex_usage_file_model_days
                 BEGIN
                   INSERT INTO repriced_model_days(model) VALUES(NEW.model);
                 END;",
            )
            .unwrap();

        let changed = changed_pricing_manifest("openai-standard-2026-08-06-v1", 60.0);
        reprice_index_with_manifest(&connection, &changed, day, day).unwrap();

        let repriced = connection
            .prepare("SELECT model FROM repriced_model_days ORDER BY model")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(repriced, vec!["gpt-5.6-sol"]);
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_fingerprint FROM codex_usage_file_model_days
                     WHERE model = 'gpt-5.6-terra'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            pricing_rule_fingerprint(&changed, "gpt-5.6-terra", day, usage, pricing_input_tokens,)
        );
    }

    #[test]
    fn basis_only_change_updates_stored_rows_without_rescanning() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let before: (i64, i64, f64, String) = connection
            .query_row(
                "SELECT f.parsed_offset, d.observed_tokens, d.cost_usd, d.pricing_basis
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        value["basis"] = json!("test-basis-only-v2");
        let changed = parse_pricing_manifest(&value.to_string()).unwrap();

        reprice_index_with_manifest(&connection, &changed, now.date(), now.date()).unwrap();

        let after: (i64, i64, f64, String) = connection
            .query_row(
                "SELECT f.parsed_offset, d.observed_tokens, d.cost_usd, d.pricing_basis
                 FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(before.0, after.0, "rollout cursor must not move");
        assert_eq!(before.1, after.1, "Observed Tokens must not change");
        assert_eq!(before.2, after.2, "numeric prices did not change");
        assert_ne!(before.3, after.3, "stored basis must be refreshed");
        assert_eq!(after.3, "test-basis-only-v2");
    }

    #[test]
    fn repricing_ignores_model_days_outside_the_retention_window() {
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
                   observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                   complete, observed_through
                 ) VALUES (?1, '2026-07-07', 'gpt-5.6-sol', 70, 70, 20, 0, 30, 10,
                           100, 123.0, 'old-basis', 'old-rule', 1,
                           '2026-07-07T12:00:00Z')",
                [path.as_str()],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TABLE repriced_days(day TEXT NOT NULL);
                 CREATE TEMP TRIGGER record_repriced_day
                 AFTER UPDATE OF cost_usd ON codex_usage_file_model_days
                 BEGIN
                   INSERT INTO repriced_days(day) VALUES(NEW.day);
                 END;",
            )
            .unwrap();

        let changed = changed_pricing_manifest("test-retained-price-v2", 60.0);
        let cutoff = Date::from_calendar_date(2026, Month::July, 8).unwrap();
        reprice_index_with_manifest(&connection, &changed, cutoff, now.date()).unwrap();

        let repriced = connection
            .prepare("SELECT day FROM repriced_days ORDER BY day")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(repriced, vec!["2026-08-06"]);
        assert_eq!(
            connection
                .query_row(
                    "SELECT cost_usd FROM codex_usage_file_model_days
                     WHERE day = '2026-07-07'",
                    [],
                    |row| row.get::<_, f64>(0),
                )
                .unwrap(),
            123.0
        );
    }

    #[test]
    fn index_prunes_private_detail_and_checkpoints_outside_the_retention_window() {
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
                [path.as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, complete, observed_through
                 ) VALUES (?1, '2026-08-07', 'gpt-5.6-sol', 7, 7, 0, 0, 3, 0, 10,
                           0.000125, 'future-basis', 1, '2026-08-07T12:00:00Z')",
                [path.as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_days(
                   path, day, observed_tokens, priced_tokens, cost_usd, complete,
                   observed_through, priced_observed_through
                 ) VALUES (?1, '2026-08-07', 10, 10, 0.000125, 1,
                           '2026-08-07T12:00:00Z', '2026-08-07T12:00:00Z')",
                [path.as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   parser_version, completion_state, schema_supported
                 ) VALUES('expired-rollout', 'old-file', 0, 0, 0, ?1, 'complete', 1)",
                [ROLLOUT_PARSER_VERSION],
            )
            .unwrap();
        drop(connection);

        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let out_of_window_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM codex_usage_file_model_days
                 WHERE day < '2026-07-08' OR day > '2026-08-06'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(out_of_window_rows, 0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM codex_usage_file_days
                     WHERE day < '2026-07-08' OR day > '2026-08-06'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM codex_usage_files WHERE path = 'expired-rollout'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
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
        assert!(is_supported_cli_version("0.147.0-alpha.6.5"));
        assert!(!is_supported_cli_version("0.129.9"));
        assert!(!is_supported_cli_version("0.148.0"));
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
        assert!(matches!(
            pricing_catalog_entry(pricing_manifest().unwrap(), "future-model", after_release),
            Err(PricingLookupFailure::UnknownModel)
        ));
        assert!(matches!(
            pricing_catalog_entry(pricing_manifest().unwrap(), "gpt-5.6-sol", before_release),
            Err(PricingLookupFailure::MissingApplicablePrice)
        ));
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

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 300);
        assert!(days[&day].complete);
        assert!(days[&day].api_equivalent_cost_usd.is_some());
    }

    #[test]
    fn rollout_scan_excludes_future_ranking_days() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-07T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"new_total":200},"rate_limits":null}}"#,
            "\n",
        );
        let today = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(
            fixture.as_bytes(),
            today,
            today,
            &mut days
        ));
        assert_eq!(days.len(), 1);
        assert_eq!(days[&today].observed_tokens, 100);
    }

    #[test]
    fn rollout_scan_excludes_an_inherited_fork_without_a_proven_boundary() {
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

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert!(days.is_empty());
    }

    #[test]
    fn rollout_scan_excludes_a_subagent_without_a_proven_history_boundary() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","timestamp":"2026-08-06T10:00:00Z","thread_source":"subagent"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T09:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}"#,
            "\n",
        );
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert!(days.is_empty());
    }

    #[test]
    fn rollout_scan_uses_an_explicit_subagent_history_ordinal() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","thread_source":"subagent","subagent_history_start_ordinal":3}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T09:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}"#,
            "\n",
        );
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
    }

    #[test]
    fn rollout_scan_restores_parser_state_before_the_retention_cutoff() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-05T23:58:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T23:58:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T23:59:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T00:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}"#,
            "\n",
        );
        let cutoff = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(
            fixture.as_bytes(),
            cutoff,
            cutoff,
            &mut days
        ));
        assert_eq!(days.len(), 1);
        assert_eq!(days[&cutoff].observed_tokens, 100);
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

        assert!(!scan_rollout_reader(
            fixture.as_bytes(),
            day,
            day,
            &mut days
        ));
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
                    priced_tokens: 600,
                    api_equivalent_cost_usd: Some(1.0),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
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
    fn committed_partial_rows_supply_an_ongoing_modeled_estimate() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 1_000)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 400,
                    priced_tokens: 400,
                    api_equivalent_cost_usd: Some(0.8),
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(40.0));
    }

    #[test]
    fn a_recent_period_does_not_wait_for_older_pending_files() {
        let period_start = OffsetDateTime::parse("2026-08-06T00:00:00Z", &Rfc3339).unwrap();

        assert_eq!(
            period_scan_status(
                UsageScanStatus::Indexing,
                Some(period_start - Duration::days(1)),
                None,
                period_start,
                true,
            ),
            UsageScanStatus::Complete
        );
    }

    #[test]
    fn a_finished_period_reports_unavailable_when_recent_parser_evidence_failed() {
        let period_start = OffsetDateTime::parse("2026-08-06T00:00:00Z", &Rfc3339).unwrap();

        assert_eq!(
            period_scan_status(
                UsageScanStatus::Unavailable,
                Some(period_start - Duration::days(1)),
                Some(period_start + Duration::hours(1)),
                period_start,
                true,
            ),
            UsageScanStatus::Unavailable
        );
    }

    #[test]
    fn indexing_preserves_the_last_cost_without_replacing_new_account_tokens() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let previous_account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100)]),
        };
        let previous_local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 100,
                    priced_tokens: 100,
                    api_equivalent_cost_usd: Some(1.25),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(2)),
                    priced_observed_through: Some(now - Duration::minutes(2)),
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            ..LocalUsageObservation::default()
        };
        let previous = project_usage_periods(
            Some(&previous_account),
            Some(&previous_local),
            now - Duration::minutes(1),
        );
        let current_account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 200)]),
        };
        let current_local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 201,
                    priced_tokens: 0,
                    api_equivalent_cost_usd: None,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
        };

        let current = project_usage_periods(Some(&current_account), Some(&current_local), now);
        let preserved = preserve_best_known_costs(current, &previous);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = preserved.today
        else {
            panic!("expected current account usage");
        };

        assert_eq!(observed_tokens, 200);
        assert_eq!(api_equivalent_cost_usd, Some(2.5));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(50.0));
        assert_eq!(preserved.scan_status, UsageScanStatus::Indexing);
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
                    priced_tokens: 1_000,
                    api_equivalent_cost_usd: Some(1.25),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            ..LocalUsageObservation::default()
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
    fn a_missing_previous_account_window_does_not_invent_a_trend() {
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
    fn local_rollouts_are_a_partial_fallback_when_account_usage_is_unavailable() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 600,
                    priced_tokens: 600,
                    api_equivalent_cost_usd: Some(1.0),
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
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
    fn local_tokens_above_account_are_scaled_to_the_authoritative_account_total() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(now.date(), 100)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 202,
                    priced_tokens: 202,
                    api_equivalent_cost_usd: Some(2.02),
                    complete: true,
                    observed_through: Some(now + Duration::minutes(1)),
                    priced_observed_through: Some(now + Duration::minutes(1)),
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = projected.today
        else {
            panic!("expected account usage");
        };
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(1.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(100.0));
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
            priced_tokens: tokens,
            api_equivalent_cost_usd: Some(cost),
            complete: true,
            observed_through: Some(now - Duration::minutes(1)),
            priced_observed_through: Some(now - Duration::minutes(1)),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([
                (now.date(), detail(100, 1.0)),
                (yesterday, detail(450, 4.5)),
            ]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
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
