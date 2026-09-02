use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};
use time::{
    Date, Duration, Month, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339,
};

use super::fast_pricing::{FastTurnEvidence, load_fast_turn_evidence, valid_turn_id};
use crate::daily_usage_aggregate::{
    DailyCostEvidence, DailyUsageEvidence, ProviderUsageEvidence, calculate_daily_usage_aggregates,
    calculate_usage_periods, checked_sum, period_days,
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
const TOKEN_HISTORY_RETENTION_DAYS: i64 = 60;
const COST_DETAIL_RETENTION_DAYS: i64 = 30;
const REPRICE_ROWS_PER_PASS: usize = 256;
const PRUNE_ROWS_PER_PASS: usize = 1_000;
const MIN_SUPPORTED_CODEX_CLI_MINOR: u16 = 130;
const MAX_SUPPORTED_CODEX_CLI_MINOR: u16 = 151;
const MIN_REVIEWED_PROVIDER_ORDINAL_MINOR: u16 = 148;
const MAX_REVIEWED_PROVIDER_ORDINAL_MINOR: u16 = 151;
const COMPATIBLE_ROLLOUT_PARSER_VERSION: i64 = 18;
const ROLLOUT_PARSER_VERSION: i64 = 19;
const REQUIRED_PARENT_PROBE_ORDER_VERSION: u8 = 2;
const UNKNOWN_MODEL: &str = "__unknown__";
pub(crate) const USAGE_INDEX_SCHEMA_MODULE: &str = "codex-usage-index";
pub(crate) const USAGE_INDEX_SCHEMA_VERSION: i64 = 9;

#[derive(Clone, Copy)]
struct ScanBudget {
    max_bytes: u64,
    max_file_bytes: u64,
    max_discovery_millis: u128,
    max_parse_millis: u128,
}

const DEFAULT_SCAN_BUDGET: ScanBudget = ScanBudget {
    max_bytes: MAX_ROLLOUT_SCAN_BYTES,
    max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
    max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
    max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
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

    #[cfg(test)]
    fn observed_at_by_day(&self, observed_at: OffsetDateTime) -> BTreeMap<Date, OffsetDateTime> {
        self.daily_tokens
            .keys()
            .map(|day| (*day, observed_at))
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CachedAccountUsageObservation {
    pub(crate) observation: AccountUsageObservation,
    pub(crate) observed_at_by_day: BTreeMap<Date, OffsetDateTime>,
    /// The last successful account usage refresh, including sparse responses.
    pub(crate) observed_at: OffsetDateTime,
}

pub(crate) fn merge_cached_account_usage(
    cached: Option<CachedAccountUsageObservation>,
    observation: AccountUsageObservation,
    observed_at: OffsetDateTime,
) -> CachedAccountUsageObservation {
    let today = utc_ranking_day(observed_at);
    let cutoff = today - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
    let mut daily_tokens = cached
        .as_ref()
        .map(|cached| cached.observation.daily_tokens.clone())
        .unwrap_or_default();
    let mut observed_at_by_day = cached
        .map(|cached| cached.observed_at_by_day)
        .unwrap_or_default();
    daily_tokens.retain(|day, _| (cutoff..=today).contains(day));
    observed_at_by_day.retain(|day, _| (cutoff..=today).contains(day));
    for (day, tokens) in observation.daily_tokens.range(cutoff..=today) {
        daily_tokens.insert(*day, *tokens);
        observed_at_by_day.insert(*day, observed_at);
    }
    CachedAccountUsageObservation {
        observation: AccountUsageObservation { daily_tokens },
        observed_at_by_day,
        observed_at,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawAccountUsageResponse {
    daily_usage_buckets: Option<Vec<RawDailyUsageBucket>>,
    summary: IgnoredAny,
    #[allow(dead_code)]
    #[serde(default)]
    thread_usage: Option<IgnoredAny>,
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
    load_cached_account_usage_from_connection(&connection)
}

fn load_cached_account_usage_from_connection(
    connection: &Connection,
) -> Option<CachedAccountUsageObservation> {
    let observed_at = connection
        .query_row(
            "SELECT refreshed_at FROM codex_account_usage_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()??;
    let observed_at = OffsetDateTime::parse(&observed_at, &Rfc3339).ok()?;
    let daily_rows = connection
        .prepare(
            "SELECT day, tokens, observed_at
             FROM codex_account_usage_days ORDER BY day",
        )
        .ok()?
        .query_map([], |row| {
            let day = parse_ranking_day(&row.get::<_, String>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let tokens = from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let observed_at = OffsetDateTime::parse(&row.get::<_, String>(2)?, &Rfc3339)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((day, tokens, observed_at))
        })
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let daily_tokens = daily_rows
        .iter()
        .map(|(day, tokens, _)| (*day, *tokens))
        .collect();
    let observed_at_by_day = daily_rows
        .into_iter()
        .map(|(day, _, observed_at)| (day, observed_at))
        .collect();
    Some(CachedAccountUsageObservation {
        observation: AccountUsageObservation { daily_tokens },
        observed_at_by_day,
        observed_at,
    })
}

pub(crate) fn store_cached_account_usage(
    database_path: Option<&Path>,
    observation: &AccountUsageObservation,
    observed_at: OffsetDateTime,
) -> Result<(), ()> {
    let today = utc_ranking_day(observed_at);
    let cutoff = today
        .checked_sub(Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1))
        .ok_or(())?;
    let database_path = database_path.ok_or(())?;
    let mut connection = Connection::open(database_path).map_err(|_| ())?;
    ensure_index_schema(&mut connection, Some(database_path))?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_account_usage_days WHERE day < ?1 OR day > ?2",
            params![cutoff.to_string(), today.to_string()],
        )
        .map_err(|_| ())?;
    let observed_at = observed_at.format(&Rfc3339).map_err(|_| ())?;
    for (day, tokens) in observation.daily_tokens.range(cutoff..=today) {
        transaction
            .execute(
                "INSERT INTO codex_account_usage_days(day, tokens, observed_at)
                 VALUES(?1, ?2, ?3)
                 ON CONFLICT(day) DO UPDATE SET
                   tokens = excluded.tokens,
                   observed_at = excluded.observed_at",
                params![day.to_string(), to_i64(*tokens)?, &observed_at],
            )
            .map_err(|_| ())?;
    }
    transaction
        .execute(
            "INSERT INTO codex_account_usage_meta(singleton, refreshed_at) VALUES(1, ?1)
             ON CONFLICT(singleton) DO UPDATE SET refreshed_at=excluded.refreshed_at",
            [&observed_at],
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
    fast_multiplier: Option<f64>,
    long_context: RawLongContextRule,
    fast_long_context: Option<RawFastLongContextPrice>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLongContextRule {
    input_tokens_above: u64,
    input_multiplier: f64,
    output_multiplier: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawFastLongContextPrice {
    effective_from: String,
    effective_until: Option<String>,
    input_usd_per_million: f64,
    cached_input_usd_per_million: f64,
    cache_write_usd_per_million: Option<f64>,
    output_usd_per_million: f64,
}

#[derive(Clone)]
struct PricingManifest {
    basis: String,
    fingerprint: String,
    models: Vec<PricedModel>,
}

impl PricingManifest {
    fn canonical_model_name(&self, model_name: &str) -> Option<&str> {
        self.model(model_name)
            .and_then(|model| model.names.first())
            .map(String::as_str)
    }

    fn model(&self, model_name: &str) -> Option<&PricedModel> {
        let model_name = model_name.strip_prefix("openai/").unwrap_or(model_name);
        self.models
            .iter()
            .find(|model| model.names.iter().any(|name| name == model_name))
            .or_else(|| {
                let suffix_start = model_name.len().checked_sub(11)?;
                let suffix = model_name.get(suffix_start + 1..)?;
                if model_name.as_bytes().get(suffix_start) != Some(&b'-')
                    || parse_ranking_day(suffix).is_err()
                {
                    return None;
                }
                let base = model_name.get(..suffix_start)?;
                self.models
                    .iter()
                    .find(|model| model.names.iter().any(|name| name == base))
            })
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
    fast_multiplier: Option<f64>,
    long_context_input_tokens_above: u64,
    long_context_input_multiplier: f64,
    long_context_output_multiplier: f64,
    fast_long_context: Option<DatedTokenRates>,
}

#[derive(Clone, Copy)]
struct DatedTokenRates {
    effective_from: Date,
    effective_until: Option<Date>,
    rates: TokenRates,
}

impl DatedTokenRates {
    fn applies_to(self, day: Date) -> bool {
        day >= self.effective_from && self.effective_until.is_none_or(|until| day < until)
    }
}

#[derive(Clone, Copy)]
struct TokenRates {
    input_usd_per_million: f64,
    cached_input_usd_per_million: f64,
    cache_write_usd_per_million: Option<f64>,
    output_usd_per_million: f64,
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
                let fast_long_context = period
                    .fast_long_context
                    .map(|fast| {
                        let fast_effective_from = parse_ranking_day(&fast.effective_from)?;
                        let fast_effective_until = fast
                            .effective_until
                            .as_deref()
                            .map(parse_ranking_day)
                            .transpose()?;
                        if fast_effective_until.is_some_and(|until| until <= fast_effective_from)
                            || fast_effective_from < effective_from
                            || effective_until.is_some_and(|until| {
                                fast_effective_from >= until
                                    || fast_effective_until
                                        .is_none_or(|fast_until| fast_until > until)
                            })
                            || ![
                                fast.input_usd_per_million,
                                fast.cached_input_usd_per_million,
                                fast.output_usd_per_million,
                            ]
                            .into_iter()
                            .all(|value| value.is_finite() && value >= 0.0)
                            || fast
                                .cache_write_usd_per_million
                                .is_some_and(|value| !value.is_finite() || value < 0.0)
                        {
                            return Err(());
                        }
                        Ok(DatedTokenRates {
                            effective_from: fast_effective_from,
                            effective_until: fast_effective_until,
                            rates: TokenRates {
                                input_usd_per_million: fast.input_usd_per_million,
                                cached_input_usd_per_million: fast.cached_input_usd_per_million,
                                cache_write_usd_per_million: fast.cache_write_usd_per_million,
                                output_usd_per_million: fast.output_usd_per_million,
                            },
                        })
                    })
                    .transpose()?;
                if effective_until.is_some_and(|until| until <= effective_from)
                    || period.long_context.input_tokens_above == 0
                    || ![
                        period.input_usd_per_million,
                        period.cached_input_usd_per_million,
                        period.output_usd_per_million,
                        period.fast_multiplier.unwrap_or(1.0),
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
                    || period.fast_multiplier.is_some_and(|value| value < 1.0)
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
                    fast_multiplier: period.fast_multiplier,
                    long_context_input_tokens_above: period.long_context.input_tokens_above,
                    long_context_input_multiplier: period.long_context.input_multiplier,
                    long_context_output_multiplier: period.long_context.output_multiplier,
                    fast_long_context,
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
                    let fast_long_context = period.fast_long_context.map_or_else(
                        || "none".to_owned(),
                        |fast| {
                            format!(
                                "{}~{}~{:016x}~{:016x}~{}~{:016x}",
                                fast.effective_from,
                                fast.effective_until
                                    .map_or_else(|| "open".to_owned(), |until| until.to_string()),
                                fast.rates.input_usd_per_million.to_bits(),
                                fast.rates.cached_input_usd_per_million.to_bits(),
                                fast.rates.cache_write_usd_per_million.map_or_else(
                                    || "none".to_owned(),
                                    |price| format!("{:016x}", price.to_bits())
                                ),
                                fast.rates.output_usd_per_million.to_bits(),
                            )
                        },
                    );
                    format!(
                        "{}|{}|{:016x}|{:016x}|{}|{:016x}|{}|{}|{:016x}|{:016x}|{}",
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
                        period.fast_multiplier.map_or_else(
                            || "none".to_owned(),
                            |multiplier| format!("{:016x}", multiplier.to_bits())
                        ),
                        period.long_context_input_tokens_above,
                        period.long_context_input_multiplier.to_bits(),
                        period.long_context_output_multiplier.to_bits(),
                        fast_long_context,
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

pub(super) fn current_pricing_basis() -> Option<&'static str> {
    pricing_manifest().map(|manifest| manifest.basis.as_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PricingLookupFailure {
    MissingApplicablePrice,
    MissingCacheWritePrice,
    MissingFastLongContextPrice,
    MissingFastPrice,
    UnknownModel,
}

impl PricingLookupFailure {
    fn as_reason(self) -> &'static str {
        match self {
            Self::MissingApplicablePrice => "missing_applicable_price",
            Self::MissingCacheWritePrice => "missing_cache_write_price",
            Self::MissingFastLongContextPrice => "missing_fast_long_context_price",
            Self::MissingFastPrice => "missing_fast_price",
            Self::UnknownModel => "unknown_model",
        }
    }
}

fn pricing_catalog_entry(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
) -> Result<PriceCatalogEntry, PricingLookupFailure> {
    let model = manifest
        .model(model)
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

fn model_has_fast_multiplier(model: &str, day: Date) -> bool {
    pricing_manifest()
        .and_then(|manifest| pricing_catalog_entry(manifest, model, day).ok())
        .is_some_and(|entry| entry.fast_multiplier.is_some())
}

#[cfg(debug_assertions)]
fn debug_pricing_lookup_failure(model: &str, day: Date, failure: PricingLookupFailure) {
    static REPORTED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let reason = if model == UNKNOWN_MODEL {
        "model_not_observed"
    } else {
        failure.as_reason()
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
    price_usage_tier(model, day, usage, usage.input, PricingMode::Standard)
}

#[cfg(test)]
fn price_usage_tier(
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
    pricing_mode: PricingMode,
) -> Option<f64> {
    price_usage_tier_with_manifest(
        pricing_manifest()?,
        model,
        day,
        usage,
        pricing_input_tokens,
        pricing_mode,
    )
}

fn price_usage_tier_with_manifest(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
    pricing_mode: PricingMode,
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
    let rates = match pricing_rates(entry, day, long_context, &pricing_mode) {
        Ok(rates) => rates,
        Err(failure) => {
            debug_pricing_lookup_failure(model, day, failure);
            return None;
        }
    };
    let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
    let cache_write = if billable.cache_write_input == 0 {
        0.0
    } else {
        let Some(rate) = rates.cache_write_usd_per_million else {
            debug_pricing_lookup_failure(model, day, PricingLookupFailure::MissingCacheWritePrice);
            return None;
        };
        per_million(billable.cache_write_input, rate)
    };
    let cost = per_million(billable.standard_input, rates.input_usd_per_million)
        + per_million(billable.cached_input, rates.cached_input_usd_per_million)
        + cache_write
        + per_million(billable.output, rates.output_usd_per_million);
    cost.is_finite().then_some(cost)
}

fn pricing_rates(
    entry: PriceCatalogEntry,
    day: Date,
    long_context: bool,
    pricing_mode: &PricingMode,
) -> Result<TokenRates, PricingLookupFailure> {
    match pricing_mode {
        PricingMode::Standard => {
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
            Ok(TokenRates {
                input_usd_per_million: entry.input_usd_per_million * input_multiplier,
                cached_input_usd_per_million: entry.cached_input_usd_per_million * input_multiplier,
                cache_write_usd_per_million: entry
                    .cache_write_usd_per_million
                    .map(|rate| rate * input_multiplier),
                output_usd_per_million: entry.output_usd_per_million * output_multiplier,
            })
        }
        PricingMode::Fast if long_context => {
            if let Some(price) = entry
                .fast_long_context
                .filter(|price| price.applies_to(day))
            {
                return Ok(price.rates);
            }
            if entry.fast_long_context.is_some() {
                return Err(PricingLookupFailure::MissingFastLongContextPrice);
            }
            if entry.long_context_input_multiplier != 1.0
                || entry.long_context_output_multiplier != 1.0
            {
                return Err(PricingLookupFailure::MissingFastLongContextPrice);
            }
            let multiplier = entry
                .fast_multiplier
                .ok_or(PricingLookupFailure::MissingFastPrice)?;
            Ok(TokenRates {
                input_usd_per_million: entry.input_usd_per_million * multiplier,
                cached_input_usd_per_million: entry.cached_input_usd_per_million * multiplier,
                cache_write_usd_per_million: entry
                    .cache_write_usd_per_million
                    .map(|rate| rate * multiplier),
                output_usd_per_million: entry.output_usd_per_million * multiplier,
            })
        }
        PricingMode::Fast => {
            let multiplier = entry
                .fast_multiplier
                .ok_or(PricingLookupFailure::MissingFastPrice)?;
            Ok(TokenRates {
                input_usd_per_million: entry.input_usd_per_million * multiplier,
                cached_input_usd_per_million: entry.cached_input_usd_per_million * multiplier,
                cache_write_usd_per_million: entry
                    .cache_write_usd_per_million
                    .map(|rate| rate * multiplier),
                output_usd_per_million: entry.output_usd_per_million * multiplier,
            })
        }
    }
}

fn pricing_rule_fingerprint(
    manifest: &PricingManifest,
    model: &str,
    day: Date,
    usage: TokenUsage,
    pricing_input_tokens: u64,
    pricing_mode: PricingMode,
) -> String {
    let billable = match usage.billable() {
        Ok(billable) => billable,
        Err(()) => return stable_pricing_fingerprint("unavailable:invalid-token-arithmetic"),
    };
    let entry = match pricing_catalog_entry(manifest, model, day) {
        Ok(entry) => entry,
        Err(failure) => {
            return stable_pricing_fingerprint(&format!(
                "unavailable:{}:{model}:{day}",
                failure.as_reason()
            ));
        }
    };
    let long_context = pricing_input_tokens > entry.long_context_input_tokens_above;
    let rates = match pricing_rates(entry, day, long_context, &pricing_mode) {
        Ok(rates) => rates,
        Err(failure) => {
            return stable_pricing_fingerprint(&format!(
                "unavailable:{}:{model}:{day}",
                failure.as_reason()
            ));
        }
    };
    if billable.cache_write_input > 0 && rates.cache_write_usd_per_million.is_none() {
        return stable_pricing_fingerprint(&format!(
            "unavailable:missing-cache-write-price:{model}:{day}"
        ));
    }
    let applicable_rate =
        |tokens: u64, rate: f64| (tokens > 0).then(|| format!("{:016x}", rate.to_bits()));
    stable_pricing_fingerprint(&format!(
        "priced:mode={}:standard={}:cached={}:cache-write={}:output={}",
        pricing_mode.as_stored(),
        applicable_rate(billable.standard_input, rates.input_usd_per_million)
            .unwrap_or_else(|| "unused".to_owned()),
        applicable_rate(billable.cached_input, rates.cached_input_usd_per_million)
            .unwrap_or_else(|| "unused".to_owned()),
        rates
            .cache_write_usd_per_million
            .and_then(|rate| applicable_rate(billable.cache_write_input, rate))
            .unwrap_or_else(|| "unused".to_owned()),
        applicable_rate(billable.output, rates.output_usd_per_million)
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
    has_excluded_usage: bool,
    latest_pending_modified_at: Option<OffsetDateTime>,
    latest_incomplete_modified_at: Option<OffsetDateTime>,
    scan_scope_known: bool,
}

fn period_scan_status(
    scan_status: UsageScanStatus,
    latest_pending_modified_at: Option<OffsetDateTime>,
    latest_incomplete_modified_at: Option<OffsetDateTime>,
    period_start: OffsetDateTime,
    scan_scope_known: bool,
) -> UsageScanStatus {
    if !scan_scope_known {
        return scan_status;
    }
    if latest_pending_modified_at.is_some_and(|modified_at| modified_at >= period_start) {
        return UsageScanStatus::Indexing;
    }
    if latest_incomplete_modified_at.is_some_and(|modified_at| modified_at >= period_start) {
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
            has_excluded_usage: false,
            latest_pending_modified_at: None,
            latest_incomplete_modified_at: None,
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
            self.latest_incomplete_modified_at,
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
            detail.pricing_basis = None;
        }
        self.pricing_basis = None;
    }
}

#[derive(Default)]
enum RequiredWhenPresent<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T> Deserialize<'de> for RequiredWhenPresent<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

impl<T> RequiredWhenPresent<T> {
    fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRolloutHeader {
    #[serde(default)]
    ordinal: RequiredWhenPresent<u64>,
    timestamp: String,
    #[serde(rename = "type")]
    record_type: String,
    payload: IgnoredAny,
    #[serde(default, rename = "model")]
    _model: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEventLine {
    #[serde(default, rename = "ordinal")]
    _ordinal: RequiredWhenPresent<u64>,
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawEventPayload,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTurnContextLine {
    #[serde(default, rename = "ordinal")]
    _ordinal: RequiredWhenPresent<u64>,
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawTurnContext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessionMetaLine {
    #[serde(default, rename = "ordinal")]
    _ordinal: RequiredWhenPresent<u64>,
    timestamp: String,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawSessionMeta,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInterAgentCommunicationLine {
    #[serde(default, rename = "ordinal")]
    _ordinal: RequiredWhenPresent<u64>,
    timestamp: IgnoredAny,
    #[serde(rename = "type")]
    record_type: IgnoredAny,
    payload: RawInterAgentCommunication,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "type")]
enum RawEventPayload {
    TaskStarted {
        #[serde(default, alias = "turnId")]
        turn_id: Option<String>,
        model_context_window: IgnoredAny,
        collaboration_mode_kind: IgnoredAny,
        #[serde(default)]
        started_at: Option<IgnoredAny>,
    },
    TokenCount {
        #[serde(default)]
        info: Option<Box<RawTokenInfo>>,
        #[serde(default, alias = "turnId")]
        turn_id: Option<String>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        rate_limits: Option<IgnoredAny>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        model_name: Option<IgnoredAny>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTokenInfo {
    #[serde(default)]
    last_token_usage: Option<TokenUsage>,
    model_context_window: u64,
    #[serde(default)]
    total_token_usage: Option<TokenUsage>,
    #[serde(default, alias = "turnId")]
    turn_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
}

#[derive(Deserialize)]
struct RawTurnContext {
    model: String,
}

#[derive(Deserialize)]
struct RawSessionMeta {
    cli_version: String,
    #[serde(default)]
    history_mode: Option<String>,
    #[serde(default)]
    history_base: RequiredWhenPresent<RawHistoryBase>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
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
#[serde(deny_unknown_fields)]
struct RawHistoryBase {
    #[serde(rename = "thread_id")]
    _thread_id: String,
    end_ordinal_exclusive: u64,
    #[serde(rename = "end_byte_offset")]
    _end_byte_offset: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInterAgentCommunication {
    #[serde(default)]
    trigger_turn: Option<bool>,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LineageMode {
    #[default]
    Unknown,
    Root,
    Discovering,
    ExplicitBoundary,
    Independent,
    ParentResolved,
    Unresolved,
}

impl LineageMode {
    fn as_stored(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Root => "root",
            Self::Discovering => "discovering",
            Self::ExplicitBoundary => "explicit-boundary",
            Self::Independent => "independent",
            Self::ParentResolved => "parent-resolved",
            Self::Unresolved => "unresolved",
        }
    }

    fn from_stored(value: &str) -> Self {
        match value {
            "root" => Self::Root,
            "discovering" => Self::Discovering,
            "explicit-boundary" => Self::ExplicitBoundary,
            "independent" => Self::Independent,
            "parent-resolved" => Self::ParentResolved,
            "unresolved" => Self::Unresolved,
            _ => Self::Unknown,
        }
    }

    fn needs_dependency_check(self) -> bool {
        matches!(self, Self::ParentResolved | Self::Unresolved)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ProviderOrdinalMode {
    #[default]
    Unknown,
    Legacy,
    Provider,
}

impl ProviderOrdinalMode {
    fn as_stored(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Legacy => "legacy",
            Self::Provider => "provider",
        }
    }

    fn from_stored(value: &str) -> Result<Self, ()> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "legacy" => Ok(Self::Legacy),
            "provider" => Ok(Self::Provider),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RolloutScanState {
    active_model: Option<String>,
    active_turn_id: Option<String>,
    baseline_is_inherited: Option<bool>,
    history_start_ordinal: Option<u64>,
    record_ordinal: u64,
    provider_ordinal_mode: ProviderOrdinalMode,
    exclude_usage: bool,
    previous: Option<TokenUsage>,
    schema_supported: bool,
    lineage_mode: LineageMode,
    leaf_session_id: Option<String>,
    parent_session_id: Option<String>,
    parent_identity_explicit: bool,
    fork_timestamp_ns: Option<i64>,
    embedded_ancestor_seen: bool,
    lineage_invalid: bool,
    parent_dependency_key: Option<String>,
    parent_baseline: Option<TokenUsage>,
    last_turn_context_is_first: bool,
    last_turn_context_ordinal: Option<u64>,
    marker_based_boundary: bool,
    marker_candidate_invalidated: bool,
    marker_local_confirmation: Option<bool>,
    parser_error_seen: bool,
    snapshot_last_timestamp_ns: Option<i64>,
    snapshot_timestamp_regressed: bool,
    task_counter_reset_pending: bool,
}

fn normalized_session_id(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
            .then(|| value.to_owned())
    })
}

fn timestamp_ns(value: &str) -> Result<i64, ()> {
    i64::try_from(parse_rollout_timestamp(value)?.unix_timestamp_nanos()).map_err(|_| ())
}

fn apply_session_metadata(
    state: &mut RolloutScanState,
    metadata: RawSessionMeta,
    line_timestamp: &str,
) -> Result<(), ()> {
    let supported = is_supported_cli_version(&metadata.cli_version);
    let observed_id = normalized_session_id(metadata.id.clone());
    let observed_parent_id = normalized_session_id(metadata.forked_from_id.clone());
    let malformed_parent_id = metadata.forked_from_id.is_some() && observed_parent_id.is_none();
    let observed_fork_timestamp_ns =
        timestamp_ns(metadata.timestamp.as_deref().unwrap_or(line_timestamp)).ok();
    if state.baseline_is_inherited.is_none() {
        state.schema_supported = supported;
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
        state.leaf_session_id = observed_id.clone();
        state.lineage_mode = if !is_inherited {
            LineageMode::Root
        } else if state.history_start_ordinal.is_some() {
            LineageMode::ExplicitBoundary
        } else {
            LineageMode::Discovering
        };
        if state.lineage_mode == LineageMode::Discovering {
            state.parent_session_id = observed_parent_id.clone();
            state.parent_identity_explicit = observed_parent_id.is_some();
            state.fork_timestamp_ns = observed_fork_timestamp_ns;
            state.lineage_invalid = state.leaf_session_id.is_none()
                || state.fork_timestamp_ns.is_none()
                || malformed_parent_id
                || (state.parent_session_id.is_some()
                    && state.parent_session_id == state.leaf_session_id)
                || (metadata.timestamp.is_some()
                    && metadata
                        .timestamp
                        .as_deref()
                        .is_some_and(|value| timestamp_ns(value).is_err()));
        }
    } else {
        if observed_id.as_deref() == state.leaf_session_id.as_deref() {
            state.schema_supported &= supported;
        }
        if state.lineage_mode == LineageMode::Discovering {
            match (state.leaf_session_id.as_deref(), observed_id.as_deref()) {
                (Some(leaf), Some(observed)) if leaf == observed => {
                    if malformed_parent_id {
                        state.lineage_invalid = true;
                    } else if let Some(observed_parent) = observed_parent_id.as_deref() {
                        if state.parent_identity_explicit
                            && state
                                .parent_session_id
                                .as_deref()
                                .is_some_and(|parent| parent != observed_parent)
                        {
                            state.lineage_invalid = true;
                        } else {
                            state.parent_session_id = Some(observed_parent.to_owned());
                            state.parent_identity_explicit = true;
                            state.fork_timestamp_ns = observed_fork_timestamp_ns;
                            state.lineage_invalid |= state.fork_timestamp_ns.is_none();
                        }
                    }
                }
                (Some(_), Some(observed)) => {
                    state.embedded_ancestor_seen = true;
                    if state.marker_based_boundary {
                        state.marker_candidate_invalidated = true;
                        state.history_start_ordinal = None;
                        state.parent_baseline = None;
                        state.marker_local_confirmation = None;
                    }
                    if !state.parent_identity_explicit {
                        if state
                            .parent_session_id
                            .as_deref()
                            .is_some_and(|parent| parent != observed)
                        {
                            state.lineage_invalid = true;
                        } else {
                            state.parent_session_id = Some(observed.to_owned());
                        }
                    }
                }
                _ => state.lineage_invalid = true,
            }
        } else if state.lineage_mode == LineageMode::ExplicitBoundary && state.marker_based_boundary
        {
            if observed_id.as_deref() != state.leaf_session_id.as_deref() {
                state.lineage_mode = LineageMode::Discovering;
                state.embedded_ancestor_seen = true;
                state.marker_candidate_invalidated = true;
                state.history_start_ordinal = None;
                state.parent_baseline = None;
                state.marker_local_confirmation = None;
                state.exclude_usage = true;
                if let Some(observed) = observed_id {
                    if !state.parent_identity_explicit {
                        if state
                            .parent_session_id
                            .as_deref()
                            .is_some_and(|parent| parent != observed)
                        {
                            state.lineage_invalid = true;
                        } else {
                            state.parent_session_id = Some(observed);
                        }
                    }
                } else {
                    state.lineage_invalid = true;
                }
            }
        } else if state.lineage_mode == LineageMode::Independent {
            if observed_id.as_deref() != state.leaf_session_id.as_deref() {
                state.lineage_mode = LineageMode::Discovering;
                state.embedded_ancestor_seen = true;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
                if let Some(observed) = observed_id {
                    if !state.parent_identity_explicit {
                        if state
                            .parent_session_id
                            .as_deref()
                            .is_some_and(|parent| parent != observed)
                        {
                            state.lineage_invalid = true;
                        } else {
                            state.parent_session_id = Some(observed);
                        }
                    }
                } else {
                    state.lineage_invalid = true;
                }
            } else if malformed_parent_id {
                state.lineage_mode = LineageMode::Discovering;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
                state.lineage_invalid = true;
                state.parent_dependency_key = None;
                state.parent_baseline = None;
            } else if let Some(history_start) = metadata.subagent_history_start_ordinal {
                state.lineage_mode = LineageMode::Discovering;
                state.history_start_ordinal = Some(history_start);
                state.marker_based_boundary = false;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
            } else if observed_parent_id.is_some()
                && (observed_parent_id != state.parent_session_id
                    || observed_fork_timestamp_ns != state.fork_timestamp_ns)
            {
                state.lineage_mode = LineageMode::Discovering;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
                state.parent_session_id = observed_parent_id;
                state.parent_identity_explicit = true;
                state.fork_timestamp_ns = observed_fork_timestamp_ns;
                state.parent_dependency_key = None;
                state.parent_baseline = None;
                state.lineage_invalid |= state.fork_timestamp_ns.is_none()
                    || state.parent_session_id == state.leaf_session_id;
            }
        } else if state.lineage_mode == LineageMode::ParentResolved {
            let is_same_leaf = observed_id.as_deref() == state.leaf_session_id.as_deref();
            let is_known_identity = is_same_leaf
                || observed_id.as_deref() == state.parent_session_id.as_deref()
                || (state.parent_identity_explicit && observed_id.is_some());
            let same_leaf_parent_conflicts = is_same_leaf
                && observed_parent_id.is_some()
                && observed_parent_id != state.parent_session_id;
            let same_leaf_fork_changed = is_same_leaf
                && observed_parent_id == state.parent_session_id
                && observed_parent_id.is_some()
                && observed_fork_timestamp_ns != state.fork_timestamp_ns;
            if is_same_leaf
                && observed_parent_id == state.parent_session_id
                && observed_parent_id.is_some()
            {
                state.parent_identity_explicit = true;
            }
            if !is_known_identity
                || same_leaf_parent_conflicts
                || (is_same_leaf && malformed_parent_id)
            {
                state.lineage_mode = LineageMode::Discovering;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
                state.lineage_invalid = true;
                state.parent_dependency_key = None;
                state.parent_baseline = None;
            } else if same_leaf_fork_changed {
                state.lineage_mode = LineageMode::Discovering;
                state.marker_candidate_invalidated = true;
                state.exclude_usage = true;
                state.fork_timestamp_ns = observed_fork_timestamp_ns;
                state.parent_dependency_key = None;
                state.parent_baseline = None;
            }
        }
    }
    // An unsupported inherited rollout can fast-forward safely. A supported
    // inherited rollout remains excluded while the index looks for a boundary.
    (state.schema_supported || state.exclude_usage)
        .then_some(())
        .ok_or(())
}

fn apply_inter_agent_boundary(
    state: &mut RolloutScanState,
    record_ordinal: u64,
    metadata: RawInterAgentCommunication,
) {
    if metadata.trigger_turn == Some(true)
        && state.schema_supported
        && state.baseline_is_inherited == Some(true)
        && state.lineage_mode == LineageMode::Discovering
        && state.history_start_ordinal.is_none()
        && (state.embedded_ancestor_seen || state.last_turn_context_is_first)
        && (state.previous.is_some()
            || (!state.embedded_ancestor_seen && state.last_turn_context_is_first))
        && (state.embedded_ancestor_seen
            || state.previous.is_none_or(|baseline| baseline.total > 0))
        && state
            .last_turn_context_ordinal
            .and_then(|ordinal| ordinal.checked_add(1))
            == Some(record_ordinal)
        && let Some(history_start) = record_ordinal.checked_add(1)
    {
        // Reuse the persisted ordinal boundary so append and restart scans keep
        // the copied cumulative prefix only as the child's delta baseline.
        state.history_start_ordinal = Some(history_start);
        state.parent_baseline = Some(state.previous.unwrap_or_default());
        state.marker_local_confirmation = None;
        state.marker_based_boundary = true;
    }
}

fn is_unowned_copied_prefix(
    state: &RolloutScanState,
    record_ordinal: u64,
    timestamp_ns: Option<i64>,
) -> bool {
    if state.lineage_mode == LineageMode::ParentResolved
        && state.baseline_is_inherited == Some(true)
        && state
            .fork_timestamp_ns
            .zip(timestamp_ns)
            .is_some_and(|(fork, timestamp)| timestamp < fork)
    {
        return true;
    }
    if state.baseline_is_inherited == Some(true)
        && state
            .history_start_ordinal
            .is_some_and(|start| record_ordinal < start)
    {
        return true;
    }
    state.lineage_mode == LineageMode::Discovering
        && !(state.marker_based_boundary
            && state
                .history_start_ordinal
                .is_some_and(|start| record_ordinal >= start))
}

fn record_precedes_parent_fork(state: &RolloutScanState, timestamp_ns: Option<i64>) -> bool {
    state.lineage_mode == LineageMode::ParentResolved
        && state
            .fork_timestamp_ns
            .zip(timestamp_ns)
            .is_some_and(|(fork, timestamp)| timestamp < fork)
}

fn reviewed_provider_ordinal_origin(line: &[u8], record_type: &str, ordinal: u64) -> bool {
    if record_type != "session_meta" {
        return false;
    }
    let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(line) else {
        return false;
    };
    let metadata = line.payload;
    let uses_reviewed_provider_ordinals = metadata
        .cli_version
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u16>().ok())
        .is_some_and(|minor| {
            (MIN_REVIEWED_PROVIDER_ORDINAL_MINOR..=MAX_REVIEWED_PROVIDER_ORDINAL_MINOR)
                .contains(&minor)
        });
    if !uses_reviewed_provider_ordinals || metadata.history_mode.as_deref() != Some("paginated") {
        return false;
    }
    match metadata.history_base.as_ref() {
        Some(base) => base.end_ordinal_exclusive == ordinal,
        None => ordinal == 0,
    }
}

fn reviewed_legacy_ordinal_origin(line: &[u8], record_type: &str) -> bool {
    if record_type != "session_meta" {
        return false;
    }
    let Ok(line) = serde_json::from_slice::<RawSessionMetaLine>(line) else {
        return false;
    };
    let metadata = line.payload;
    if metadata.history_base.as_ref().is_some() {
        return false;
    }
    let minor = metadata
        .cli_version
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u16>().ok());
    match minor {
        Some(148) => metadata.history_mode.as_deref() == Some("legacy"),
        Some(130..=147) => metadata
            .history_mode
            .as_deref()
            .is_none_or(|mode| mode == "legacy"),
        Some(149..=MAX_REVIEWED_PROVIDER_ORDINAL_MINOR) => false,
        _ => {
            metadata
                .thread_source
                .as_ref()
                .into_iter()
                .chain(metadata.source.as_ref())
                .any(RawThreadSource::is_subagent)
                && metadata
                    .history_mode
                    .as_deref()
                    .is_none_or(|mode| mode == "legacy")
        }
    }
}

fn effective_record_ordinal(
    expected: u64,
    provider_ordinal: RequiredWhenPresent<u64>,
    line_starts_file: bool,
    record_type: &str,
    line: &[u8],
    mode: &mut ProviderOrdinalMode,
) -> Result<u64, ()> {
    match (*mode, provider_ordinal) {
        (ProviderOrdinalMode::Unknown, RequiredWhenPresent::Present(ordinal))
            if line_starts_file
                && expected == 0
                && ordinal < u64::MAX
                && reviewed_provider_ordinal_origin(line, record_type, ordinal) =>
        {
            *mode = ProviderOrdinalMode::Provider;
            Ok(ordinal)
        }
        (ProviderOrdinalMode::Unknown, RequiredWhenPresent::Missing)
            if line_starts_file
                && expected == 0
                && reviewed_legacy_ordinal_origin(line, record_type) =>
        {
            *mode = ProviderOrdinalMode::Legacy;
            Ok(expected)
        }
        (ProviderOrdinalMode::Provider, RequiredWhenPresent::Present(ordinal))
            if ordinal == expected && ordinal < u64::MAX =>
        {
            Ok(ordinal)
        }
        (ProviderOrdinalMode::Legacy, RequiredWhenPresent::Missing) if expected < u64::MAX => {
            Ok(expected)
        }
        _ => Err(()),
    }
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
    (MIN_SUPPORTED_CODEX_CLI_MINOR..=MAX_SUPPORTED_CODEX_CLI_MINOR).contains(&minor)
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
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

fn first_valid_model<'a>(candidates: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    candidates.into_iter().flatten().find_map(|candidate| {
        let candidate = candidate.trim();
        valid_model_name(candidate).then(|| candidate.to_owned())
    })
}

fn parse_rollout_timestamp(timestamp: &str) -> Result<OffsetDateTime, ()> {
    OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| ())
}

fn oversized_record_is_ignorable(prefix: &[u8]) -> bool {
    const PREFIX_LIMIT: usize = 4 * 1024;
    const IGNORED_TYPES: [&[u8]; 3] = [
        b"\"type\":\"compacted\"",
        b"\"type\":\"response_item\"",
        b"\"payload\":{\"type\":\"user_message\"",
    ];
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
    let mut next_line_starts_file = true;
    for line in reader.split(b'\n') {
        let Ok(line) = line else {
            return false;
        };
        if line.is_empty() {
            continue;
        }
        let line_starts_file = next_line_starts_file;
        next_line_starts_file = false;
        let expected_ordinal = state.record_ordinal;
        if line.len() > MAX_ROLLOUT_LINE_BYTES {
            state.record_ordinal = expected_ordinal.saturating_add(1);
            complete = false;
            continue;
        }
        let header: RawRolloutHeader = match serde_json::from_slice(&line) {
            Ok(header) => header,
            Err(_) => {
                state.record_ordinal = expected_ordinal.saturating_add(1);
                complete = false;
                continue;
            }
        };
        let record_ordinal = match effective_record_ordinal(
            expected_ordinal,
            header.ordinal,
            line_starts_file,
            &header.record_type,
            &line,
            &mut state.provider_ordinal_mode,
        ) {
            Ok(record_ordinal) => record_ordinal,
            Err(()) => {
                state.record_ordinal = expected_ordinal.saturating_add(1);
                complete = false;
                continue;
            }
        };
        state.record_ordinal = record_ordinal.saturating_add(1);
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
                let _ = line.record_type;
                let line_timestamp = line.timestamp;
                if apply_session_metadata(&mut state, line.payload, &line_timestamp).is_err() {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                }
            }
            "inter_agent_communication_metadata" => {
                let Ok(line) = serde_json::from_slice::<RawInterAgentCommunicationLine>(&line)
                else {
                    if in_retention && !state.exclude_usage {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                    continue;
                };
                let _ = (line.timestamp, line.record_type);
                apply_inter_agent_boundary(&mut state, record_ordinal, line.payload);
                if state.marker_based_boundary && state.history_start_ordinal.is_some() {
                    state.exclude_usage = false;
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
                if state.active_model.is_some() {
                    state.last_turn_context_is_first = state.last_turn_context_ordinal.is_none();
                    state.last_turn_context_ordinal = Some(record_ordinal);
                } else {
                    state.last_turn_context_is_first = false;
                }
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
                let RawEventLine {
                    _ordinal: _,
                    timestamp: event_timestamp,
                    record_type,
                    payload,
                    model: root_model,
                } = line;
                let _ = (event_timestamp, record_type);
                if let RawEventPayload::TaskStarted {
                    turn_id,
                    model_context_window,
                    collaboration_mode_kind,
                    started_at,
                } = payload
                {
                    let _ = (
                        turn_id,
                        model_context_window,
                        collaboration_mode_kind,
                        started_at,
                    );
                    state.task_counter_reset_pending = state.lineage_mode == LineageMode::Root;
                    continue;
                }
                let RawEventPayload::TokenCount {
                    info,
                    turn_id,
                    id,
                    rate_limits,
                    model: payload_model,
                    model_name: payload_model_name,
                } = payload
                else {
                    continue;
                };
                let Some(info) = info else {
                    continue;
                };
                let _ = payload_model_name;
                let previous_active_model = state.active_model.clone();
                if let Some(model) = first_valid_model([
                    info.model.as_deref(),
                    info.model_name.as_deref(),
                    payload_model.as_deref(),
                    root_model.as_deref(),
                ]) {
                    state.active_model = Some(model);
                }
                let _ = (turn_id, id, &info.turn_id, &info.id);
                let raw_last = info.last_token_usage;
                let _ = (info.model_context_window, rate_limits);
                if !state.schema_supported {
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                    continue;
                }
                let timestamp_ns = i64::try_from(timestamp.unix_timestamp_nanos()).ok();
                let unowned_prefix = is_unowned_copied_prefix(&state, record_ordinal, timestamp_ns);
                let Some(current) = info.total_token_usage else {
                    if raw_last.is_some() && !unowned_prefix {
                        if in_retention {
                            mark_incomplete(days, day);
                        }
                        complete = false;
                    }
                    continue;
                };
                if current.validate().is_err() {
                    if unowned_prefix {
                        continue;
                    }
                    if in_retention {
                        mark_incomplete(days, day);
                    }
                    complete = false;
                    continue;
                }
                let last = match raw_last.map(TokenUsage::canonical_last).transpose() {
                    Ok(last) => last,
                    Err(()) if unowned_prefix => {
                        if record_precedes_parent_fork(&state, timestamp_ns) {
                            state.previous = None;
                        } else {
                            state.previous = Some(current);
                        }
                        continue;
                    }
                    Err(()) => {
                        if in_retention {
                            mark_incomplete(days, day);
                        }
                        complete = false;
                        continue;
                    }
                };
                let task_counter_reset = state.lineage_mode == LineageMode::Root
                    && state.task_counter_reset_pending
                    && state.previous.is_some_and(|previous| {
                        current.total != previous.total && last == Some(current)
                    });
                state.task_counter_reset_pending = false;
                let allow_strong_reset = task_counter_reset
                    || state.baseline_is_inherited == Some(true)
                        && state
                            .history_start_ordinal
                            .is_some_and(|history_start| history_start <= record_ordinal);
                let mut next_previous = current;
                let delta = match state.previous {
                    Some(previous)
                        if !allow_strong_reset
                            && last == Some(current)
                            && usage_is_at_or_below(current, previous) =>
                    {
                        state.active_model = previous_active_model;
                        next_previous = previous;
                        Ok(TokenUsage::default())
                    }
                    Some(_) if task_counter_reset => current.delta_from(TokenUsage::default()),
                    Some(previous) => cumulative_delta(current, last, previous, allow_strong_reset),
                    None if state
                        .history_start_ordinal
                        .is_some_and(|history_start| history_start <= record_ordinal)
                        && last == Some(current) =>
                    {
                        current.delta_from(TokenUsage::default())
                    }
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
                state.previous = Some(next_previous);
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

fn is_regular_file_without_intermediate_symlinks(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let Ok(root_metadata) = fs::metadata(root) else {
        return false;
    };
    if !root_metadata.file_type().is_dir() {
        return false;
    }

    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(component) = component else {
            return false;
        };
        current.push(component);
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            return false;
        };
        if components.peek().is_some() {
            if !metadata.file_type().is_dir() {
                return false;
            }
        } else {
            return metadata.file_type().is_file();
        }
    }
    false
}

const MAX_ROLLOUT_METADATA_PROBE_BYTES: u64 = 256 * 1024;

struct LeafSessionProbe {
    session_id: Option<String>,
    next_offset: u64,
    bytes_read: u64,
    complete: bool,
}

fn rollout_leaf_session_id(
    path: &Path,
    start_offset: u64,
    max_bytes: u64,
) -> Result<LeafSessionProbe, ()> {
    if start_offset >= MAX_ROLLOUT_METADATA_PROBE_BYTES || max_bytes == 0 {
        return Ok(LeafSessionProbe {
            session_id: None,
            next_offset: start_offset,
            bytes_read: 0,
            complete: start_offset >= MAX_ROLLOUT_METADATA_PROBE_BYTES,
        });
    }
    let mut file = fs::File::open(path).map_err(|_| ())?;
    let file_size = file.metadata().map_err(|_| ())?.len();
    file.seek(SeekFrom::Start(start_offset)).map_err(|_| ())?;
    let allowed_bytes = max_bytes.min(
        MAX_ROLLOUT_METADATA_PROBE_BYTES
            .checked_sub(start_offset)
            .ok_or(())?,
    );
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut next_offset = start_offset;
    let mut bytes_read = 0_u64;
    loop {
        if bytes_read >= allowed_bytes {
            return Ok(LeafSessionProbe {
                session_id: None,
                next_offset,
                bytes_read,
                complete: next_offset >= MAX_ROLLOUT_METADATA_PROBE_BYTES,
            });
        }
        line.clear();
        let read_limit = allowed_bytes.checked_sub(bytes_read).ok_or(())?;
        let bytes = Read::by_ref(&mut reader)
            .take(read_limit)
            .read_until(b'\n', &mut line)
            .map_err(|_| ())?;
        if bytes == 0 {
            return Ok(LeafSessionProbe {
                session_id: None,
                next_offset,
                bytes_read,
                complete: true,
            });
        }
        let bytes = u64::try_from(bytes).map_err(|_| ())?;
        bytes_read = bytes_read.checked_add(bytes).ok_or(())?;
        #[cfg(test)]
        REQUIRED_PARENT_PROBE_BYTES.with(|total| {
            total.set(total.get().saturating_add(bytes));
        });
        if line.len() > MAX_ROLLOUT_LINE_BYTES {
            return Ok(LeafSessionProbe {
                session_id: None,
                next_offset,
                bytes_read,
                complete: true,
            });
        }
        let record_end = next_offset.checked_add(bytes).ok_or(())?;
        let record_complete = line.ends_with(b"\n") || record_end >= file_size;
        if !record_complete {
            return Ok(LeafSessionProbe {
                session_id: None,
                next_offset,
                bytes_read,
                complete: record_end >= MAX_ROLLOUT_METADATA_PROBE_BYTES,
            });
        }
        next_offset = record_end;
        let Ok(header) = serde_json::from_slice::<RawRolloutHeader>(&line) else {
            return Ok(LeafSessionProbe {
                session_id: None,
                next_offset,
                bytes_read,
                complete: true,
            });
        };
        if header.record_type == "session_meta" {
            let Ok(metadata) = serde_json::from_slice::<RawSessionMetaLine>(&line) else {
                return Ok(LeafSessionProbe {
                    session_id: None,
                    next_offset,
                    bytes_read,
                    complete: true,
                });
            };
            return Ok(LeafSessionProbe {
                session_id: normalized_session_id(metadata.payload.id),
                next_offset,
                bytes_read,
                complete: true,
            });
        }
    }
}

#[cfg(test)]
thread_local! {
    static REQUIRED_PARENT_PROBE_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_required_parent_probe_bytes() {
    REQUIRED_PARENT_PROBE_BYTES.with(|total| total.set(0));
}

#[cfg(test)]
fn required_parent_probe_bytes() -> u64 {
    REQUIRED_PARENT_PROBE_BYTES.with(std::cell::Cell::get)
}

fn required_parent_session_ids(
    connection: &Connection,
    cutoff_modified_ns: i64,
) -> Result<BTreeSet<String>, ()> {
    connection
        .prepare(
            "SELECT DISTINCT parent_session_id FROM codex_usage_files
             WHERE parent_session_id IS NOT NULL AND modified_ns >= ?1",
        )
        .map_err(|_| ())?
        .query_map([cutoff_modified_ns], |row| row.get::<_, String>(0))
        .map_err(|_| ())?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| ())
}

fn load_required_parent_probe_cursor(connection: &Connection) -> Result<Option<(String, u64)>, ()> {
    let stored = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta
             WHERE key = 'required_parent_probe_cursor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    stored
        .map(|value| {
            serde_json::from_str::<(u8, String, u64)>(&value)
                .map_err(|_| ())
                .map(|(version, path, offset)| {
                    (version == REQUIRED_PARENT_PROBE_ORDER_VERSION).then_some((path, offset))
                })
        })
        .transpose()
        .map(Option::flatten)
}

fn store_required_parent_probe_cursor(
    connection: &Connection,
    cursor: Option<(&str, u64)>,
) -> Result<(), ()> {
    if let Some((path, offset)) = cursor {
        let value = serde_json::to_string(&(REQUIRED_PARENT_PROBE_ORDER_VERSION, path, offset))
            .map_err(|_| ())?;
        connection
            .execute(
                "INSERT INTO codex_usage_index_meta(key, value)
                 VALUES('required_parent_probe_cursor', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [value],
            )
            .map_err(|_| ())?;
    } else {
        connection
            .execute(
                "DELETE FROM codex_usage_index_meta
                 WHERE key = 'required_parent_probe_cursor'",
                [],
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

fn filename_matches_required_parent(path: &Path, required_parent_ids: &BTreeSet<String>) -> bool {
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(stem) = filename.strip_suffix(".jsonl") else {
        return false;
    };
    required_parent_ids
        .iter()
        .any(|parent_id| stem.ends_with(parent_id))
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
    accounting_ready: bool,
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
    lineage_mode: LineageMode,
    has_parent_dependency: bool,
    leaf_session_id: Option<String>,
}

impl StoredFileSummary {
    fn position_is_settled(&self, size: u64, today: Date) -> bool {
        (self.parsed_offset == size && self.completion_state.is_terminal())
            || (self.parsed_offset < size
                && self.completion_state.is_deferred()
                && self.deferred_until_day.is_some_and(|day| day > today))
    }

    fn needs_work(&self, identity: &str, size: u64, modified_ns: i64, today: Date) -> bool {
        self.parser_version != ROLLOUT_PARSER_VERSION
            || self.identity != identity
            || self.size != size
            || self.modified_ns != modified_ns
            || self.lineage_mode.needs_dependency_check()
            || (self.lineage_mode == LineageMode::ExplicitBoundary && self.has_parent_dependency)
            || !self.position_is_settled(size, today)
    }

    fn is_pending(&self, identity: &str, size: u64, modified_ns: i64, today: Date) -> bool {
        self.parser_version != ROLLOUT_PARSER_VERSION
            || self.identity != identity
            || self.size != size
            || self.modified_ns != modified_ns
            || !self.position_is_settled(size, today)
    }
}

fn rollout_work_priority(
    needs_work: bool,
    is_pending: bool,
    is_required_parent: bool,
    modified_ns: i64,
) -> (bool, bool, bool, std::cmp::Reverse<i64>) {
    (
        !needs_work,
        !is_pending,
        !is_required_parent,
        std::cmp::Reverse(modified_ns),
    )
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
enum PricingMode {
    Standard,
    Fast,
}

impl PricingMode {
    fn as_stored(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    fn from_stored(value: &str) -> Result<Self, ()> {
        match value {
            "standard" => Ok(Self::Standard),
            "fast" => Ok(Self::Fast),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ModelDayKey {
    day: Date,
    model: String,
    pricing_input_tokens: u64,
    pricing_mode: PricingMode,
}

#[derive(Clone, Debug)]
struct ModelDayDelta {
    usage: TokenUsage,
    complete: bool,
    observed_through: OffsetDateTime,
}

#[derive(Clone, Copy, Debug)]
struct TokenSnapshot {
    record_ordinal: u64,
    timestamp_ns: i64,
    usage: TokenUsage,
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

fn subtract_parent_snapshot(current: TokenUsage, baseline: TokenUsage) -> Result<TokenUsage, ()> {
    current.validate()?;
    baseline.validate()?;
    let adjusted = TokenUsage {
        input: current.input.saturating_sub(baseline.input),
        cached_input: current.cached_input.saturating_sub(baseline.cached_input),
        cache_write_input: current
            .cache_write_input
            .saturating_sub(baseline.cache_write_input),
        output: current.output.saturating_sub(baseline.output),
        reasoning_output: current
            .reasoning_output
            .saturating_sub(baseline.reasoning_output),
        total: current
            .input
            .saturating_sub(baseline.input)
            .checked_add(current.output.saturating_sub(baseline.output))
            .ok_or(())?,
    };
    adjusted.validate()?;
    Ok(adjusted)
}

fn cumulative_delta(
    current: TokenUsage,
    last: Option<TokenUsage>,
    previous: TokenUsage,
    allow_strong_reset: bool,
) -> Result<TokenUsage, ()> {
    current.delta_from(previous).or_else(|_| {
        if allow_strong_reset && last == Some(current) {
            current.delta_from(TokenUsage::default())
        } else {
            Err(())
        }
    })
}

fn usage_is_at_or_below(current: TokenUsage, watermark: TokenUsage) -> bool {
    current.input <= watermark.input
        && current.cached_input <= watermark.cached_input
        && current.cache_write_input <= watermark.cache_write_input
        && current.output <= watermark.output
        && current.reasoning_output <= watermark.reasoning_output
}

fn add_model_day_delta(
    rows: &mut BTreeMap<ModelDayKey, ModelDayDelta>,
    timestamp: OffsetDateTime,
    model: Option<&str>,
    delta: TokenUsage,
    pricing_input_tokens: u64,
    pricing_mode: PricingMode,
) -> Result<(), ()> {
    if delta.total == 0 {
        return Ok(());
    }
    let key = ModelDayKey {
        day: utc_ranking_day(timestamp),
        model: model.unwrap_or(UNKNOWN_MODEL).to_owned(),
        pricing_input_tokens,
        pricing_mode,
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
        pricing_mode: PricingMode::Standard,
    };
    let row = rows.entry(key).or_insert(ModelDayDelta {
        usage: TokenUsage::default(),
        complete: false,
        observed_through: timestamp,
    });
    row.complete = false;
    row.observed_through = row.observed_through.max(timestamp);
}

struct FastTurnIndex<'a> {
    referenced_turn_days: &'a mut BTreeSet<(String, Date)>,
    turns: &'a BTreeMap<String, Option<String>>,
    detail_cutoff: Date,
}

struct IndexedLineOutput<'a> {
    rows: &'a mut BTreeMap<ModelDayKey, ModelDayDelta>,
    snapshots: &'a mut Vec<TokenSnapshot>,
}

struct IndexLineContext {
    cutoff: Date,
    today: Date,
    record_ordinal: u64,
    line_starts_file: bool,
}

fn process_index_line(
    line: &[u8],
    context: IndexLineContext,
    state: &mut RolloutScanState,
    output: &mut IndexedLineOutput<'_>,
    fast_turn_index: &mut FastTurnIndex<'_>,
) -> IndexLineOutcome {
    let IndexLineContext {
        cutoff,
        today,
        record_ordinal,
        line_starts_file,
    } = context;
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
    let record_ordinal = match effective_record_ordinal(
        record_ordinal,
        header.ordinal,
        line_starts_file,
        &header.record_type,
        line,
        &mut state.provider_ordinal_mode,
    ) {
        Ok(record_ordinal) => record_ordinal,
        Err(()) => {
            debug_parser_failure("record_ordinal", None);
            return IndexLineOutcome::Processed(false);
        }
    };
    state.record_ordinal = record_ordinal;
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
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("session_meta_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = line.record_type;
            let line_timestamp = line.timestamp;
            if apply_session_metadata(state, line.payload, &line_timestamp).is_err() {
                if in_retention {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("session_metadata", in_retention.then_some(day));
            }
            state.schema_supported || state.exclude_usage
        }
        "inter_agent_communication_metadata" => {
            let Ok(line) = serde_json::from_slice::<RawInterAgentCommunicationLine>(line) else {
                if in_retention && !state.exclude_usage {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("inter_agent_metadata_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = (line.timestamp, line.record_type);
            apply_inter_agent_boundary(state, record_ordinal, line.payload);
            true
        }
        "turn_context" => {
            let Ok(line) = serde_json::from_slice::<RawTurnContextLine>(line) else {
                if in_retention {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("turn_context_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let _ = (line.timestamp, line.record_type);
            state.active_model =
                valid_model_name(&line.payload.model).then_some(line.payload.model);
            if in_retention && state.active_model.is_none() {
                mark_model_day_incomplete(output.rows, timestamp, None);
                debug_parser_failure("model_name", Some(day));
                return IndexLineOutcome::Processed(false);
            }
            if state.active_model.is_some() {
                state.last_turn_context_is_first = state.last_turn_context_ordinal.is_none();
                state.last_turn_context_ordinal = Some(record_ordinal);
            } else {
                state.last_turn_context_is_first = false;
            }
            true
        }
        "event_msg" => {
            let Ok(line) = serde_json::from_slice::<RawEventLine>(line) else {
                if in_retention {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("event_schema", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            let RawEventLine {
                _ordinal: _,
                timestamp: event_timestamp,
                record_type,
                payload,
                model: root_model,
            } = line;
            let _ = (event_timestamp, record_type);
            if let RawEventPayload::TaskStarted {
                turn_id,
                model_context_window,
                collaboration_mode_kind,
                started_at,
            } = payload
            {
                let _ = (model_context_window, collaboration_mode_kind, started_at);
                state.active_turn_id = turn_id.filter(|value| valid_turn_id(value));
                state.task_counter_reset_pending = state.lineage_mode == LineageMode::Root;
                return IndexLineOutcome::Processed(true);
            }
            let RawEventPayload::TokenCount {
                info,
                turn_id,
                id,
                rate_limits,
                model: payload_model,
                model_name: payload_model_name,
            } = payload
            else {
                return IndexLineOutcome::Processed(true);
            };
            let Some(info) = info else {
                return IndexLineOutcome::Processed(true);
            };
            let _ = payload_model_name;
            let previous_active_model = state.active_model.clone();
            if let Some(model) = first_valid_model([
                info.model.as_deref(),
                info.model_name.as_deref(),
                payload_model.as_deref(),
                root_model.as_deref(),
            ]) {
                state.active_model = Some(model);
            }
            let pricing_turn_id = turn_id
                .as_deref()
                .or(id.as_deref())
                .or(info.turn_id.as_deref())
                .or(info.id.as_deref())
                .filter(|value| valid_turn_id(value))
                .or(state.active_turn_id.as_deref());
            let raw_last = info.last_token_usage;
            let raw_current = info.total_token_usage;
            let _ = (info.model_context_window, rate_limits);
            if !state.schema_supported {
                if in_retention {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("schema_not_initialized", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            }
            let Ok(snapshot_timestamp_ns) = i64::try_from(timestamp.unix_timestamp_nanos()) else {
                state.parser_error_seen = true;
                return IndexLineOutcome::Processed(false);
            };
            let unowned_prefix =
                is_unowned_copied_prefix(state, record_ordinal, Some(snapshot_timestamp_ns));
            let Some(raw_current) = raw_current else {
                if raw_last.is_none() || unowned_prefix {
                    return IndexLineOutcome::Processed(true);
                }
                if in_retention && !state.exclude_usage {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure("missing_total_token_usage", in_retention.then_some(day));
                return IndexLineOutcome::Processed(false);
            };
            if raw_current.validate().is_err() {
                if unowned_prefix {
                    return IndexLineOutcome::Processed(true);
                }
                if in_retention && !state.exclude_usage {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                }
                debug_parser_failure(
                    "invalid_total_token_arithmetic",
                    in_retention.then_some(day),
                );
                return IndexLineOutcome::Processed(false);
            }
            if state
                .snapshot_last_timestamp_ns
                .is_some_and(|previous| snapshot_timestamp_ns < previous)
            {
                state.snapshot_timestamp_regressed = true;
            }
            state.snapshot_last_timestamp_ns = Some(snapshot_timestamp_ns);
            let snapshot_index = if in_retention {
                output.snapshots.push(TokenSnapshot {
                    record_ordinal,
                    timestamp_ns: snapshot_timestamp_ns,
                    usage: raw_current,
                });
                Some(output.snapshots.len() - 1)
            } else {
                None
            };
            let last = match raw_last.map(TokenUsage::canonical_last).transpose() {
                Ok(last) => last,
                Err(()) if unowned_prefix => {
                    if record_precedes_parent_fork(state, Some(snapshot_timestamp_ns)) {
                        state.previous = None;
                    } else {
                        state.previous = Some(raw_current);
                    }
                    return IndexLineOutcome::Processed(true);
                }
                Err(()) => {
                    if in_retention && !state.exclude_usage {
                        mark_model_day_incomplete(
                            output.rows,
                            timestamp,
                            state.active_model.as_deref(),
                        );
                    }
                    debug_parser_failure(
                        "invalid_last_token_arithmetic",
                        in_retention.then_some(day),
                    );
                    return IndexLineOutcome::Processed(false);
                }
            };
            if state.lineage_mode == LineageMode::Discovering
                && state.marker_based_boundary
                && state.marker_local_confirmation.is_none()
                && state
                    .history_start_ordinal
                    .is_some_and(|start| record_ordinal >= start)
            {
                state.marker_local_confirmation =
                    Some(state.parent_baseline.is_some_and(|baseline| {
                        last.and_then(|last| raw_current.delta_from(last).ok()) == Some(baseline)
                    }));
            }
            let current = if state.lineage_mode == LineageMode::ParentResolved {
                let parent_adjusted = match state.parent_baseline {
                    Some(parent_baseline) => subtract_parent_snapshot(raw_current, parent_baseline),
                    None if state.parent_dependency_key.is_some() => Ok(raw_current),
                    None => Err(()),
                };
                match parent_adjusted {
                    Ok(adjusted) => adjusted,
                    Err(()) => {
                        state.exclude_usage = true;
                        return IndexLineOutcome::Processed(false);
                    }
                }
            } else {
                raw_current
            };
            if unowned_prefix {
                if record_precedes_parent_fork(state, Some(snapshot_timestamp_ns)) {
                    state.previous = None;
                } else {
                    state.previous = Some(current);
                }
                return IndexLineOutcome::Processed(true);
            }
            let task_counter_reset = state.lineage_mode == LineageMode::Root
                && state.task_counter_reset_pending
                && state.previous.is_some_and(|previous| {
                    current.total != previous.total && last == Some(current)
                });
            state.task_counter_reset_pending = false;
            let allow_strong_reset = task_counter_reset
                || state.lineage_mode == LineageMode::ParentResolved
                || (state.baseline_is_inherited == Some(true)
                    && state
                        .history_start_ordinal
                        .is_some_and(|history_start| history_start <= record_ordinal));
            let mut next_previous = current;
            let delta = match state.previous {
                Some(previous)
                    if !allow_strong_reset
                        && last == Some(current)
                        && usage_is_at_or_below(current, previous) =>
                {
                    state.active_model = previous_active_model;
                    next_previous = previous;
                    if let Some(snapshot_index) = snapshot_index {
                        output.snapshots[snapshot_index].usage = previous;
                    }
                    Ok(TokenUsage::default())
                }
                Some(_) if task_counter_reset => current.delta_from(TokenUsage::default()),
                Some(previous) => cumulative_delta(current, last, previous, allow_strong_reset),
                None if state
                    .history_start_ordinal
                    .is_some_and(|history_start| history_start <= record_ordinal)
                    && last == Some(current) =>
                {
                    current.delta_from(TokenUsage::default())
                }
                None if state.lineage_mode == LineageMode::Independent && last == Some(current) => {
                    current.delta_from(TokenUsage::default())
                }
                None if state.lineage_mode == LineageMode::ParentResolved => {
                    current.delta_from(TokenUsage::default())
                }
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
                        mark_model_day_incomplete(
                            output.rows,
                            timestamp,
                            state.active_model.as_deref(),
                        );
                    }
                    debug_parser_failure("baseline", in_retention.then_some(day));
                    return IndexLineOutcome::Processed(false);
                }
            };
            state.previous = Some(next_previous);
            if !in_retention
                || (state.exclude_usage
                    && !matches!(
                        state.lineage_mode,
                        LineageMode::Independent | LineageMode::ParentResolved
                    )
                    && !(state.lineage_mode == LineageMode::Discovering
                        && state.marker_based_boundary
                        && state
                            .history_start_ordinal
                            .is_some_and(|start| record_ordinal >= start)))
                || state
                    .history_start_ordinal
                    .is_some_and(|history_start| record_ordinal < history_start)
            {
                return IndexLineOutcome::Processed(true);
            }
            if day >= fast_turn_index.detail_cutoff
                && let Some(turn_id) = pricing_turn_id
            {
                fast_turn_index
                    .referenced_turn_days
                    .insert((turn_id.to_owned(), day));
            }
            let Ok(delta) = delta else {
                mark_model_day_incomplete(output.rows, timestamp, state.active_model.as_deref());
                debug_parser_failure("cumulative_token_arithmetic", Some(day));
                return IndexLineOutcome::Processed(false);
            };
            let fast_turn = pricing_turn_id.and_then(|turn_id| fast_turn_index.turns.get(turn_id));
            let pricing_mode = if fast_turn.is_some() {
                PricingMode::Fast
            } else {
                PricingMode::Standard
            };
            let pricing_model = fast_turn
                .and_then(|model| model.as_deref())
                .filter(|model| model_has_fast_multiplier(model, day))
                .or(state.active_model.as_deref());
            match add_model_day_delta(
                output.rows,
                timestamp,
                pricing_model,
                delta,
                last.unwrap_or(delta).input,
                pricing_mode,
            ) {
                Ok(()) => true,
                Err(()) => {
                    mark_model_day_incomplete(
                        output.rows,
                        timestamp,
                        state.active_model.as_deref(),
                    );
                    debug_parser_failure("model_day_token_arithmetic", Some(day));
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
    if source_version == 6 {
        transaction
            .execute_batch(
                "DROP TABLE IF EXISTS codex_usage_file_turns;
                 DROP TABLE IF EXISTS codex_usage_fast_turns;
                 UPDATE codex_usage_files SET active_turn_id = NULL;
                 DELETE FROM codex_usage_index_meta WHERE key = 'fast_turn_fingerprint';",
            )
            .map_err(|_| ())?;
    } else if source_version > 0 && source_version < 7 {
        transaction
            .execute_batch(
                "DROP TABLE IF EXISTS codex_usage_token_snapshots;
                 DROP TABLE IF EXISTS codex_usage_file_turns;
                 DROP TABLE IF EXISTS codex_usage_file_days;
                 DROP TABLE IF EXISTS codex_usage_file_model_days;
                 DROP TABLE IF EXISTS codex_usage_files;
                 DROP TABLE IF EXISTS codex_usage_fast_turns;
                 DELETE FROM codex_usage_index_meta WHERE key = 'fast_turn_fingerprint';",
            )
            .map_err(|_| ())?;
    }
    if source_version > 0 {
        let account_day_columns = table_columns(&transaction, "codex_account_usage_days")?;
        let account_meta_columns = table_columns(&transaction, "codex_account_usage_meta")?;
        let account_tables_exist = match (
            account_day_columns.is_empty(),
            account_meta_columns.is_empty(),
        ) {
            (true, true) => false,
            (false, false) => true,
            _ => return Err(()),
        };
        let has_current_account_shape = account_day_columns == ["day", "tokens", "observed_at"]
            && account_meta_columns == ["singleton", "refreshed_at"];
        if account_tables_exist && !has_current_account_shape {
            if account_day_columns != ["day", "tokens"]
                || account_meta_columns != ["singleton", "observed_at"]
            {
                return Err(());
            }
            let account_day_count = transaction
                .query_row("SELECT COUNT(*) FROM codex_account_usage_days", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|_| ())?;
            let has_refresh_timestamp = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM codex_account_usage_meta WHERE singleton = 1
                     )",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|_| ())?
                == 1;
            if account_day_count > 0 && !has_refresh_timestamp {
                return Err(());
            }
            transaction
                .execute_batch(
                    "ALTER TABLE codex_account_usage_days
                       RENAME TO codex_account_usage_days_legacy;
                     ALTER TABLE codex_account_usage_meta
                       RENAME TO codex_account_usage_meta_legacy;
                     CREATE TABLE codex_account_usage_meta (
                       singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
                       refreshed_at TEXT NOT NULL
                     );
                     CREATE TABLE codex_account_usage_days (
                       day TEXT PRIMARY KEY NOT NULL,
                       tokens INTEGER NOT NULL,
                       observed_at TEXT NOT NULL
                     );
                     INSERT INTO codex_account_usage_meta(singleton, refreshed_at)
                       SELECT singleton, observed_at
                       FROM codex_account_usage_meta_legacy;
                     INSERT INTO codex_account_usage_days(day, tokens, observed_at)
                       SELECT day, tokens, (
                         SELECT observed_at
                         FROM codex_account_usage_meta_legacy
                         WHERE singleton = 1
                       )
                       FROM codex_account_usage_days_legacy;
                     DROP TABLE codex_account_usage_days_legacy;
                     DROP TABLE codex_account_usage_meta_legacy;",
                )
                .map_err(|_| ())?;
        }
    }
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
               refreshed_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_account_usage_days (
               day TEXT PRIMARY KEY NOT NULL,
               tokens INTEGER NOT NULL,
               observed_at TEXT NOT NULL
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
               active_turn_id TEXT,
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
               previous_total INTEGER,
               lineage_mode TEXT NOT NULL DEFAULT 'unknown',
               leaf_session_id TEXT,
               parent_session_id TEXT,
               parent_identity_explicit INTEGER NOT NULL DEFAULT 0,
               fork_timestamp_ns INTEGER,
               embedded_ancestor_seen INTEGER NOT NULL DEFAULT 0,
               lineage_invalid INTEGER NOT NULL DEFAULT 0,
               parent_dependency_key TEXT,
               parent_baseline_input INTEGER,
               parent_baseline_cached_input INTEGER,
               parent_baseline_cache_write_input INTEGER,
               parent_baseline_output INTEGER,
               parent_baseline_reasoning_output INTEGER,
               parent_baseline_total INTEGER,
               last_turn_context_is_first INTEGER NOT NULL DEFAULT 0,
               last_turn_context_ordinal INTEGER,
               marker_based_boundary INTEGER NOT NULL DEFAULT 0,
               marker_candidate_invalidated INTEGER NOT NULL DEFAULT 0,
               marker_local_confirmation INTEGER,
               accounting_ready INTEGER NOT NULL DEFAULT 0,
               parser_error_seen INTEGER NOT NULL DEFAULT 0,
               snapshot_last_timestamp_ns INTEGER,
               snapshot_timestamp_regressed INTEGER NOT NULL DEFAULT 0,
               task_counter_reset_pending INTEGER NOT NULL DEFAULT 0,
               provider_ordinal_mode TEXT NOT NULL DEFAULT 'unknown'
             );
             CREATE TABLE IF NOT EXISTS codex_usage_token_snapshots (
               path TEXT NOT NULL,
               record_ordinal INTEGER NOT NULL,
               timestamp_ns INTEGER NOT NULL,
               input_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               cache_write_input_tokens INTEGER NOT NULL,
               output_tokens INTEGER NOT NULL,
               reasoning_output_tokens INTEGER NOT NULL,
               total_tokens INTEGER NOT NULL,
               PRIMARY KEY (path, record_ordinal),
               FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS codex_usage_file_model_days (
               path TEXT NOT NULL,
               day TEXT NOT NULL,
               model TEXT NOT NULL,
               pricing_input_tokens INTEGER NOT NULL,
               pricing_mode TEXT NOT NULL CHECK(pricing_mode IN ('standard', 'fast')),
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
               PRIMARY KEY (path, day, model, pricing_input_tokens, pricing_mode),
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
             );
             CREATE TABLE IF NOT EXISTS codex_usage_file_turns (
               path TEXT NOT NULL,
               turn_id TEXT NOT NULL,
               day TEXT NOT NULL,
               PRIMARY KEY (path, turn_id, day),
               FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS codex_usage_fast_turns (
               turn_id TEXT PRIMARY KEY NOT NULL,
               model TEXT
             );",
        )
        .map_err(|_| ())?;
    if source_version > 0 && source_version < 5 {
        transaction
            .execute_batch(
                "DELETE FROM codex_usage_files;
                 DELETE FROM codex_usage_fast_turns;
                 DELETE FROM codex_usage_index_meta WHERE key = 'fast_turn_fingerprint';",
            )
            .map_err(|_| ())?;
    }
    let file_columns = table_columns(&transaction, "codex_usage_files")?;
    if !file_columns.iter().any(|column| column == "active_turn_id") {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files ADD COLUMN active_turn_id TEXT",
                [],
            )
            .map_err(|_| ())?;
    }
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
    if !file_columns
        .iter()
        .any(|column| column == "task_counter_reset_pending")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files
                 ADD COLUMN task_counter_reset_pending INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|_| ())?;
    }
    if !file_columns
        .iter()
        .any(|column| column == "provider_ordinal_mode")
    {
        transaction
            .execute(
                "ALTER TABLE codex_usage_files
                 ADD COLUMN provider_ordinal_mode TEXT NOT NULL DEFAULT 'unknown'",
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
            "CREATE INDEX IF NOT EXISTS codex_usage_file_turns_by_turn_id
             ON codex_usage_file_turns(turn_id)",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "CREATE INDEX IF NOT EXISTS codex_usage_files_by_leaf_session
             ON codex_usage_files(leaf_session_id)",
            [],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "CREATE INDEX IF NOT EXISTS codex_usage_snapshots_by_path_timestamp
             ON codex_usage_token_snapshots(path, timestamp_ns, record_ordinal)",
            [],
        )
        .map_err(|_| ())?;
    if source_version < 7 {
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
    }
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
                    observed_tokens, pricing_basis, pricing_fingerprint, pricing_mode
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
                    PricingMode::from_stored(&row.get::<_, String>(13)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
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
    for (
        _,
        path,
        day,
        model,
        pricing_input_tokens,
        usage,
        stored_basis,
        stored_rule_fingerprint,
        pricing_mode,
    ) in rows
    {
        let day = parse_ranking_day(&day)?;
        let rule_fingerprint = pricing_rule_fingerprint(
            manifest,
            &model,
            day,
            usage,
            pricing_input_tokens,
            pricing_mode.clone(),
        );
        if stored_rule_fingerprint.as_deref() != Some(rule_fingerprint.as_str()) {
            let cost = price_usage_tier_with_manifest(
                manifest,
                &model,
                day,
                usage,
                pricing_input_tokens,
                pricing_mode.clone(),
            );
            transaction
                .execute(
                    "UPDATE codex_usage_file_model_days
                     SET cost_usd = ?1, pricing_basis = ?2, pricing_fingerprint = ?3
                     WHERE path = ?4 AND day = ?5 AND model = ?6
                       AND pricing_input_tokens = ?7 AND pricing_mode = ?8",
                    params![
                        cost,
                        manifest.basis.as_str(),
                        rule_fingerprint,
                        path.as_str(),
                        day.to_string(),
                        model.as_str(),
                        to_i64(pricing_input_tokens)?,
                        pricing_mode.as_stored(),
                    ],
                )
                .map_err(|_| ())?;
            affected_file_days.insert((path.clone(), day));
        } else if stored_basis.as_deref() != Some(manifest.basis.as_str()) {
            transaction
                .execute(
                    "UPDATE codex_usage_file_model_days
                     SET pricing_basis = ?1
                     WHERE path = ?2 AND day = ?3 AND model = ?4
                       AND pricing_input_tokens = ?5 AND pricing_mode = ?6",
                    params![
                        manifest.basis.as_str(),
                        path.as_str(),
                        day.to_string(),
                        model.as_str(),
                        to_i64(pricing_input_tokens)?,
                        pricing_mode.as_stored(),
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
    history_cutoff: Date,
    detail_cutoff: Date,
    today: Date,
    cutoff_modified_ns: i64,
) -> Result<bool, ()> {
    let retention_end_ns = i64::try_from(
        (today + Duration::days(1))
            .midnight()
            .assume_utc()
            .unix_timestamp_nanos(),
    )
    .map_err(|_| ())?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_token_snapshots
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_token_snapshots
               WHERE timestamp_ns < ?1 OR timestamp_ns >= ?2 LIMIT ?3
             )",
            params![
                cutoff_modified_ns,
                retention_end_ns,
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let snapshots_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_model_days
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_model_days
               WHERE day < ?1 OR day > ?2 LIMIT ?3
             )",
            params![
                detail_cutoff.to_string(),
                today.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let model_days_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "UPDATE codex_usage_file_days
             SET priced_tokens = 0,
                 cost_usd = 0.0,
                 complete = 0,
                 priced_observed_through = NULL,
                 pricing_fingerprint = NULL
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_days
               WHERE day < ?1 AND day >= ?2
                 AND (priced_tokens != 0 OR cost_usd != 0.0 OR complete != 0
                      OR priced_observed_through IS NOT NULL
                      OR pricing_fingerprint IS NOT NULL)
               LIMIT ?3
             )",
            params![
                detail_cutoff.to_string(),
                history_cutoff.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let cost_details_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_days
             WHERE rowid IN (
               SELECT rowid FROM codex_usage_file_days
               WHERE day < ?1 OR day > ?2 LIMIT ?3
             )",
            params![
                history_cutoff.to_string(),
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
                 AND NOT EXISTS (
                   SELECT 1 FROM codex_usage_files child
                   WHERE child.path != f.path
                     AND child.parent_session_id = f.leaf_session_id
                     AND child.modified_ns >= ?1
                 )
               LIMIT ?4
             )",
            params![
                cutoff_modified_ns,
                history_cutoff.to_string(),
                today.to_string(),
                i64::try_from(PRUNE_ROWS_PER_PASS).map_err(|_| ())?
            ],
        )
        .map_err(|_| ())?;
    let files_complete = transaction.changes() < PRUNE_ROWS_PER_PASS as u64;
    transaction.commit().map_err(|_| ())?;
    Ok(snapshots_complete
        && model_days_complete
        && cost_details_complete
        && file_days_complete
        && files_complete)
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
         FROM codex_usage_file_model_days d
         JOIN codex_usage_files f ON f.path = d.path
         WHERE day >= ?1 AND day <= ?2 AND cost_usd IS NULL
           AND f.accounting_ready = 1 AND f.usage_excluded = 0
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
                    completion_state, parser_version, deferred_until_day, lineage_mode,
                    parent_dependency_key IS NOT NULL, leaf_session_id
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
                    lineage_mode: LineageMode::from_stored(&row.get::<_, String>(8)?),
                    has_parent_dependency: row.get(9)?,
                    leaf_session_id: row.get(10)?,
                },
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())
}

fn promote_compatible_parser_rows(connection: &Connection) -> Result<usize, ()> {
    // Parser 19 adds CLI 0.151 support. It does not change rows that parser 18
    // accepted. Promote only rows with complete, included evidence.
    connection
        .execute(
            "UPDATE codex_usage_files
             SET parser_version = ?1
             WHERE parser_version = ?2
               AND completion_state = 'complete'
               AND parsed_offset = size_bytes
               AND accounting_ready = 1
               AND usage_excluded = 0
               AND schema_supported = 1
               AND parser_error_seen = 0
               AND lineage_invalid = 0
               AND snapshot_timestamp_regressed = 0
               AND deferred_until_day IS NULL
               AND lineage_mode IN (
                 'root', 'explicit-boundary', 'independent', 'parent-resolved'
               )
               AND provider_ordinal_mode IN ('legacy', 'provider')",
            params![ROLLOUT_PARSER_VERSION, COMPATIBLE_ROLLOUT_PARSER_VERSION],
        )
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
                    parsed_prefix_anchor, deferred_until_day, active_turn_id,
                    lineage_mode, leaf_session_id, parent_session_id, parent_identity_explicit,
                    fork_timestamp_ns,
                    embedded_ancestor_seen, lineage_invalid, parent_dependency_key,
                    parent_baseline_input, parent_baseline_cached_input,
                    parent_baseline_cache_write_input, parent_baseline_output,
                    parent_baseline_reasoning_output, parent_baseline_total,
                    last_turn_context_is_first, last_turn_context_ordinal,
                    marker_based_boundary, marker_candidate_invalidated,
                    marker_local_confirmation
                    , accounting_ready, parser_error_seen, snapshot_last_timestamp_ns,
                    snapshot_timestamp_regressed, task_counter_reset_pending,
                    provider_ordinal_mode
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
                let parent_baseline = row
                    .get::<_, Option<i64>>(34)?
                    .map(|total| {
                        Ok::<TokenUsage, rusqlite::Error>(TokenUsage {
                            input: from_i64(row.get(29)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cached_input: from_i64(row.get(30)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_write_input: from_i64(row.get(31)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            output: from_i64(row.get(32)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            reasoning_output: from_i64(row.get(33)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            total: from_i64(total).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    })
                    .transpose()?;
                if parent_baseline.is_some_and(|usage| usage.validate().is_err()) {
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
                    accounting_ready: row.get(40)?,
                    parser_state: RolloutScanState {
                        active_model: row.get(5)?,
                        active_turn_id: row.get(20)?,
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
                        lineage_mode: LineageMode::from_stored(&row.get::<_, String>(21)?),
                        leaf_session_id: row.get(22)?,
                        parent_session_id: row.get(23)?,
                        parent_identity_explicit: row.get(24)?,
                        fork_timestamp_ns: row.get(25)?,
                        embedded_ancestor_seen: row.get(26)?,
                        lineage_invalid: row.get(27)?,
                        parent_dependency_key: row.get(28)?,
                        parent_baseline,
                        last_turn_context_is_first: row.get(35)?,
                        last_turn_context_ordinal: row
                            .get::<_, Option<i64>>(36)?
                            .map(from_i64)
                            .transpose()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        marker_based_boundary: row.get(37)?,
                        marker_candidate_invalidated: row.get(38)?,
                        marker_local_confirmation: row.get(39)?,
                        parser_error_seen: row.get(41)?,
                        snapshot_last_timestamp_ns: row.get(42)?,
                        snapshot_timestamp_regressed: row.get(43)?,
                        task_counter_reset_pending: row.get(44)?,
                        provider_ordinal_mode: ProviderOrdinalMode::from_stored(
                            &row.get::<_, String>(45)?,
                        )
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
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

struct FileProgressCommit {
    turn_days: BTreeSet<(String, Date)>,
    rows: BTreeMap<ModelDayKey, ModelDayDelta>,
    snapshots: Vec<TokenSnapshot>,
    replace_existing_usage: bool,
    detail_cutoff: Date,
}

fn commit_file_progress(
    connection: &Connection,
    path: &str,
    cursor: &FileCursor,
    commit: FileProgressCommit,
) -> Result<(), ()> {
    let FileProgressCommit {
        turn_days,
        rows,
        snapshots,
        replace_existing_usage,
        detail_cutoff,
    } = commit;
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
               parsed_prefix_anchor, deferred_until_day, active_turn_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
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
               deferred_until_day=excluded.deferred_until_day,
               active_turn_id=excluded.active_turn_id",
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
                cursor.parser_state.active_turn_id,
            ],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "UPDATE codex_usage_files SET
               lineage_mode = ?2, leaf_session_id = ?3, parent_session_id = ?4,
               parent_identity_explicit = ?5, fork_timestamp_ns = ?6,
               embedded_ancestor_seen = ?7, lineage_invalid = ?8,
               parent_dependency_key = ?9, parent_baseline_input = ?10,
               parent_baseline_cached_input = ?11,
               parent_baseline_cache_write_input = ?12, parent_baseline_output = ?13,
               parent_baseline_reasoning_output = ?14, parent_baseline_total = ?15,
               last_turn_context_is_first = ?16, last_turn_context_ordinal = ?17,
               marker_based_boundary = ?18, marker_candidate_invalidated = ?19,
               marker_local_confirmation = ?20, accounting_ready = ?21,
               parser_error_seen = ?22, snapshot_last_timestamp_ns = ?23,
               snapshot_timestamp_regressed = ?24,
               task_counter_reset_pending = ?25,
               provider_ordinal_mode = ?26
             WHERE path = ?1",
            params![
                path,
                cursor.parser_state.lineage_mode.as_stored(),
                cursor.parser_state.leaf_session_id,
                cursor.parser_state.parent_session_id,
                cursor.parser_state.parent_identity_explicit,
                cursor.parser_state.fork_timestamp_ns,
                cursor.parser_state.embedded_ancestor_seen,
                cursor.parser_state.lineage_invalid,
                cursor.parser_state.parent_dependency_key,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.input))
                    .transpose()?,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.cached_input))
                    .transpose()?,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.cache_write_input))
                    .transpose()?,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.output))
                    .transpose()?,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.reasoning_output))
                    .transpose()?,
                cursor
                    .parser_state
                    .parent_baseline
                    .map(|usage| to_i64(usage.total))
                    .transpose()?,
                cursor.parser_state.last_turn_context_is_first,
                cursor
                    .parser_state
                    .last_turn_context_ordinal
                    .map(to_i64)
                    .transpose()?,
                cursor.parser_state.marker_based_boundary,
                cursor.parser_state.marker_candidate_invalidated,
                cursor.parser_state.marker_local_confirmation,
                cursor.accounting_ready,
                cursor.parser_state.parser_error_seen,
                cursor.parser_state.snapshot_last_timestamp_ns,
                cursor.parser_state.snapshot_timestamp_regressed,
                cursor.parser_state.task_counter_reset_pending,
                cursor.parser_state.provider_ordinal_mode.as_stored(),
            ],
        )
        .map_err(|_| ())?;
    if replace_existing_usage {
        transaction
            .execute(
                "DELETE FROM codex_usage_file_model_days WHERE path = ?1",
                [path],
            )
            .map_err(|_| ())?;
        transaction
            .execute("DELETE FROM codex_usage_file_days WHERE path = ?1", [path])
            .map_err(|_| ())?;
        transaction
            .execute("DELETE FROM codex_usage_file_turns WHERE path = ?1", [path])
            .map_err(|_| ())?;
    }
    for snapshot in snapshots {
        transaction
            .execute(
                "INSERT INTO codex_usage_token_snapshots(
                   path, record_ordinal, timestamp_ns, input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(path, record_ordinal) DO UPDATE SET
                   timestamp_ns=excluded.timestamp_ns, input_tokens=excluded.input_tokens,
                   cached_input_tokens=excluded.cached_input_tokens,
                   cache_write_input_tokens=excluded.cache_write_input_tokens,
                   output_tokens=excluded.output_tokens,
                   reasoning_output_tokens=excluded.reasoning_output_tokens,
                   total_tokens=excluded.total_tokens",
                params![
                    path,
                    to_i64(snapshot.record_ordinal)?,
                    snapshot.timestamp_ns,
                    to_i64(snapshot.usage.input)?,
                    to_i64(snapshot.usage.cached_input)?,
                    to_i64(snapshot.usage.cache_write_input)?,
                    to_i64(snapshot.usage.output)?,
                    to_i64(snapshot.usage.reasoning_output)?,
                    to_i64(snapshot.usage.total)?,
                ],
            )
            .map_err(|_| ())?;
    }
    for (turn_id, day) in turn_days {
        transaction
            .execute(
                "INSERT OR IGNORE INTO codex_usage_file_turns(path, turn_id, day)
                 VALUES(?1, ?2, ?3)",
                params![path, turn_id, day.to_string()],
            )
            .map_err(|_| ())?;
    }
    let mut file_days = BTreeMap::<Date, FileDayDelta>::new();
    for (key, delta) in rows {
        let retains_cost_detail = key.day >= detail_cutoff;
        let cost = if retains_cost_detail {
            manifest.and_then(|manifest| {
                price_usage_tier_with_manifest(
                    manifest,
                    &key.model,
                    key.day,
                    delta.usage,
                    key.pricing_input_tokens,
                    key.pricing_mode.clone(),
                )
            })
        } else {
            None
        };
        let pricing_fingerprint = if retains_cost_detail {
            manifest.map(|manifest| {
                pricing_rule_fingerprint(
                    manifest,
                    &key.model,
                    key.day,
                    delta.usage,
                    key.pricing_input_tokens,
                    key.pricing_mode.clone(),
                )
            })
        } else {
            None
        };
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
        if retains_cost_detail {
            transaction
                .execute(
                    "INSERT INTO codex_usage_file_model_days(
                       path, day, model, pricing_input_tokens, pricing_mode,
                       input_tokens, cached_input_tokens,
                       cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                       observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                       complete, observed_through
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                     ON CONFLICT(path, day, model, pricing_input_tokens, pricing_mode) DO UPDATE SET
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
                        key.pricing_mode.as_stored(),
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

#[derive(Clone, Debug)]
struct ResolvedParentSnapshot {
    baseline: TokenUsage,
    dependency_key: String,
}

fn private_dependency_key(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in parts.iter().flat_map(|part| part.bytes().chain([0])) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn parent_lineage_is_acyclic(
    connection: &Connection,
    child_path: &str,
    parent_path: &str,
) -> Result<bool, ()> {
    let child_session_id = connection
        .query_row(
            "SELECT leaf_session_id FROM codex_usage_files WHERE path = ?1",
            [child_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|_| ())?
        .flatten();
    let mut current_path = parent_path.to_owned();
    let mut visited = BTreeSet::from([child_path.to_owned()]);
    for _ in 0..64 {
        if !visited.insert(current_path.clone()) {
            return Ok(false);
        }
        let next_session_id = connection
            .query_row(
                "SELECT parent_session_id FROM codex_usage_files WHERE path = ?1",
                [current_path.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| ())?
            .flatten();
        let Some(next_session_id) = next_session_id else {
            return Ok(true);
        };
        if child_session_id.as_deref() == Some(next_session_id.as_str()) {
            return Ok(false);
        }
        let paths = connection
            .prepare(
                "SELECT path FROM codex_usage_files
                 WHERE leaf_session_id = ?1 LIMIT 2",
            )
            .map_err(|_| ())?
            .query_map([next_session_id], |row| row.get::<_, String>(0))
            .map_err(|_| ())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ())?;
        let [next_path] = paths.as_slice() else {
            return Ok(paths.is_empty());
        };
        current_path.clone_from(next_path);
    }
    Ok(false)
}

fn resolve_parent_snapshot(
    connection: &Connection,
    child_path: &str,
    parent_session_id: &str,
    fork_timestamp_ns: i64,
) -> Result<Option<ResolvedParentSnapshot>, ()> {
    let candidates = connection
        .prepare(
            "SELECT path, file_identity, size_bytes, modified_ns, parsed_offset,
                    parsed_prefix_anchor, completion_state, parser_version, schema_supported,
                    lineage_invalid, parser_error_seen, snapshot_timestamp_regressed
             FROM codex_usage_files
             WHERE leaf_session_id = ?1 AND path != ?2 LIMIT 2",
        )
        .map_err(|_| ())?
        .query_map(params![parent_session_id, child_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, bool>(8)?,
                row.get::<_, bool>(9)?,
                row.get::<_, bool>(10)?,
                row.get::<_, bool>(11)?,
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    let [
        (
            path,
            identity,
            size,
            modified_ns,
            parsed_offset,
            anchor,
            completion,
            parser,
            schema,
            lineage_invalid,
            parser_error_seen,
            snapshot_timestamp_regressed,
        ),
    ] = candidates.as_slice()
    else {
        return Ok(None);
    };
    let source_is_complete = *parsed_offset == *size && completion == "complete";
    let source_is_stable_partial = *parsed_offset >= 0
        && *parsed_offset < *size
        && matches!(completion.as_str(), "indexing" | "deferred");
    if *size < 0
        || (!source_is_complete && !source_is_stable_partial)
        || *parser != ROLLOUT_PARSER_VERSION
        || !schema
        || *lineage_invalid
        || *parser_error_seen
        || *snapshot_timestamp_regressed
        || !parent_lineage_is_acyclic(connection, child_path, path)?
    {
        return Ok(None);
    }
    let source_path = Path::new(path);
    let Ok(metadata) = fs::metadata(source_path) else {
        return Ok(None);
    };
    if file_identity(&metadata) != *identity
        || to_i64(metadata.len())? != *size
        || file_modified_ns(&metadata)? != *modified_ns
        || parsed_prefix_anchor(source_path, from_i64(*parsed_offset)?)?.as_ref() != anchor.as_ref()
    {
        return Ok(None);
    }
    let bounds = connection
        .query_row(
            "SELECT MIN(timestamp_ns), MAX(timestamp_ns)
             FROM codex_usage_token_snapshots WHERE path = ?1",
            [path.as_str()],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|_| ())?;
    let (Some(first_snapshot), Some(last_snapshot)) = bounds else {
        return Ok(None);
    };
    if first_snapshot > fork_timestamp_ns
        || (!source_is_complete && last_snapshot < fork_timestamp_ns)
    {
        return Ok(None);
    }
    let selected = connection
        .query_row(
            "SELECT record_ordinal, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens
             FROM codex_usage_token_snapshots
             WHERE path = ?1 AND timestamp_ns <= ?2
             ORDER BY timestamp_ns DESC, record_ordinal DESC LIMIT 1",
            params![path, fork_timestamp_ns],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    TokenUsage {
                        input: from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cached_input: from_i64(row.get(2)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cache_write_input: from_i64(row.get(3)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        output: from_i64(row.get(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        reasoning_output: from_i64(row.get(5)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        total: from_i64(row.get(6)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    },
                ))
            },
        )
        .optional()
        .map_err(|_| ())?;
    let Some((record_ordinal, baseline)) = selected else {
        return Ok(None);
    };
    baseline.validate()?;
    let dependency_key = private_dependency_key(&[
        path,
        identity,
        &size.to_string(),
        &modified_ns.to_string(),
        &parsed_offset.to_string(),
        anchor.as_deref().unwrap_or(""),
        completion,
        &record_ordinal.to_string(),
        &baseline.input.to_string(),
        &baseline.cached_input.to_string(),
        &baseline.cache_write_input.to_string(),
        &baseline.output.to_string(),
        &baseline.reasoning_output.to_string(),
        &baseline.total.to_string(),
    ]);
    Ok(Some(ResolvedParentSnapshot {
        baseline,
        dependency_key,
    }))
}

fn reset_dependent_accounting(
    connection: &Connection,
    path: &str,
    resolved: Option<&ResolvedParentSnapshot>,
) -> Result<(), ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_model_days WHERE path = ?1",
            [path],
        )
        .map_err(|_| ())?;
    transaction
        .execute("DELETE FROM codex_usage_file_days WHERE path = ?1", [path])
        .map_err(|_| ())?;
    transaction
        .execute("DELETE FROM codex_usage_file_turns WHERE path = ?1", [path])
        .map_err(|_| ())?;
    if let Some(resolved) = resolved {
        transaction
            .execute(
                "UPDATE codex_usage_files SET
                   parsed_offset = 0, parsed_prefix_anchor = NULL,
                   completion_state = 'indexing', deferred_until_day = NULL,
                   active_model = NULL, active_turn_id = NULL, record_ordinal = 0,
                   usage_excluded = 0, accounting_ready = 0, previous_input = NULL,
                   previous_cached_input = NULL, previous_cache_write_input = NULL,
                   previous_output = NULL, previous_reasoning_output = NULL,
                   previous_total = NULL, lineage_mode = 'parent-resolved',
                   snapshot_last_timestamp_ns = NULL,
                   snapshot_timestamp_regressed = 0,
                   task_counter_reset_pending = 0,
                   provider_ordinal_mode = 'unknown',
                   parent_dependency_key = ?2, parent_baseline_input = ?3,
                   parent_baseline_cached_input = ?4,
                   parent_baseline_cache_write_input = ?5, parent_baseline_output = ?6,
                   parent_baseline_reasoning_output = ?7, parent_baseline_total = ?8,
                   last_turn_context_is_first = 0, marker_local_confirmation = NULL
                 WHERE path = ?1",
                params![
                    path,
                    resolved.dependency_key,
                    to_i64(resolved.baseline.input)?,
                    to_i64(resolved.baseline.cached_input)?,
                    to_i64(resolved.baseline.cache_write_input)?,
                    to_i64(resolved.baseline.output)?,
                    to_i64(resolved.baseline.reasoning_output)?,
                    to_i64(resolved.baseline.total)?,
                ],
            )
            .map_err(|_| ())?;
    } else {
        transaction
            .execute(
                "UPDATE codex_usage_files SET usage_excluded = 1, accounting_ready = 0,
                   previous_input = NULL,
                   previous_cached_input = NULL, previous_cache_write_input = NULL,
                   previous_output = NULL, previous_reasoning_output = NULL,
                   previous_total = NULL,
                   lineage_mode = 'unresolved', parent_dependency_key = NULL,
                   parent_baseline_input = NULL, parent_baseline_cached_input = NULL,
                   parent_baseline_cache_write_input = NULL, parent_baseline_output = NULL,
                   parent_baseline_reasoning_output = NULL, parent_baseline_total = NULL,
                   last_turn_context_is_first = 0, marker_local_confirmation = NULL,
                   task_counter_reset_pending = 0
                 WHERE path = ?1",
                [path],
            )
            .map_err(|_| ())?;
    }
    transaction.commit().map_err(|_| ())
}

struct FileIndexContext<'a> {
    cutoff: Date,
    detail_cutoff: Date,
    today: Date,
    fast_turns: &'a BTreeMap<String, Option<String>>,
    dependency_parent_paths: &'a BTreeSet<String>,
}

fn index_file(
    connection: &Connection,
    path: &Path,
    context: &FileIndexContext<'_>,
    started: Instant,
    max_millis: u128,
    remaining_bytes: &mut u64,
) -> Result<bool, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    let path_value = path.to_string_lossy().into_owned();
    let identity = file_identity(&metadata);
    let size = metadata.len();
    let modified_ns = file_modified_ns(&metadata)?;
    let cutoff_modified_ns = i64::try_from(
        context
            .cutoff
            .midnight()
            .assume_utc()
            .unix_timestamp_nanos(),
    )
    .map_err(|_| ())?;
    if modified_ns < cutoff_modified_ns && !context.dependency_parent_paths.contains(&path_value) {
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
    let mut stored = if rebuild { None } else { stored };
    if let Some(cursor) = &stored
        && (cursor.parser_state.lineage_mode.needs_dependency_check()
            || (cursor.parser_state.lineage_mode == LineageMode::ExplicitBoundary
                && cursor.parser_state.parent_dependency_key.is_some()))
    {
        let parent_confirmed_boundary =
            cursor.parser_state.lineage_mode == LineageMode::ExplicitBoundary;
        let resolved = if cursor.parser_state.lineage_invalid {
            None
        } else {
            cursor
                .parser_state
                .parent_session_id
                .as_deref()
                .zip(cursor.parser_state.fork_timestamp_ns)
                .map(|(parent, fork)| {
                    resolve_parent_snapshot(connection, &path_value, parent, fork)
                })
                .transpose()?
                .flatten()
        };
        let dependency_is_current = resolved.as_ref().is_some_and(|resolved| {
            matches!(
                cursor.parser_state.lineage_mode,
                LineageMode::ParentResolved | LineageMode::ExplicitBoundary
            ) && cursor.parser_state.parent_dependency_key.as_deref()
                == Some(resolved.dependency_key.as_str())
        });
        if !dependency_is_current {
            if parent_confirmed_boundary {
                reset_file(connection, &path_value)?;
                stored = None;
            } else {
                reset_dependent_accounting(connection, &path_value, resolved.as_ref())?;
                stored = load_file_cursor(connection, &path_value)?;
            }
        }
    }
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
        accounting_ready: false,
    });
    cursor.identity = identity;
    cursor.size = size;
    cursor.modified_ns = modified_ns;
    if cursor.parsed_offset < size
        && (matches!(
            cursor.parser_state.lineage_mode,
            LineageMode::Independent | LineageMode::ParentResolved
        ) || (cursor.parser_state.lineage_mode == LineageMode::ExplicitBoundary
            && cursor.parser_state.marker_based_boundary))
    {
        cursor.accounting_ready = false;
    }
    let mut file = fs::File::open(path).map_err(|_| ())?;
    file.seek(SeekFrom::Start(cursor.parsed_offset))
        .map_err(|_| ())?;
    let mut reader = BufReader::new(file);
    let mut rows = BTreeMap::new();
    let mut turn_ids = BTreeSet::new();
    let mut snapshots = Vec::new();
    let mut replace_existing_usage = false;
    let mut parser_complete =
        !cursor.completion_state.has_parser_error() && !cursor.parser_state.parser_error_seen;
    let mut discarding_overlong_line =
        cursor.completion_state == FileCompletionState::DiscardingOverlongLine;
    let mut deferred_until_day = None;
    loop {
        if cursor.parser_state.exclude_usage && !cursor.parser_state.schema_supported {
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
                cursor.parser_state.record_ordinal = cursor
                    .parser_state
                    .record_ordinal
                    .checked_add(1)
                    .ok_or(())?;
                if !oversized_record_is_ignorable(&line) {
                    parser_complete = false;
                    cursor.parser_state.parser_error_seen = true;
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
            let line_starts_file = cursor.parsed_offset == 0;
            let mut output = IndexedLineOutput {
                rows: &mut rows,
                snapshots: &mut snapshots,
            };
            let mut fast_turn_index = FastTurnIndex {
                referenced_turn_days: &mut turn_ids,
                turns: context.fast_turns,
                detail_cutoff: context.detail_cutoff,
            };
            match process_index_line(
                &line,
                IndexLineContext {
                    cutoff: context.cutoff,
                    today: context.today,
                    record_ordinal,
                    line_starts_file,
                },
                &mut cursor.parser_state,
                &mut output,
                &mut fast_turn_index,
            ) {
                IndexLineOutcome::Processed(processed) => {
                    cursor.parser_state.record_ordinal = cursor
                        .parser_state
                        .record_ordinal
                        .checked_add(1)
                        .ok_or(())?;
                    parser_complete &= processed;
                    cursor.parser_state.parser_error_seen |= !processed;
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
    if cursor.completion_state == FileCompletionState::Complete
        && cursor.parser_state.lineage_mode == LineageMode::Discovering
    {
        cursor.accounting_ready = false;
        replace_existing_usage = cursor.parser_state.marker_candidate_invalidated;
        let marker_parent_resolution = if cursor.parser_state.marker_based_boundary
            && !cursor.parser_state.embedded_ancestor_seen
        {
            cursor
                .parser_state
                .parent_session_id
                .as_deref()
                .zip(cursor.parser_state.fork_timestamp_ns)
                .zip(cursor.parser_state.parent_baseline)
                .map(|((parent, fork), marker_baseline)| {
                    resolve_parent_snapshot(connection, &path_value, parent, fork).map(|resolved| {
                        resolved.filter(|resolved| resolved.baseline == marker_baseline)
                    })
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let marker_parent_confirmed = marker_parent_resolution.is_some();
        let marker_confirmed = cursor.parser_state.embedded_ancestor_seen
            || cursor.parser_state.marker_local_confirmation == Some(true)
            || marker_parent_confirmed;
        if cursor.parser_state.marker_based_boundary
            && cursor.parser_state.history_start_ordinal.is_some()
            && !marker_confirmed
        {
            cursor.parser_state.history_start_ordinal = None;
            cursor.parser_state.marker_based_boundary = false;
            cursor.parser_state.marker_candidate_invalidated = false;
            cursor.parser_state.parent_baseline = None;
            cursor.parser_state.marker_local_confirmation = None;
            replace_existing_usage = true;
        }
        if !cursor.parser_state.marker_based_boundary
            && cursor.parser_state.history_start_ordinal.is_some()
        {
            rows.clear();
            turn_ids.clear();
            replace_existing_usage = true;
            cursor.parser_state.active_model = None;
            cursor.parser_state.active_turn_id = None;
            cursor.parser_state.previous = None;
            cursor.parser_state.record_ordinal = 0;
            cursor.parser_state.provider_ordinal_mode = ProviderOrdinalMode::Unknown;
            cursor.parser_state.snapshot_last_timestamp_ns = None;
            cursor.parser_state.snapshot_timestamp_regressed = false;
            cursor.parser_state.task_counter_reset_pending = false;
            cursor.parser_state.lineage_mode = LineageMode::ExplicitBoundary;
            cursor.parser_state.exclude_usage = false;
            cursor.parsed_offset = 0;
            cursor.completion_state = FileCompletionState::Indexing;
        } else if cursor.parser_state.marker_based_boundary
            && cursor.parser_state.history_start_ordinal.is_some()
            && marker_confirmed
        {
            let parent_confirmed_only = cursor.parser_state.marker_local_confirmation != Some(true)
                && !cursor.parser_state.embedded_ancestor_seen;
            if parent_confirmed_only {
                let resolved = marker_parent_resolution.as_ref().ok_or(())?;
                cursor.parser_state.parent_dependency_key = Some(resolved.dependency_key.clone());
                cursor.parser_state.parent_baseline = Some(resolved.baseline);
            } else {
                cursor.parser_state.parent_dependency_key = None;
                cursor.parser_state.parent_baseline = None;
            }
            cursor.parser_state.lineage_mode = LineageMode::ExplicitBoundary;
            cursor.parser_state.exclude_usage = false;
            rows.clear();
            turn_ids.clear();
            replace_existing_usage = true;
            cursor.parser_state.active_model = None;
            cursor.parser_state.active_turn_id = None;
            cursor.parser_state.previous = None;
            cursor.parser_state.record_ordinal = 0;
            cursor.parser_state.provider_ordinal_mode = ProviderOrdinalMode::Unknown;
            cursor.parser_state.snapshot_last_timestamp_ns = None;
            cursor.parser_state.snapshot_timestamp_regressed = false;
            cursor.parser_state.task_counter_reset_pending = false;
            cursor.parser_state.marker_based_boundary = false;
            cursor.parser_state.marker_candidate_invalidated = true;
            cursor.parser_state.marker_local_confirmation = None;
            cursor.parsed_offset = 0;
            cursor.completion_state = FileCompletionState::Indexing;
        } else {
            cursor.parser_state.active_model = None;
            cursor.parser_state.active_turn_id = None;
            cursor.parser_state.previous = None;
            cursor.parser_state.record_ordinal = 0;
            cursor.parser_state.provider_ordinal_mode = ProviderOrdinalMode::Unknown;
            cursor.parser_state.last_turn_context_is_first = false;
            cursor.parser_state.snapshot_last_timestamp_ns = None;
            cursor.parser_state.snapshot_timestamp_regressed = false;
            cursor.parser_state.task_counter_reset_pending = false;
            if !parser_complete || cursor.parser_state.lineage_invalid {
                rows.clear();
                turn_ids.clear();
                replace_existing_usage = true;
                cursor.parser_state.lineage_mode = LineageMode::Unresolved;
                cursor.parser_state.exclude_usage = true;
            } else if !cursor.parser_state.embedded_ancestor_seen {
                rows.clear();
                turn_ids.clear();
                cursor.parser_state.lineage_mode = LineageMode::Independent;
                cursor.parser_state.exclude_usage = false;
                cursor.parsed_offset = 0;
                cursor.completion_state = FileCompletionState::Indexing;
            } else {
                rows.clear();
                turn_ids.clear();
                let resolved = cursor
                    .parser_state
                    .parent_session_id
                    .as_deref()
                    .zip(cursor.parser_state.fork_timestamp_ns)
                    .map(|(parent, fork)| {
                        resolve_parent_snapshot(connection, &path_value, parent, fork)
                    })
                    .transpose()?
                    .flatten();
                if let Some(resolved) = resolved {
                    cursor.parser_state.lineage_mode = LineageMode::ParentResolved;
                    cursor.parser_state.parent_dependency_key = Some(resolved.dependency_key);
                    cursor.parser_state.parent_baseline = Some(resolved.baseline);
                    cursor.parser_state.exclude_usage = false;
                    cursor.parsed_offset = 0;
                    cursor.completion_state = FileCompletionState::Indexing;
                } else {
                    cursor.parser_state.lineage_mode = LineageMode::Unresolved;
                    cursor.parser_state.exclude_usage = true;
                    replace_existing_usage = true;
                    cursor.completion_state = FileCompletionState::Indexing;
                }
            }
        }
    }
    if cursor.completion_state.is_terminal()
        && cursor.parser_state.lineage_mode == LineageMode::ExplicitBoundary
        && !cursor.parser_state.marker_based_boundary
        && cursor.parser_state.marker_candidate_invalidated
        && cursor.parser_state.history_start_ordinal.is_some()
    {
        cursor.parser_state.marker_based_boundary = true;
        cursor.parser_state.marker_candidate_invalidated = false;
    }
    if cursor.parser_state.lineage_mode == LineageMode::ParentResolved {
        let current_dependency = cursor
            .parser_state
            .parent_session_id
            .as_deref()
            .zip(cursor.parser_state.fork_timestamp_ns)
            .map(|(parent, fork)| resolve_parent_snapshot(connection, &path_value, parent, fork))
            .transpose()?
            .flatten();
        let dependency_is_current = current_dependency.as_ref().is_some_and(|resolved| {
            cursor.parser_state.parent_dependency_key.as_deref()
                == Some(resolved.dependency_key.as_str())
        });
        if !dependency_is_current || !parser_complete {
            rows.clear();
            turn_ids.clear();
            replace_existing_usage = true;
            cursor.parser_state.lineage_mode = LineageMode::Unresolved;
            cursor.parser_state.exclude_usage = true;
            cursor.parser_state.parent_dependency_key = None;
            cursor.parser_state.parent_baseline = None;
            cursor.accounting_ready = false;
            cursor.completion_state = FileCompletionState::Indexing;
        } else if cursor.completion_state == FileCompletionState::Complete {
            cursor.parser_state.exclude_usage = false;
            cursor.accounting_ready = true;
        }
    } else if cursor.completion_state == FileCompletionState::Complete
        && cursor.parser_state.lineage_mode == LineageMode::Independent
    {
        cursor.parser_state.exclude_usage = false;
        cursor.accounting_ready = true;
    } else if (cursor.parser_state.lineage_mode == LineageMode::Root
        || (cursor.completion_state.is_terminal()
            && cursor.parser_state.lineage_mode == LineageMode::ExplicitBoundary))
        && !cursor.parser_state.exclude_usage
    {
        cursor.accounting_ready = true;
    }
    cursor.parsed_prefix_anchor = parsed_prefix_anchor(path, cursor.parsed_offset)?;
    commit_file_progress(
        connection,
        &path_value,
        &cursor,
        FileProgressCommit {
            turn_days: turn_ids,
            rows,
            snapshots,
            replace_existing_usage,
            detail_cutoff: context.detail_cutoff,
        },
    )?;
    Ok(cursor.completion_state.is_terminal() || is_deferred)
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
    let mut stored_pricing_bases = BTreeMap::<Date, Option<String>>::new();
    let mut basis_statement = connection
        .prepare(
            "SELECT d.day,
                    CASE WHEN COUNT(*) = COUNT(d.pricing_basis)
                                  AND COUNT(DISTINCT d.pricing_basis) = 1
                         THEN MIN(d.pricing_basis) END
             FROM codex_usage_file_model_days d
             JOIN codex_usage_files f ON f.path = d.path
             WHERE f.parser_version = ?1 AND f.accounting_ready = 1
               AND f.usage_excluded = 0 AND d.day >= ?2 AND d.day <= ?3
               AND d.complete = 1 AND d.cost_usd IS NOT NULL
             GROUP BY d.day ORDER BY d.day",
        )
        .map_err(|_| ())?;
    for row in basis_statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|_| ())?
    {
        let (day, basis) = row.map_err(|_| ())?;
        stored_pricing_bases.insert(parse_ranking_day(&day)?, basis);
    }
    drop(basis_statement);
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
             WHERE f.parser_version = ?1 AND f.accounting_ready = 1
               AND f.usage_excluded = 0 AND d.day >= ?2 AND d.day <= ?3
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
                let mut cost = (priced_tokens > 0)
                    .then(|| row.get::<_, f64>(3))
                    .transpose()?;
                let mut complete = row.get::<_, bool>(4)?;
                let observed_through = OffsetDateTime::parse(&row.get::<_, String>(5)?, &Rfc3339)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let mut priced_observed_through = row
                    .get::<_, Option<String>>(6)?
                    .map(|value| OffsetDateTime::parse(&value, &Rfc3339))
                    .transpose()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let pricing_basis = stored_pricing_bases.get(&day).cloned().flatten();
                let priced_tokens = if priced_tokens > 0 && pricing_basis.is_none() {
                    cost = None;
                    complete = false;
                    priced_observed_through = None;
                    0
                } else {
                    priced_tokens
                };
                Ok((
                    day,
                    LocalUsageDay {
                        observed_tokens,
                        priced_tokens,
                        api_equivalent_cost_usd: cost,
                        modeled: false,
                        complete,
                        observed_through: Some(observed_through),
                        priced_observed_through,
                        pricing_basis,
                    },
                ))
            },
        )
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())?;
    let (latest_pending_modified_ns, latest_incomplete_modified_ns, has_excluded_files) =
        connection
            .query_row(
                "SELECT
               MAX(CASE WHEN completion_state NOT IN (
                                'complete', 'error', 'deferred', 'deferred-error'
                              )
                        THEN modified_ns END),
               MAX(CASE WHEN completion_state IN ('error', 'deferred-error')
                                  OR usage_excluded = 1
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
    let detail_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    let top_model_usage = read_top_model_usage(connection, detail_cutoff, today)?;
    Ok(LocalUsageObservation {
        daily: rows,
        top_model_usage,
        pricing_basis,
        scan_status,
        has_excluded_usage: has_excluded_files,
        latest_pending_modified_at: parse_modified_at(latest_pending_modified_ns)?
            .into_iter()
            .chain(latest_pending_modified_hint)
            .max(),
        latest_incomplete_modified_at: parse_modified_at(latest_incomplete_modified_ns)?,
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
             WHERE f.parser_version = ?1 AND f.accounting_ready = 1
               AND f.usage_excluded = 0 AND d.day >= ?2 AND d.day <= ?3
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

fn load_stored_fast_turns(connection: &Connection) -> Result<BTreeMap<String, Option<String>>, ()> {
    connection
        .prepare("SELECT turn_id, model FROM codex_usage_fast_turns ORDER BY turn_id")
        .map_err(|_| ())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())
}

fn prune_stored_fast_turns_outside_detail_window(
    connection: &Connection,
    detail_cutoff: Date,
    today: Date,
) -> Result<(), ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM codex_usage_file_turns WHERE day < ?1 OR day > ?2",
            params![detail_cutoff.to_string(), today.to_string()],
        )
        .map_err(|_| ())?;
    let deleted = transaction
        .execute(
            "DELETE FROM codex_usage_fast_turns
             WHERE NOT EXISTS (
               SELECT 1 FROM codex_usage_file_turns turns
               WHERE turns.turn_id = codex_usage_fast_turns.turn_id
             )",
            [],
        )
        .map_err(|_| ())?;
    if deleted > 0 {
        transaction
            .execute(
                "DELETE FROM codex_usage_index_meta WHERE key = 'fast_turn_fingerprint'",
                [],
            )
            .map_err(|_| ())?;
    }
    transaction.commit().map_err(|_| ())
}

fn reconcile_fast_turn_evidence(
    connection: &Connection,
    evidence: &FastTurnEvidence,
) -> Result<(), ()> {
    let current = connection
        .query_row(
            "SELECT value FROM codex_usage_index_meta WHERE key = 'fast_turn_fingerprint'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if current.as_deref() == Some(evidence.fingerprint.as_str()) {
        return Ok(());
    }
    let stored = load_stored_fast_turns(connection)?;
    let changed_turn_ids = stored
        .keys()
        .chain(evidence.turns.keys())
        .filter(|turn_id| stored.get(*turn_id) != evidence.turns.get(*turn_id))
        .cloned()
        .collect::<BTreeSet<_>>();
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    for turn_id in changed_turn_ids {
        transaction
            .execute(
                "DELETE FROM codex_usage_files
                 WHERE path IN (
                   SELECT path FROM codex_usage_file_turns WHERE turn_id = ?1
                 )",
                [turn_id.as_str()],
            )
            .map_err(|_| ())?;
        if let Some(model) = evidence.turns.get(&turn_id) {
            transaction
                .execute(
                    "INSERT INTO codex_usage_fast_turns(turn_id, model) VALUES(?1, ?2)
                     ON CONFLICT(turn_id) DO UPDATE SET model = excluded.model",
                    params![turn_id, model],
                )
                .map_err(|_| ())?;
        } else {
            transaction
                .execute(
                    "DELETE FROM codex_usage_fast_turns WHERE turn_id = ?1",
                    [turn_id.as_str()],
                )
                .map_err(|_| ())?;
        }
    }
    transaction
        .execute(
            "INSERT INTO codex_usage_index_meta(key, value)
             VALUES('fast_turn_fingerprint', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [evidence.fingerprint.as_str()],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

fn index_local_usage_with_budget(
    database_path: &Path,
    home: &Path,
    now: OffsetDateTime,
    budget: ScanBudget,
) -> Option<LocalUsageObservation> {
    static INDEX_PASS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _index_pass = INDEX_PASS_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let max_bytes = budget.max_bytes.min(MAX_ROLLOUT_SCAN_BYTES);
    let max_file_bytes = budget.max_file_bytes.min(MAX_ROLLOUT_FILE_SCAN_BYTES);
    let max_discovery_millis = budget.max_discovery_millis.min(MAX_ROLLOUT_SCAN_MILLIS);
    let max_parse_millis = budget.max_parse_millis.min(MAX_ROLLOUT_SCAN_MILLIS);
    debug_usage_event(&format!(
        "scan_pass_started max_bytes={max_bytes} max_file_bytes={max_file_bytes} max_discovery_millis={max_discovery_millis} max_parse_millis={max_parse_millis}"
    ));
    let mut connection = Connection::open(database_path).ok()?;
    ensure_index_schema(&mut connection, Some(database_path)).ok()?;
    let today = utc_ranking_day(now);
    let cutoff = today - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
    let detail_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    prune_stored_fast_turns_outside_detail_window(&connection, detail_cutoff, today).ok()?;
    let fast_turns = match load_fast_turn_evidence(home, detail_cutoff, today) {
        Ok(fresh) => {
            reconcile_fast_turn_evidence(&connection, &fresh).ok()?;
            fresh.turns
        }
        Err(()) => load_stored_fast_turns(&connection).ok()?,
    };
    let promoted_rows = promote_compatible_parser_rows(&connection).ok()?;
    if promoted_rows > 0 {
        debug_usage_event(&format!(
            "compatible_parser_rows_promoted source_version={} target_version={} rows={promoted_rows}",
            COMPATIBLE_ROLLOUT_PARSER_VERSION, ROLLOUT_PARSER_VERSION
        ));
    }
    // Trace evidence and rollout bytes are independent local inputs. A large
    // read-only trace database must not consume the bounded rollout budget.
    let pass_started = Instant::now();
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).ok()?;
    let retention_complete = prune_expired_index(
        &connection,
        cutoff,
        detail_cutoff,
        today,
        cutoff_modified_ns,
    )
    .ok()?;
    let pricing_complete =
        retention_complete && reprice_index(&connection, detail_cutoff, today).ok()?;
    let summaries_complete = pricing_complete && ensure_file_day_summaries(&connection).is_ok();
    if !retention_complete || !pricing_complete || !summaries_complete {
        debug_usage_event(&format!(
            "scan_pass_completed stop=maintenance elapsed_ms={} retention_complete={retention_complete} pricing_complete={pricing_complete} summaries_complete={summaries_complete}",
            pass_started.elapsed().as_millis()
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
    let discovery_started = Instant::now();
    let rollout_roots = [home.join("sessions"), home.join("archived_sessions")];
    let canonical_home = fs::canonicalize(home).ok();
    let mut trusted_rollout_roots = Vec::new();
    let mut files = Vec::new();
    let mut found_root = false;
    let mut traversal_complete = true;
    for root in &rollout_roots {
        if fs::symlink_metadata(root).is_err() {
            continue;
        }
        found_root = true;
        if !fs::metadata(root).is_ok_and(|metadata| metadata.file_type().is_dir()) {
            traversal_complete = false;
            continue;
        }
        let Ok(canonical_root) = fs::canonicalize(root) else {
            traversal_complete = false;
            continue;
        };
        if !canonical_home
            .as_ref()
            .is_some_and(|home| canonical_root.starts_with(home))
        {
            traversal_complete = false;
            continue;
        }
        traversal_complete &=
            collect_rollout_files(root, &mut files, discovery_started, max_discovery_millis)
                .is_ok();
        trusted_rollout_roots.push(canonical_root);
    }
    if !found_root {
        return None;
    }
    files.sort();
    let stored_files = load_file_summaries(&connection).ok()?;
    let required_parent_ids = required_parent_session_ids(&connection, cutoff_modified_ns).ok()?;
    let mut discovered_files = Vec::with_capacity(files.len());
    let mut accepted_canonical_rollout_paths = BTreeSet::new();
    let mut duplicate_rollout_paths = BTreeSet::new();
    for path in files {
        if discovery_started.elapsed().as_millis() >= max_discovery_millis {
            traversal_complete = false;
            break;
        }
        if !rollout_roots
            .iter()
            .any(|root| is_regular_file_without_intermediate_symlinks(root, &path))
        {
            traversal_complete = false;
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            traversal_complete = false;
            continue;
        };
        if !metadata.file_type().is_file() {
            traversal_complete = false;
            continue;
        }
        let Ok(canonical_path) = fs::canonicalize(&path) else {
            traversal_complete = false;
            continue;
        };
        if !trusted_rollout_roots
            .iter()
            .any(|root| canonical_path.starts_with(root))
        {
            traversal_complete = false;
            continue;
        }
        if !accepted_canonical_rollout_paths.insert(canonical_path) {
            duplicate_rollout_paths.insert(path.to_string_lossy().into_owned());
            continue;
        }
        let Ok(modified_ns) = file_modified_ns(&metadata) else {
            traversal_complete = false;
            continue;
        };
        discovered_files.push((file_identity(&metadata), modified_ns, metadata.len(), path));
    }

    let mut dependency_parent_paths = BTreeSet::new();
    let mut probe_candidates = Vec::new();
    let mut unresolved_parent_ids = required_parent_ids.clone();
    if !required_parent_ids.is_empty() {
        for (index, (identity, modified_ns, size, path)) in discovered_files.iter().enumerate() {
            let filename_matches_parent =
                filename_matches_required_parent(path, &unresolved_parent_ids);
            if *modified_ns >= cutoff_modified_ns {
                if filename_matches_parent {
                    dependency_parent_paths.insert(path.to_string_lossy().into_owned());
                }
                continue;
            }
            let path_value = path.to_string_lossy();
            let stored = stored_files.get(path_value.as_ref()).filter(|stored| {
                stored.identity == *identity
                    && stored.size == *size
                    && stored.modified_ns == *modified_ns
            });
            if let Some(session_id) = stored
                .and_then(|stored| stored.leaf_session_id.as_ref())
                .filter(|session_id| required_parent_ids.contains(*session_id))
            {
                unresolved_parent_ids.remove(session_id);
                dependency_parent_paths.insert(path_value.into_owned());
            } else if stored.is_none()
                || stored.is_some_and(|stored| stored.parser_version != ROLLOUT_PARSER_VERSION)
            {
                probe_candidates.push(index);
            }
        }
    }
    if unresolved_parent_ids.is_empty() {
        probe_candidates.clear();
    } else {
        // Codex rollout names end with the session identifier. Use that fact only
        // to order candidates. The bounded metadata probe below still verifies
        // the trusted leaf session identifier before it accepts a parent.
        probe_candidates.sort_by_key(|candidate| {
            !filename_matches_required_parent(
                &discovered_files[*candidate].3,
                &unresolved_parent_ids,
            )
        });
    }

    let mut remaining_bytes = max_bytes;
    let mut parent_discovery_complete = probe_candidates.is_empty();
    if !probe_candidates.is_empty() {
        let stored_cursor = load_required_parent_probe_cursor(&connection)
            .ok()
            .flatten();
        let stored_cursor_position = stored_cursor.as_ref().and_then(|(cursor_path, _)| {
            probe_candidates.iter().position(|candidate| {
                discovered_files[*candidate].3.to_string_lossy() == cursor_path.as_str()
            })
        });
        let cursor_position = stored_cursor_position.unwrap_or(0);
        let mut position = cursor_position;
        let mut completed_candidates = 0_usize;
        let mut probe_bytes = remaining_bytes / 2;
        let mut next_cursor = Some((
            discovered_files[probe_candidates[position]]
                .3
                .to_string_lossy()
                .into_owned(),
            stored_cursor_position
                .and_then(|_| stored_cursor.as_ref().map(|(_, offset)| *offset))
                .unwrap_or(0),
        ));
        while probe_bytes > 0
            && completed_candidates < probe_candidates.len()
            && discovery_started.elapsed().as_millis() < max_discovery_millis
        {
            let candidate = probe_candidates[position];
            let path = &discovered_files[candidate].3;
            let path_value = path.to_string_lossy().into_owned();
            let start_offset = next_cursor
                .as_ref()
                .filter(|(cursor_path, _)| cursor_path == &path_value)
                .map_or(0, |(_, offset)| *offset);
            let probe = match rollout_leaf_session_id(path, start_offset, probe_bytes) {
                Ok(probe) => probe,
                Err(()) => {
                    traversal_complete = false;
                    completed_candidates = completed_candidates.saturating_add(1);
                    position = (position + 1) % probe_candidates.len();
                    next_cursor = Some((
                        discovered_files[probe_candidates[position]]
                            .3
                            .to_string_lossy()
                            .into_owned(),
                        0,
                    ));
                    continue;
                }
            };
            probe_bytes = probe_bytes.saturating_sub(probe.bytes_read);
            remaining_bytes = remaining_bytes.saturating_sub(probe.bytes_read);
            if probe
                .session_id
                .as_ref()
                .is_some_and(|session_id| unresolved_parent_ids.remove(session_id))
            {
                dependency_parent_paths.insert(path_value.clone());
                next_cursor = Some((path_value, 0));
                break;
            }
            if probe.complete {
                completed_candidates = completed_candidates.saturating_add(1);
                position = (position + 1) % probe_candidates.len();
                next_cursor = Some((
                    discovered_files[probe_candidates[position]]
                        .3
                        .to_string_lossy()
                        .into_owned(),
                    0,
                ));
            } else {
                next_cursor = Some((path_value, probe.next_offset));
                break;
            }
        }
        parent_discovery_complete =
            unresolved_parent_ids.is_empty() || completed_candidates == probe_candidates.len();
        store_required_parent_probe_cursor(
            &connection,
            next_cursor
                .as_ref()
                .map(|(path, offset)| (path.as_str(), *offset)),
        )
        .ok()?;
    } else {
        store_required_parent_probe_cursor(&connection, None).ok()?;
    }

    let mut ordered_files = Vec::with_capacity(discovered_files.len());
    for (identity, modified_ns, size, path) in discovered_files {
        let path_value = path.to_string_lossy().into_owned();
        let is_required_parent = dependency_parent_paths.contains(&path_value);
        let stored = stored_files.get(&path_value);
        let needs_work = (modified_ns >= cutoff_modified_ns || is_required_parent)
            && stored.is_none_or(|stored| stored.needs_work(&identity, size, modified_ns, today));
        let is_pending = needs_work
            && stored.is_none_or(|stored| stored.is_pending(&identity, size, modified_ns, today));
        ordered_files.push((
            needs_work,
            is_required_parent,
            is_pending,
            identity,
            modified_ns,
            size,
            path,
        ));
    }
    let present = ordered_files
        .iter()
        .map(|(_, _, _, _, _, _, path)| path.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    for duplicate in &duplicate_rollout_paths {
        reset_file(&connection, duplicate).ok()?;
    }
    if traversal_complete {
        for missing in stored_files.keys().filter(|path| !present.contains(*path)) {
            reset_file(&connection, missing).ok()?;
        }
    }
    for (path_value, stored) in &stored_files {
        if present.contains(path_value) {
            continue;
        }
        let path = PathBuf::from(path_value);
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl")
            || !rollout_roots.iter().any(|root| path.starts_with(root))
            || !rollout_roots
                .iter()
                .any(|root| is_regular_file_without_intermediate_symlinks(root, &path))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let Ok(modified_ns) = file_modified_ns(&metadata) else {
            continue;
        };
        let identity = file_identity(&metadata);
        let size = metadata.len();
        let Ok(canonical_path) = fs::canonicalize(&path) else {
            continue;
        };
        if !trusted_rollout_roots
            .iter()
            .any(|root| canonical_path.starts_with(root))
        {
            continue;
        }
        if !accepted_canonical_rollout_paths.insert(canonical_path) {
            reset_file(&connection, path_value).ok()?;
            continue;
        }
        let is_required_parent = dependency_parent_paths.contains(path_value)
            || stored
                .leaf_session_id
                .as_ref()
                .is_some_and(|session_id| required_parent_ids.contains(session_id));
        let needs_work = (modified_ns >= cutoff_modified_ns || is_required_parent)
            && stored.needs_work(&identity, size, modified_ns, today);
        let is_pending = needs_work && stored.is_pending(&identity, size, modified_ns, today);
        if !is_pending {
            continue;
        }
        if is_required_parent {
            dependency_parent_paths.insert(path_value.clone());
        }
        ordered_files.push((
            needs_work,
            is_required_parent,
            is_pending,
            identity,
            modified_ns,
            size,
            path,
        ));
    }
    ordered_files.sort_by_key(|entry| rollout_work_priority(entry.0, entry.2, entry.1, entry.4));
    let files = ordered_files
        .into_iter()
        .filter(|(needs_work, _, _, _, _, _, _)| *needs_work)
        .collect::<Vec<_>>();
    let index_context = FileIndexContext {
        cutoff,
        detail_cutoff,
        today,
        fast_turns: &fast_turns,
        dependency_parent_paths: &dependency_parent_paths,
    };
    let discovery_elapsed_millis = discovery_started.elapsed().as_millis();
    let parse_started = Instant::now();
    let mut all_complete = traversal_complete && parent_discovery_complete;
    let mut failed = false;
    let mut visited_files = 0_u64;
    let mut completed_files = 0_u64;
    for (_, _, _, _, _, _, path) in &files {
        if parse_started.elapsed().as_millis() >= max_parse_millis {
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
            &index_context,
            parse_started,
            max_parse_millis,
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
    } else if discovery_elapsed_millis >= max_discovery_millis
        || parse_started.elapsed().as_millis() >= max_parse_millis
    {
        "time"
    } else if remaining_bytes == 0 {
        "bytes"
    } else {
        "pending"
    };
    debug_usage_event(&format!(
        "scan_pass_completed stop={stop} bytes_read={} elapsed_ms={} discovery_elapsed_ms={} parse_elapsed_ms={} visited_files={visited_files} completed_files={completed_files} pending_files={pending_files} error_files={error_files} excluded_inherited_files={excluded_files} traversal_complete={traversal_complete}",
        max_bytes.saturating_sub(remaining_bytes),
        pass_started.elapsed().as_millis(),
        discovery_elapsed_millis,
        parse_started.elapsed().as_millis()
    ));
    debug_unpriced_model_days(&connection, detail_cutoff, today);
    let indexed_files = load_file_summaries(&connection).ok()?;
    let latest_pending_modified_at = files
        .iter()
        .filter(|(_, _, _, identity, modified_ns, size, path)| {
            let path_value = path.to_string_lossy();
            indexed_files
                .get(path_value.as_ref())
                .is_none_or(|stored| stored.is_pending(identity, *size, *modified_ns, today))
        })
        .filter_map(|(_, _, _, _, modified_ns, _, _)| {
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
        Err(PricingLookupFailure::MissingFastLongContextPrice) => {
            "status=missing-fast-long-context-price".to_owned()
        }
        Err(PricingLookupFailure::MissingFastPrice) => "status=missing-fast-price".to_owned(),
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
    let cutoff = today - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
    let detail_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
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
        TOKEN_HISTORY_RETENTION_DAYS,
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
            "SELECT d.day, d.model, d.pricing_mode,
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
             WHERE f.parser_version = ?1 AND f.accounting_ready = 1
               AND f.usage_excluded = 0 AND d.day >= ?2 AND d.day <= ?3
             GROUP BY d.day, d.model, d.pricing_mode
             ORDER BY d.day DESC, SUM(d.observed_tokens) DESC, d.pricing_mode",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                ROLLOUT_PARSER_VERSION,
                detail_cutoff.to_string(),
                today.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, u64>(7)?,
                    row.get::<_, u64>(8)?,
                    row.get::<_, u64>(9)?,
                    row.get::<_, f64>(10)?,
                    row.get::<_, bool>(11)?,
                ))
            },
        )
        .map_err(|_| ())?;
    for row in rows {
        let (
            day,
            model,
            pricing_mode,
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
            "[TouchGrassBar][codex-usage-report] day={day} model={model} pricing_mode={pricing_mode} observed_tokens={observed} input_tokens={input} cached_input_subset={cached_input} cache_write_input_subset={cache_write_input} output_tokens={output} reasoning_output_subset={reasoning_output} priced_tokens={priced} local_cost_usd={:.6} detail_complete={complete} catalog_{catalog}",
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
        account.as_ref().map(|cached| &cached.observed_at_by_day),
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
    project_usage_periods_with_account_time(account, local, now, now, None)
}

pub(crate) fn project_usage_periods_with_account_time(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
    account_observed_at: OffsetDateTime,
    account_observed_at_by_day: Option<&BTreeMap<Date, OffsetDateTime>>,
) -> UsagePeriods {
    let evidence = provider_usage_evidence(
        account,
        local,
        now,
        account_observed_at,
        account_observed_at_by_day,
    );
    calculate_usage_periods(&evidence, now)
}

fn provider_usage_evidence(
    account: Option<&AccountUsageObservation>,
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
    account_observed_at: OffsetDateTime,
    account_observed_at_by_day: Option<&BTreeMap<Date, OffsetDateTime>>,
) -> ProviderUsageEvidence {
    let today = utc_ranking_day(now);
    ProviderUsageEvidence {
        provider_reported_tokens: account.map(|account| account.daily_tokens.clone()),
        provider_observed_at: account.map(|_| account_observed_at),
        provider_observed_at_by_day: account_observed_at_by_day.cloned().unwrap_or_default(),
        local_usage_evidence: local.map_or_else(BTreeMap::new, |local| {
            local
                .daily
                .iter()
                .map(|(day, detail)| {
                    (
                        *day,
                        DailyUsageEvidence {
                            observed_tokens: detail.observed_tokens,
                            coverage: if local.period_scan_status(*day, 1)
                                == UsageScanStatus::Complete
                            {
                                UsageCoverage::Complete
                            } else {
                                UsageCoverage::Partial
                            },
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
    }
}

pub(crate) fn load_daily_usage_history(
    connection: &Connection,
    now: OffsetDateTime,
    anchor_day: Date,
    length: i64,
) -> Result<BTreeMap<Date, UsageTotal>, ()> {
    if !(1..=60).contains(&length) {
        return Err(());
    }
    let account = load_cached_account_usage_from_connection(connection);
    let cutoff = anchor_day
        .checked_sub(Duration::days(length - 1))
        .ok_or(())?;
    let local = read_indexed_usage(
        connection,
        cutoff,
        anchor_day,
        UsageScanStatus::Complete,
        true,
        None,
    )?;
    let evidence = provider_usage_evidence(
        account.as_ref().map(|cached| &cached.observation),
        Some(&local),
        now,
        account.as_ref().map_or(now, |cached| cached.observed_at),
        account.as_ref().map(|cached| &cached.observed_at_by_day),
    );
    Ok(calculate_daily_usage_aggregates(
        &evidence,
        anchor_day.midnight().assume_utc(),
        anchor_day,
        length,
    ))
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

    fn canonical_last(self) -> Result<Self, ()> {
        let normalized = Self {
            total: self.input.checked_add(self.output).ok_or(())?,
            ..self
        };
        normalized.validate()?;
        Ok(normalized)
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

    fn turn_rollout(turn_id: &str, model: &str, total: u64) -> String {
        let input = total * 10 / 11;
        let cached = input * 9 / 10;
        let output = total - input;
        [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":model}}),
            json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":turn_id,"model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total},"model_context_window":1050000,"total_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":total}},"rate_limits":null}}),
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

    fn token_usage(
        input: u64,
        cached_input: u64,
        cache_write_input: u64,
        output: u64,
    ) -> TokenUsage {
        TokenUsage {
            input,
            cached_input,
            cache_write_input,
            output,
            reasoning_output: 0,
            total: input + output,
        }
    }

    fn session_meta_line(
        timestamp: &str,
        session_id: &str,
        parent_id: Option<&str>,
        is_subagent: bool,
    ) -> serde_json::Value {
        let mut payload = json!({
            "cli_version": "0.146.0-alpha.3.1",
            "id": session_id,
        });
        if let Some(parent_id) = parent_id {
            payload["forked_from_id"] = json!(parent_id);
        }
        if is_subagent {
            payload["source"] = json!({"subagent":{"thread_spawn":{}}});
        }
        json!({"timestamp":timestamp,"type":"session_meta","payload":payload})
    }

    fn token_count_line(timestamp: &str, total: u64, last: u64) -> serde_json::Value {
        let total_input = total * 7 / 10;
        let last_input = last * 7 / 10;
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": last_input,
                        "cached_input_tokens": 0,
                        "output_tokens": last - last_input,
                        "reasoning_output_tokens": 0,
                        "total_tokens": last
                    },
                    "model_context_window": 1_050_000,
                    "total_token_usage": {
                        "input_tokens": total_input,
                        "cached_input_tokens": 0,
                        "output_tokens": total - total_input,
                        "reasoning_output_tokens": 0,
                        "total_tokens": total
                    }
                },
                "rate_limits": null
            }
        })
    }

    fn token_count_usage_line(
        timestamp: &str,
        total: TokenUsage,
        last: TokenUsage,
    ) -> serde_json::Value {
        let usage = |usage: TokenUsage| {
            json!({
                "input_tokens": usage.input,
                "cached_input_tokens": usage.cached_input,
                "cache_write_input_tokens": usage.cache_write_input,
                "output_tokens": usage.output,
                "reasoning_output_tokens": usage.reasoning_output,
                "total_tokens": usage.total,
            })
        };
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": usage(last),
                    "model_context_window": 1_050_000,
                    "total_token_usage": usage(total),
                },
                "rate_limits": null,
            }
        })
    }

    fn with_ordinal(mut line: serde_json::Value, ordinal: u64) -> serde_json::Value {
        line["ordinal"] = json!(ordinal);
        line
    }

    fn reviewed_codex_0_148_root_rollout(total: u64) -> String {
        jsonl([
            json!({
                "ordinal": 0,
                "timestamp": "2026-08-24T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "context_window": { "window_id": "fixture-window" },
                    "history_mode": "paginated"
                }
            }),
            json!({
                "ordinal": 1,
                "timestamp": "2026-08-24T10:00:00Z",
                "type": "inter_agent_communication_metadata",
                "payload": { "trigger_turn": false }
            }),
            json!({
                "ordinal": 2,
                "timestamp": "2026-08-24T10:00:01Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
            with_ordinal(token_count_line("2026-08-24T10:01:00Z", total, total), 3),
        ])
    }

    fn codex_0_148_task_reset_prefix() -> Vec<serde_json::Value> {
        let previous = token_usage(1_000, 200, 0, 100);
        vec![
            json!({
                "ordinal": 0,
                "timestamp": "2026-08-24T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "context_window": { "window_id": "fixture-window" },
                    "history_mode": "paginated"
                }
            }),
            json!({
                "ordinal": 1,
                "timestamp": "2026-08-24T10:00:01Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
            with_ordinal(
                token_count_usage_line("2026-08-24T10:01:00Z", previous, previous),
                2,
            ),
            json!({
                "ordinal": 3,
                "timestamp": "2026-08-24T10:01:01Z",
                "type": "event_msg",
                "payload": { "type": "task_complete" }
            }),
            json!({
                "ordinal": 4,
                "timestamp": "2026-08-24T10:01:02Z",
                "type": "event_msg",
                "payload": {
                    "type": "task_started",
                    "turn_id": "fixture-new-turn",
                    "model_context_window": 1_050_000,
                    "collaboration_mode_kind": "default"
                }
            }),
            json!({
                "ordinal": 5,
                "timestamp": "2026-08-24T10:01:03Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
        ]
    }

    fn codex_0_148_task_reset_counters() -> [serde_json::Value; 2] {
        let first = token_usage(100, 20, 0, 10);
        let current = token_usage(150, 30, 0, 15);
        let last = token_usage(50, 10, 0, 5);
        [
            with_ordinal(
                token_count_usage_line("2026-08-24T10:02:00Z", first, first),
                6,
            ),
            with_ordinal(
                token_count_usage_line("2026-08-24T10:03:00Z", current, last),
                7,
            ),
        ]
    }

    fn set_modified_at(path: &Path, timestamp: OffsetDateTime) {
        let modified = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(u64::try_from(timestamp.unix_timestamp()).unwrap());
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    fn jsonl(lines: impl IntoIterator<Item = serde_json::Value>) -> String {
        lines
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn parent_snapshot_rollout(parent_id: &str, baseline: u64, final_total: u64) -> String {
        jsonl([
            session_meta_line("2026-08-06T10:00:00Z", parent_id, None, false),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            token_count_line("2026-08-06T10:04:00Z", baseline, baseline),
            token_count_line("2026-08-06T10:06:00Z", final_total, final_total - baseline),
        ])
    }

    fn copied_child_rollout(child_id: &str, parent_id: &str, final_total: u64) -> String {
        jsonl([
            session_meta_line("2026-08-06T10:05:00Z", child_id, Some(parent_id), true),
            session_meta_line("2026-08-06T10:00:00Z", parent_id, None, false),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
            json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            token_count_line("2026-08-06T10:07:00Z", final_total, final_total - 1_000),
        ])
    }

    fn copied_child_strong_reset_rollout(child_id: &str, parent_id: &str) -> String {
        jsonl([
            session_meta_line("2026-08-06T10:05:00Z", child_id, Some(parent_id), true),
            session_meta_line("2026-08-06T10:00:00Z", parent_id, None, false),
            token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
            json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:05:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            token_count_line("2026-08-06T10:07:00Z", 100, 100),
        ])
    }

    fn indexed_tokens_for_path(database: &Path, path: &Path) -> u64 {
        Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT COALESCE(SUM(observed_tokens), 0)
                 FROM codex_usage_file_model_days WHERE path = ?1",
                [path.to_string_lossy().as_ref()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
    }

    fn run_usage_passes(
        fixture: &TempUsage,
        now: OffsetDateTime,
        passes: usize,
    ) -> LocalUsageObservation {
        let mut observation = None;
        for _ in 0..passes {
            observation = index_local_usage_at(&fixture.database, &fixture.root, now);
        }
        observation.unwrap()
    }

    fn changed_pricing_manifest(basis: &str, output_rate: f64) -> PricingManifest {
        let mut value: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        value["basis"] = json!(basis);
        pricing_model_mut(&mut value, "gpt-5.6-sol")["periods"][0]["outputUsdPerMillion"] =
            json!(output_rate);
        parse_pricing_manifest(&value.to_string()).unwrap()
    }

    fn pricing_model_mut<'a>(
        manifest: &'a mut serde_json::Value,
        name: &str,
    ) -> &'a mut serde_json::Value {
        manifest["models"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|model| model["name"].as_str() == Some(name))
            .unwrap()
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
    fn account_usage_accepts_reviewed_optional_thread_usage_metadata() {
        let observation = parse_account_usage(
            r#"{
              "dailyUsageBuckets": [
                { "startDate": "2026-08-06", "tokens": 340 }
              ],
              "summary": { "lifetimeTokens": 340 },
              "threadUsage": {
                "threadId": "reviewed-but-not-retained",
                "totalTokens": 340
              }
            }"#,
        )
        .expect("reviewed thread usage metadata must not hide daily usage");

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
    fn sqlite_account_usage_cache_merges_sparse_and_empty_refreshes_by_day() {
        let fixture = TempUsage::new();
        let observed_at = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let previous_day = observed_at.date() - Duration::days(1);
        let observation = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(previous_day, 120), (observed_at.date(), 340)]),
        };

        store_cached_account_usage(Some(&fixture.database), &observation, observed_at).unwrap();

        assert_eq!(
            load_cached_account_usage(Some(&fixture.database)),
            Some(CachedAccountUsageObservation {
                observed_at_by_day: observation.observed_at_by_day(observed_at),
                observation: observation.clone(),
                observed_at,
            })
        );

        let sparse_observed_at = observed_at + Duration::minutes(30);
        let sparse = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(observed_at.date(), 0)]),
        };
        store_cached_account_usage(Some(&fixture.database), &sparse, sparse_observed_at).unwrap();

        let cached = load_cached_account_usage(Some(&fixture.database)).unwrap();
        assert_eq!(
            cached.observation.daily_tokens,
            BTreeMap::from([(previous_day, 120), (observed_at.date(), 0)])
        );
        assert_eq!(
            cached.observed_at_by_day,
            BTreeMap::from([
                (previous_day, observed_at),
                (observed_at.date(), sparse_observed_at),
            ])
        );
        assert_eq!(cached.observed_at, sparse_observed_at);

        let empty_observed_at = sparse_observed_at + Duration::minutes(30);
        store_cached_account_usage(
            Some(&fixture.database),
            &AccountUsageObservation {
                daily_tokens: BTreeMap::new(),
            },
            empty_observed_at,
        )
        .unwrap();
        let after_empty = load_cached_account_usage(Some(&fixture.database)).unwrap();
        assert_eq!(after_empty.observation, cached.observation);
        assert_eq!(after_empty.observed_at_by_day, cached.observed_at_by_day);
        assert_eq!(after_empty.observed_at, empty_observed_at);

        let fallback_observed_at = empty_observed_at + Duration::minutes(30);
        let write_failure_fallback = merge_cached_account_usage(
            Some(after_empty),
            AccountUsageObservation {
                daily_tokens: BTreeMap::from([(observed_at.date(), 5)]),
            },
            fallback_observed_at,
        );
        assert_eq!(
            write_failure_fallback.observation.daily_tokens,
            BTreeMap::from([(previous_day, 120), (observed_at.date(), 5)])
        );
        assert_eq!(
            write_failure_fallback.observed_at_by_day,
            BTreeMap::from([
                (previous_day, observed_at),
                (observed_at.date(), fallback_observed_at),
            ])
        );

        store_cached_account_usage(
            Some(&fixture.database),
            &AccountUsageObservation {
                daily_tokens: BTreeMap::new(),
            },
            observed_at - Duration::minutes(1),
        )
        .unwrap();
        crate::database::prepare(&fixture.database).expect("clock rollback keeps the cache valid");
    }

    #[test]
    fn sqlite_account_usage_cache_keeps_the_exact_sixty_day_utc_window() {
        let fixture = TempUsage::new();
        let observed_at = OffsetDateTime::parse("2026-08-06T23:59:00Z", &Rfc3339).unwrap();
        let today = utc_ranking_day(observed_at);
        let oldest_retained = today - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
        let expired = oldest_retained - Duration::days(1);
        let future = today + Duration::days(1);
        let observation = AccountUsageObservation {
            daily_tokens: BTreeMap::from([
                (expired, 61),
                (oldest_retained, 600),
                (today, 900),
                (future, 1_000),
            ]),
        };

        store_cached_account_usage(Some(&fixture.database), &observation, observed_at).unwrap();

        let cached = load_cached_account_usage(Some(&fixture.database)).unwrap();
        assert_eq!(cached.observed_at, observed_at);
        assert_eq!(
            cached.observation.daily_tokens,
            BTreeMap::from([(oldest_retained, 600), (today, 900)])
        );

        let local_day = |observed_tokens| LocalUsageDay {
            observed_tokens,
            priced_tokens: observed_tokens,
            api_equivalent_cost_usd: Some(1.0),
            modeled: false,
            complete: true,
            observed_through: Some(observed_at),
            priced_observed_through: Some(observed_at),
            pricing_basis: Some(pricing_manifest().unwrap().basis.clone()),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(oldest_retained, local_day(6)), (today, local_day(9))]),
            scan_status: UsageScanStatus::Complete,
            scan_scope_known: true,
            ..LocalUsageObservation::default()
        };
        let evidence = provider_usage_evidence(
            Some(&cached.observation),
            Some(&local),
            observed_at,
            cached.observed_at,
            Some(&cached.observed_at_by_day),
        );
        let daily = calculate_daily_usage_aggregates(&evidence, observed_at, today, 60);

        for (day, expected_tokens) in [(oldest_retained, 600), (today, 900)] {
            let UsageTotal::Current {
                observed_tokens,
                evidence_basis,
                ..
            } = &daily[&day]
            else {
                panic!("the retained account day must be available");
            };
            assert_eq!(*observed_tokens, expected_tokens);
            assert_eq!(*evidence_basis, UsageEvidenceBasis::ProviderReported);
        }
    }

    #[test]
    fn v6_migration_preserves_daily_aggregates_and_removes_legacy_fast_details() {
        let fixture = TempUsage::new();
        prepare_database(&fixture.database).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection
            .execute_batch(
                "DROP INDEX codex_usage_file_turns_by_turn_id;
                 ALTER TABLE codex_usage_file_turns RENAME TO codex_usage_file_turns_v7;
                 CREATE TABLE codex_usage_file_turns (
                   path TEXT NOT NULL,
                   turn_id TEXT NOT NULL,
                   PRIMARY KEY (path, turn_id),
                   FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
                 );
                 DROP TABLE codex_usage_file_turns_v7;
                 CREATE INDEX codex_usage_file_turns_by_turn_id
                 ON codex_usage_file_turns(turn_id);
                 INSERT INTO codex_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   parser_version, completion_state, active_turn_id, schema_supported
                 ) VALUES(
                   'private-rollout', '1:2', 10, 20, 10, 15, 'complete',
                   'private-turn', 1
                 );
                 INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens, cache_write_input_tokens,
                   output_tokens, reasoning_output_tokens, observed_tokens,
                   cost_usd, pricing_basis, pricing_fingerprint, complete,
                   observed_through
                 ) VALUES(
                   'private-rollout', '2026-08-01', 'gpt-5.6-sol', 100, 'standard',
                   100, 20, 0, 30, 5, 135, 1.25, 'stored-pricing-v1',
                   'stored-fingerprint-v1', 1, '2026-08-01T12:00:00Z'
                 );
                 INSERT INTO codex_usage_file_days(
                   path, day, observed_tokens, priced_tokens, cost_usd, complete,
                   observed_through, priced_observed_through, pricing_fingerprint
                 ) VALUES(
                   'private-rollout', '2026-08-01', 135, 135, 1.25, 1,
                   '2026-08-01T12:00:00Z', '2026-08-01T12:00:00Z',
                   'stored-fingerprint-v1'
                 );
                 INSERT INTO codex_usage_token_snapshots(
                   path, record_ordinal, timestamp_ns, input_tokens,
                   cached_input_tokens, cache_write_input_tokens, output_tokens,
                   reasoning_output_tokens, total_tokens
                 ) VALUES('private-rollout', 1, 10, 100, 20, 0, 30, 5, 135);
                 INSERT INTO codex_usage_fast_turns(turn_id, model)
                 VALUES('private-turn', 'gpt-5.6-sol');
                 INSERT INTO codex_usage_file_turns(path, turn_id)
                 VALUES('private-rollout', 'private-turn');
                 INSERT INTO codex_usage_index_meta(key, value)
                 VALUES('fast_turn_fingerprint', 'private-fingerprint');
                 UPDATE touchgrassbar_schema_versions SET version = 6
                 WHERE module = 'codex-usage-index';
                 PRAGMA journal_mode = DELETE;",
            )
            .unwrap();
        drop(connection);

        let mut connection = Connection::open(&fixture.database).unwrap();
        ensure_index_schema(&mut connection, Some(&fixture.database)).unwrap();

        assert_eq!(
            usage_index_schema_version(&connection).unwrap(),
            USAGE_INDEX_SCHEMA_VERSION
        );
        assert_eq!(
            table_columns(&connection, "codex_usage_file_turns").unwrap(),
            ["path", "turn_id", "day"]
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM codex_usage_file_turns", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM codex_usage_fast_turns", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT active_turn_id FROM codex_usage_files WHERE path = 'private-rollout'",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )
                .unwrap(),
            None
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT observed_tokens, priced_tokens, cost_usd
                     FROM codex_usage_file_days
                     WHERE path = 'private-rollout' AND day = '2026-08-01'",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?
                    ))
                )
                .unwrap(),
            (135, 135, 1.25)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_basis, pricing_fingerprint
                     FROM codex_usage_file_model_days
                     WHERE path = 'private-rollout' AND day = '2026-08-01'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            (
                "stored-pricing-v1".to_owned(),
                "stored-fingerprint-v1".to_owned()
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT total_tokens FROM codex_usage_token_snapshots
                     WHERE path = 'private-rollout' AND record_ordinal = 1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            135
        );
        assert!(usage_index_backup_path(&fixture.database, 6).is_file());
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
            "active_turn_id",
        ] {
            assert!(file_columns.iter().any(|column| column == required));
        }
        assert!(
            table_columns(&connection, "codex_usage_file_model_days")
                .unwrap()
                .iter()
                .any(|column| column == "pricing_fingerprint")
        );
        assert!(
            table_columns(&connection, "codex_usage_file_model_days")
                .unwrap()
                .iter()
                .any(|column| column == "pricing_mode")
        );
        assert_eq!(
            table_columns(&connection, "codex_usage_file_turns").unwrap(),
            ["path", "turn_id", "day"]
        );
        assert_eq!(
            table_columns(&connection, "codex_usage_fast_turns").unwrap(),
            ["turn_id", "model"]
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM codex_usage_files WHERE path = 'private-rollout'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
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
    fn sqlite_index_counts_reviewed_codex_cli_0_148_usage() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let observed = token_usage(80, 10, 20, 20);
        let mut token_count = token_count_usage_line("2026-08-24T10:01:00Z", observed, observed);
        token_count["ordinal"] = json!(3);
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({
                    "ordinal": 0,
                    "timestamp": "2026-08-24T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.148.0-alpha.21",
                        "context_window": { "window_id": "fixture-window" },
                        "history_mode": "paginated",
                        "id": "reviewed-0-148-root",
                        "source": "cli",
                        "thread_source": "cli",
                        "timestamp": "2026-08-24T10:00:00Z"
                    }
                }),
                json!({
                    "ordinal": 1,
                    "timestamp": "2026-08-24T10:00:00Z",
                    "type": "inter_agent_communication_metadata",
                    "payload": { "trigger_turn": false }
                }),
                json!({
                    "ordinal": 2,
                    "timestamp": "2026-08-24T10:00:01Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                }),
                token_count,
            ]),
        )
        .unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the reviewed Codex 0.148 usage must index");

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
    }

    #[test]
    fn sqlite_index_counts_each_reviewed_provider_ordinal_version() {
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        for cli_version in ["0.148.0-alpha.21", "0.149.1", "0.150.0-alpha.8"] {
            let fixture = TempUsage::new();
            let observed = token_usage(80, 10, 20, 20);
            let mut token_count =
                token_count_usage_line("2026-08-26T10:01:00Z", observed, observed);
            token_count["ordinal"] = json!(3);
            fs::write(
                &fixture.rollout,
                jsonl([
                    json!({
                        "ordinal": 0,
                        "timestamp": "2026-08-26T10:00:00Z",
                        "type": "session_meta",
                        "payload": {
                            "cli_version": cli_version,
                            "history_mode": "paginated",
                            "id": format!("reviewed-{cli_version}-root"),
                            "source": "cli",
                            "thread_source": "cli",
                            "timestamp": "2026-08-26T10:00:00Z"
                        }
                    }),
                    json!({
                        "ordinal": 1,
                        "timestamp": "2026-08-26T10:00:00Z",
                        "type": "inter_agent_communication_metadata",
                        "payload": { "trigger_turn": false }
                    }),
                    json!({
                        "ordinal": 2,
                        "timestamp": "2026-08-26T10:00:01Z",
                        "type": "turn_context",
                        "payload": { "model": "gpt-5.6-sol" }
                    }),
                    token_count,
                ]),
            )
            .unwrap();

            let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
                .expect("each reviewed provider-ordinal version must index");

            assert_eq!(
                indexed.scan_status,
                UsageScanStatus::Complete,
                "{cli_version}"
            );
            assert_eq!(
                indexed.daily[&now.date()].observed_tokens,
                100,
                "{cli_version}"
            );
        }
    }

    #[test]
    fn sqlite_index_counts_owned_usage_from_a_reviewed_codex_0_150_child() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        let observed = token_usage(70, 20, 0, 30);
        let mut token_count = token_count_usage_line("2026-08-26T10:00:02Z", observed, observed);
        token_count["ordinal"] = json!(6);
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({
                    "ordinal": 0,
                    "timestamp": "2026-08-26T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.150.0-alpha.8",
                        "history_mode": "paginated",
                        "id": "child-fixture",
                        "forked_from_id": "parent-fixture",
                        "thread_source": "subagent",
                        "subagent_history_start_ordinal": 3,
                        "timestamp": "2026-08-26T10:00:00Z"
                    }
                }),
                json!({
                    "ordinal": 1,
                    "timestamp": "2026-08-26T09:59:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.150.0-alpha.8",
                        "history_mode": "paginated",
                        "id": "parent-fixture",
                        "thread_source": "cli",
                        "timestamp": "2026-08-26T09:59:00Z"
                    }
                }),
                json!({
                    "ordinal": 2,
                    "timestamp": "2026-08-26T09:59:01Z",
                    "type": "response_item",
                    "payload": { "type": "message", "role": "assistant" }
                }),
                json!({
                    "ordinal": 3,
                    "timestamp": "2026-08-26T10:00:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "thread_settings_applied",
                        "thread_settings": {}
                    }
                }),
                json!({
                    "ordinal": 4,
                    "timestamp": "2026-08-26T10:00:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "task_started",
                        "turn_id": "turn-fixture",
                        "model_context_window": 1_050_000,
                        "collaboration_mode_kind": "default",
                        "started_at": 0
                    }
                }),
                json!({
                    "ordinal": 5,
                    "timestamp": "2026-08-26T10:00:01Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                }),
                token_count,
            ]),
        )
        .unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the reviewed Codex 0.150 child usage must index");

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
        assert!(!indexed.has_excluded_usage);
    }

    #[test]
    fn sqlite_index_counts_a_reviewed_codex_0_151_root() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-09-02T12:00:00Z", &Rfc3339).unwrap();
        let observed = token_usage(70, 20, 0, 30);
        let mut token_count = token_count_usage_line("2026-09-02T10:00:02Z", observed, observed);
        token_count["ordinal"] = json!(3);
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({
                    "ordinal": 0,
                    "timestamp": "2026-09-02T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.151.0-alpha.7.2",
                        "history_mode": "paginated",
                        "id": "reviewed-0-151-root",
                        "originator": "codex_app",
                        "session_id": "reviewed-0-151-root",
                        "source": "cli",
                        "thread_source": "cli",
                        "timestamp": "2026-09-02T10:00:00Z"
                    }
                }),
                json!({
                    "ordinal": 1,
                    "timestamp": "2026-09-02T10:00:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "task_started",
                        "turn_id": "turn-fixture",
                        "model_context_window": 1_050_000,
                        "collaboration_mode_kind": "default",
                        "started_at": 0
                    }
                }),
                json!({
                    "ordinal": 2,
                    "timestamp": "2026-09-02T10:00:01Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                }),
                token_count,
            ]),
        )
        .unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the reviewed Codex 0.151 usage must index");

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
        assert!(!indexed.has_excluded_usage);
    }

    #[test]
    fn sqlite_index_rejects_noncontiguous_reviewed_codex_0_150_ordinals() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        let observed = token_usage(70, 20, 0, 30);
        let mut token_count = token_count_usage_line("2026-08-26T10:00:02Z", observed, observed);
        token_count["ordinal"] = json!(3);
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({
                    "ordinal": 0,
                    "timestamp": "2026-08-26T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.150.0-alpha.8",
                        "history_mode": "paginated"
                    }
                }),
                json!({
                    "ordinal": 2,
                    "timestamp": "2026-08-26T10:00:01Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                }),
                token_count,
            ]),
        )
        .unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the failed scan must still return bounded status");

        assert_eq!(indexed.scan_status, UsageScanStatus::Unavailable);
        assert!(indexed.daily.is_empty());
        let completion: String = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT completion_state FROM codex_usage_files",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(completion, "error");
    }

    #[test]
    fn sqlite_index_retries_files_rejected_by_the_previous_codex_parser() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        let rollout = reviewed_codex_0_148_root_rollout(100)
            .replace("0.148.0-alpha.21", "0.150.0-alpha.8")
            .replace("2026-08-24", "2026-08-26");
        fs::write(&fixture.rollout, rollout).unwrap();
        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(first.daily[&now.date()].observed_tokens, 100);

        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .execute_batch(
                "DELETE FROM codex_usage_file_days;
                 DELETE FROM codex_usage_file_model_days;
                 DELETE FROM codex_usage_token_snapshots;",
            )
            .unwrap();
        connection
            .execute(
                "UPDATE codex_usage_files
                 SET parser_version = ?1,
                     completion_state = 'error',
                     parsed_offset = size_bytes,
                     schema_supported = 0,
                     accounting_ready = 0,
                     parser_error_seen = 1",
                [COMPATIBLE_ROLLOUT_PARSER_VERSION],
            )
            .unwrap();
        drop(connection);

        let retried = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(retried.scan_status, UsageScanStatus::Complete);
        assert_eq!(retried.daily[&now.date()].observed_tokens, 100);
        let state: (i64, String, bool, bool, bool) = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT parser_version, completion_state, schema_supported,
                        accounting_ready, parser_error_seen
                 FROM codex_usage_files",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            state,
            (
                ROLLOUT_PARSER_VERSION,
                "complete".to_owned(),
                true,
                true,
                false
            )
        );
    }

    #[test]
    fn sqlite_index_reuses_complete_rows_from_the_previous_compatible_parser() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            reviewed_codex_0_148_root_rollout(100).replace("2026-08-24", "2026-08-26"),
        )
        .unwrap();
        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(first.daily[&now.date()].observed_tokens, 100);

        Connection::open(&fixture.database)
            .unwrap()
            .execute(
                "UPDATE codex_usage_files SET parser_version = ?1",
                [COMPATIBLE_ROLLOUT_PARSER_VERSION],
            )
            .unwrap();

        let reused = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: 0,
                max_file_bytes: 0,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();

        assert_eq!(reused.scan_status, UsageScanStatus::Complete);
        assert_eq!(reused.daily[&now.date()].observed_tokens, 100);
        assert_eq!(
            Connection::open(&fixture.database)
                .unwrap()
                .query_row("SELECT parser_version FROM codex_usage_files", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            ROLLOUT_PARSER_VERSION
        );
    }

    #[test]
    fn sqlite_index_does_not_promote_unsafe_previous_parser_rows() {
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        for unsafe_change in [
            "completion_state = 'indexing'",
            "parsed_offset = 0",
            "accounting_ready = 0",
            "usage_excluded = 1",
            "schema_supported = 0",
            "parser_error_seen = 1",
            "lineage_invalid = 1",
            "snapshot_timestamp_regressed = 1",
            "deferred_until_day = '2026-08-27'",
            "lineage_mode = 'unknown'",
            "provider_ordinal_mode = 'unknown'",
        ] {
            let fixture = TempUsage::new();
            fs::write(
                &fixture.rollout,
                reviewed_codex_0_148_root_rollout(100).replace("2026-08-24", "2026-08-26"),
            )
            .unwrap();
            let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
            assert_eq!(initial.daily[&now.date()].observed_tokens, 100);
            Connection::open(&fixture.database)
                .unwrap()
                .execute_batch(&format!(
                    "UPDATE codex_usage_files
                     SET parser_version = {COMPATIBLE_ROLLOUT_PARSER_VERSION},
                         {unsafe_change};"
                ))
                .unwrap();

            let pending = index_local_usage_with_budget(
                &fixture.database,
                &fixture.root,
                now,
                ScanBudget {
                    max_bytes: 0,
                    max_file_bytes: 0,
                    max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                    max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
                },
            )
            .unwrap();

            assert_eq!(pending.scan_status, UsageScanStatus::Indexing);
            assert_eq!(
                Connection::open(&fixture.database)
                    .unwrap()
                    .query_row("SELECT parser_version FROM codex_usage_files", [], |row| {
                        row.get::<_, i64>(0)
                    },)
                    .unwrap(),
                COMPATIBLE_ROLLOUT_PARSER_VERSION,
                "{unsafe_change}"
            );
        }
    }

    #[test]
    fn sqlite_index_resumes_a_stored_current_parser_cursor_after_discovery_times_out() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-26T12:00:00Z", &Rfc3339).unwrap();
        let rollout = reviewed_codex_0_148_root_rollout(100).replace("2026-08-24", "2026-08-26");
        let first_record_bytes = u64::try_from(
            rollout
                .split_inclusive('\n')
                .next()
                .expect("the rollout must contain session metadata")
                .len(),
        )
        .unwrap();
        fs::write(&fixture.rollout, rollout).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);

        Connection::open(&fixture.database)
            .unwrap()
            .execute(
                "UPDATE codex_usage_files
                 SET parser_version = ?1,
                     completion_state = 'error',
                     parsed_offset = size_bytes,
                     schema_supported = 0,
                     accounting_ready = 0,
                     parser_error_seen = 1",
                [COMPATIBLE_ROLLOUT_PARSER_VERSION],
            )
            .unwrap();

        let partial = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: first_record_bytes,
                max_file_bytes: first_record_bytes,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert_eq!(partial.scan_status, UsageScanStatus::Indexing);
        let partial_cursor: (i64, String, u64, u64) = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT parser_version, completion_state, parsed_offset, size_bytes
                 FROM codex_usage_files",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        from_i64(row.get(3)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(partial_cursor.0, ROLLOUT_PARSER_VERSION);
        assert_eq!(partial_cursor.1, "indexing");
        assert!(partial_cursor.2 > 0);
        assert!(partial_cursor.2 < partial_cursor.3);

        let resumed = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: MAX_ROLLOUT_SCAN_BYTES,
                max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
                max_discovery_millis: 0,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();

        assert_eq!(resumed.scan_status, UsageScanStatus::Indexing);
        assert_eq!(
            resumed
                .daily
                .get(&now.date())
                .map(|day| day.observed_tokens),
            Some(100)
        );
        let completed_cursor: (String, u64, u64) = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT completion_state, parsed_offset, size_bytes FROM codex_usage_files",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(completed_cursor.0, "complete");
        assert_eq!(completed_cursor.1, completed_cursor.2);
    }

    #[test]
    fn sqlite_index_reads_an_appended_rollout_after_discovery_times_out() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);

        fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap()
            .write_all(appended_total(300).as_bytes())
            .unwrap();

        let resumed = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: MAX_ROLLOUT_SCAN_BYTES,
                max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
                max_discovery_millis: 0,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();

        assert_eq!(resumed.scan_status, UsageScanStatus::Indexing);
        assert_eq!(resumed.daily[&now.date()].observed_tokens, 300);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_index_rejects_a_stored_rollout_through_an_outward_directory_symlink() {
        let fixture = TempUsage::new();
        let external = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);

        fs::write(&external.rollout, root_rollout(900)).unwrap();
        fs::remove_file(&fixture.rollout).unwrap();
        fs::remove_dir(fixture.root.join("sessions")).unwrap();
        std::os::unix::fs::symlink(
            external.root.join("sessions"),
            fixture.root.join("sessions"),
        )
        .unwrap();

        let protected = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: MAX_ROLLOUT_SCAN_BYTES,
                max_file_bytes: MAX_ROLLOUT_FILE_SCAN_BYTES,
                max_discovery_millis: 0,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();

        assert_eq!(protected.scan_status, UsageScanStatus::Indexing);
        assert_eq!(protected.daily[&now.date()].observed_tokens, 100);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_index_rejects_a_discovered_rollout_through_an_outward_directory_symlink() {
        let fixture = TempUsage::new();
        let external = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);

        fs::write(&external.rollout, root_rollout(900)).unwrap();
        fs::remove_file(&fixture.rollout).unwrap();
        fs::remove_dir(fixture.root.join("sessions")).unwrap();
        std::os::unix::fs::symlink(
            external.root.join("sessions"),
            fixture.root.join("sessions"),
        )
        .unwrap();

        let protected = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(protected.daily[&now.date()].observed_tokens, 100);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_index_rejects_a_stored_rollout_through_an_inward_directory_symlink() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let sessions = fixture.root.join("sessions");
        let original_directory = sessions.join("original");
        let moved_directory = sessions.join("moved");
        fs::create_dir(&original_directory).unwrap();
        let original_rollout = original_directory.join("rollout.jsonl");
        fs::write(&original_rollout, root_rollout(100)).unwrap();
        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(initial.daily[&now.date()].observed_tokens, 100);

        fs::rename(&original_directory, &moved_directory).unwrap();
        std::os::unix::fs::symlink(&moved_directory, &original_directory).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(moved_directory.join("rollout.jsonl"))
            .unwrap()
            .write_all(appended_total(300).as_bytes())
            .unwrap();

        let protected = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(protected.daily[&now.date()].observed_tokens, 300);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_index_accepts_a_rollout_root_alias_that_stays_inside_the_codex_home() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let sessions = fixture.root.join("sessions");
        let aliased_sessions = fixture.root.join("stored-sessions");
        fs::rename(&sessions, &aliased_sessions).unwrap();
        std::os::unix::fs::symlink(&aliased_sessions, &sessions).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_index_counts_overlapping_inward_rollout_root_aliases_once() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let sessions = fixture.root.join("sessions");
        let archived_sessions = fixture.root.join("archived_sessions");
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        fs::create_dir(&archived_sessions).unwrap();
        fs::rename(&fixture.rollout, archived_sessions.join("rollout.jsonl")).unwrap();
        fs::remove_dir(&sessions).unwrap();
        std::os::unix::fs::symlink(&archived_sessions, &sessions).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 100);
        let stored_files: u64 = Connection::open(&fixture.database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM codex_usage_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored_files, 1);
    }

    #[test]
    fn sqlite_index_counts_a_reviewed_root_counter_reset_after_task_started() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let mut records = codex_0_148_task_reset_prefix();
        records.extend(codex_0_148_task_reset_counters());
        fs::write(&fixture.rollout, jsonl(records)).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the reviewed root counter reset must index");

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_265);
    }

    #[test]
    fn sqlite_index_counts_a_reviewed_codex_0_150_root_counter_reset() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let mut records = codex_0_148_task_reset_prefix();
        records.extend(codex_0_148_task_reset_counters());
        let rollout = jsonl(records).replace("0.148.0-alpha.21", "0.150.0-alpha.8");
        fs::write(&fixture.rollout, rollout).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the reviewed Codex 0.150 counter reset must index");

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_265);
    }

    #[test]
    fn sqlite_index_persists_a_pending_root_counter_reset_across_scan_passes() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let prefix = jsonl(codex_0_148_task_reset_prefix());
        let suffix = jsonl(codex_0_148_task_reset_counters());
        fs::write(&fixture.rollout, format!("{prefix}{suffix}")).unwrap();
        let prefix_bytes = u64::try_from(prefix.len()).unwrap();

        index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: prefix_bytes,
                max_file_bytes: prefix_bytes,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let reset_pending: bool = connection
            .query_row(
                "SELECT task_counter_reset_pending FROM codex_usage_files",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(reset_pending);
        drop(connection);

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_265);
    }

    #[test]
    fn sqlite_index_keeps_monotonic_counters_after_task_started() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let mut records = codex_0_148_task_reset_prefix();
        records.push(with_ordinal(
            token_count_usage_line(
                "2026-08-24T10:02:00Z",
                token_usage(1_100, 220, 0, 100),
                token_usage(90, 20, 0, 10),
            ),
            6,
        ));
        fs::write(&fixture.rollout, jsonl(records)).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_200);
    }

    #[test]
    fn sqlite_index_does_not_reset_for_only_lower_nested_counters() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let mut records = codex_0_148_task_reset_prefix();
        let nested_replay = TokenUsage {
            input: 1_000,
            cached_input: 190,
            cache_write_input: 0,
            output: 100,
            reasoning_output: 0,
            total: 1_100,
        };
        records.push(with_ordinal(
            token_count_usage_line("2026-08-24T10:02:00Z", nested_replay, nested_replay),
            6,
        ));
        fs::write(&fixture.rollout, jsonl(records)).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_100);
    }

    #[test]
    fn sqlite_index_accepts_a_larger_first_counter_after_root_task_started() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        let mut records = codex_0_148_task_reset_prefix();
        let new_epoch = token_usage(1_200, 250, 0, 200);
        records.push(with_ordinal(
            token_count_usage_line("2026-08-24T10:02:00Z", new_epoch, new_epoch),
            6,
        ));
        fs::write(&fixture.rollout, jsonl(records)).unwrap();

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 2_500);
    }

    #[test]
    fn sqlite_index_retries_the_v16_terminal_error_after_the_parser_update() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-24T12:00:00Z", &Rfc3339).unwrap();
        fs::write(&fixture.rollout, reviewed_codex_0_148_root_rollout(100)).unwrap();
        let metadata = fs::metadata(&fixture.rollout).unwrap();
        let mut connection = Connection::open(&fixture.database).unwrap();
        ensure_index_schema(&mut connection, Some(&fixture.database)).unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   parser_version, completion_state, schema_supported,
                   parser_error_seen, accounting_ready
                 ) VALUES(?1, ?2, ?3, ?4, ?3, 16, 'error', 0, 1, 0)",
                params![
                    fixture.rollout.to_string_lossy().as_ref(),
                    file_identity(&metadata),
                    i64::try_from(metadata.len()).unwrap(),
                    file_modified_ns(&metadata).unwrap(),
                ],
            )
            .unwrap();
        drop(connection);

        let indexed = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("the parser update must retry the settled v16 error row");

        assert_eq!(
            indexed
                .daily
                .get(&now.date())
                .map(|day| day.observed_tokens),
            Some(100)
        );
        assert_eq!(indexed.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn pending_parse_work_precedes_a_settled_dependency_recheck() {
        let fixture = TempUsage::new();
        let pending = fixture.root.join("sessions/pending-current.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let pending_modified_at = now - Duration::minutes(2);
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        fs::write(&pending, root_rollout(100)).unwrap();
        set_modified_at(&fixture.rollout, now - Duration::minutes(1));
        set_modified_at(&pending, pending_modified_at);
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .execute(
                "UPDATE codex_usage_files SET
                   lineage_mode = 'explicit-boundary',
                   parent_session_id = 'settled-parent',
                   fork_timestamp_ns = 1,
                   parent_dependency_key = 'stale-dependency',
                   history_start_ordinal = 1
                 WHERE path = ?1",
                [fixture.rollout.to_string_lossy().as_ref()],
            )
            .unwrap();
        drop(connection);

        let appended = appended_total(300);
        let appended_bytes = u64::try_from(appended.len()).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&pending)
            .unwrap()
            .write_all(appended.as_bytes())
            .unwrap();
        set_modified_at(&pending, pending_modified_at);

        index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: appended_bytes,
                max_file_bytes: appended_bytes,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();

        assert_eq!(indexed_tokens_for_path(&fixture.database, &pending), 300);
        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
    }

    #[test]
    fn a_pending_required_parent_precedes_its_pending_child() {
        let mut work = [
            ("child", true, true, false, 3_i64),
            ("parent", true, true, true, 1_i64),
            ("settled-parent", true, false, true, 4_i64),
        ];
        work.sort_by_key(|entry| rollout_work_priority(entry.1, entry.2, entry.3, entry.4));

        assert_eq!(
            work.map(|entry| entry.0),
            ["parent", "child", "settled-parent"]
        );
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
    fn sqlite_index_resumes_an_oversized_user_message_without_a_parser_error() {
        let fixture = TempUsage::new();
        let mut rollout = root_rollout(100);
        let first_pass_bytes = rollout.len() + MAX_ROLLOUT_LINE_BYTES + 1;
        rollout.push_str(
            r#"{"timestamp":"2026-08-06T10:02:00Z","type":"event_msg","payload":{"type":"user_message","message":""#,
        );
        rollout.extend(std::iter::repeat_n('x', MAX_ROLLOUT_LINE_BYTES + 1));
        rollout.push_str("\"}}\n");
        rollout.push_str(&appended_total(200));
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: u64::try_from(first_pass_bytes).unwrap(),
                max_file_bytes: u64::try_from(first_pass_bytes).unwrap(),
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let (first_state, first_error): (String, bool) = connection
            .query_row(
                "SELECT completion_state, parser_error_seen FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(connection);

        assert_eq!(first.daily[&now.date()].observed_tokens, 100);
        assert_eq!(first_state, "discarding-overlong-line");
        assert!(!first_error);

        let complete = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let (final_state, final_error): (String, bool) = connection
            .query_row(
                "SELECT completion_state, parser_error_seen FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(complete.daily[&now.date()].observed_tokens, 200);
        assert_eq!(final_state, "complete");
        assert!(!final_error);
    }

    #[test]
    fn sqlite_index_cleanly_fast_forwards_an_unresolved_subagent_from_an_unreviewed_version() {
        let fixture = TempUsage::new();
        let mut rollout = json!({
            "timestamp": "2026-08-06T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "cli_version": "0.152.0-alpha.1",
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
    fn sqlite_index_resumes_child_deltas_after_a_persisted_subagent_boundary() {
        let fixture = TempUsage::new();
        let token_count = |timestamp: &str, total: u64| {
            let input = total * 7 / 10;
            let output = total - input;
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 70,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 10,
                            "total_tokens": 100
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": input * 2 / 7,
                            "output_tokens": output,
                            "reasoning_output_tokens": output / 3,
                            "total_tokens": total
                        }
                    },
                    "rate_limits": null
                }
            })
        };
        let rollout = [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","source":{"subagent":{"thread_spawn":{}}}}}),
            token_count("2026-08-06T10:01:00Z", 1_000),
            json!({"timestamp":"2026-08-06T10:01:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:01:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            token_count("2026-08-06T10:02:00Z", 1_100),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = run_usage_passes(&fixture, now, 2);
        assert_eq!(first.daily[&now.date()].observed_tokens, 100);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        writeln!(file, "{}", token_count("2026-08-06T10:03:00Z", 1_200)).unwrap();
        drop(file);

        let second = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(second.daily[&now.date()].observed_tokens, 200);
        let connection = Connection::open(&fixture.database).unwrap();
        let (history_start, excluded): (Option<i64>, bool) = connection
            .query_row(
                "SELECT history_start_ordinal, usage_excluded FROM codex_usage_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(history_start.is_some());
        assert!(!excluded);
    }

    #[test]
    fn sqlite_index_resumes_a_paginated_child_from_nonzero_provider_ordinals() {
        let fixture = TempUsage::new();
        let with_ordinal = |mut line: serde_json::Value, ordinal: u64| {
            line["ordinal"] = json!(ordinal);
            line
        };
        let rollout = jsonl([
            json!({
                "ordinal": 41,
                "timestamp": "2026-08-06T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "history_base": {
                        "thread_id": "fixture-thread",
                        "end_ordinal_exclusive": 41,
                        "end_byte_offset": 0
                    },
                    "history_mode": "paginated",
                    "source": "subagent",
                    "subagent_history_start_ordinal": 44
                }
            }),
            json!({
                "ordinal": 42,
                "timestamp": "2026-08-06T09:00:00Z",
                "type": "response_item",
                "payload": { "type": "message", "role": "assistant" }
            }),
            with_ordinal(token_count_line("2026-08-06T09:01:00Z", 1_000, 100), 43),
            json!({
                "ordinal": 44,
                "timestamp": "2026-08-06T10:00:01Z",
                "type": "event_msg",
                "payload": { "type": "thread_settings_applied" }
            }),
            json!({
                "ordinal": 45,
                "timestamp": "2026-08-06T10:00:02Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
        ]);
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert!(first.daily.is_empty());
        assert_eq!(first.scan_status, UsageScanStatus::Complete);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        writeln!(
            file,
            "{}",
            with_ordinal(token_count_line("2026-08-06T10:01:00Z", 1_100, 100), 46)
        )
        .unwrap();
        drop(file);

        let second = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(second.daily[&now.date()].observed_tokens, 100);
        assert_eq!(second.scan_status, UsageScanStatus::Complete);
        let connection = Connection::open(&fixture.database).unwrap();
        let next_ordinal: u64 = connection
            .query_row("SELECT record_ordinal FROM codex_usage_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(next_ordinal, 47);
    }

    #[test]
    fn sqlite_index_rejects_a_missing_ordinal_after_a_paginated_resume() {
        let fixture = TempUsage::new();
        let rollout = jsonl([
            json!({
                "ordinal": 41,
                "timestamp": "2026-08-06T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "history_base": {
                        "thread_id": "fixture-thread",
                        "end_ordinal_exclusive": 41,
                        "end_byte_offset": 12
                    },
                    "history_mode": "paginated"
                }
            }),
            json!({
                "ordinal": 42,
                "timestamp": "2026-08-06T10:00:01Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
            with_ordinal(token_count_line("2026-08-06T10:01:00Z", 100, 100), 43),
        ]);
        fs::write(&fixture.rollout, rollout).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(first.scan_status, UsageScanStatus::Complete);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        writeln!(
            file,
            "{}",
            token_count_line("2026-08-06T10:02:00Z", 200, 100)
        )
        .unwrap();
        drop(file);

        let second = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(second.scan_status, UsageScanStatus::Unavailable);
        let completion: String = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT completion_state FROM codex_usage_files",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(completion, "error");
    }

    #[test]
    fn sqlite_index_counts_a_self_contained_first_child_counter_and_resumes() {
        let fixture = TempUsage::new();
        let prefix = [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","source":{"subagent":{"thread_spawn":{}}}}}),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:00:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, format!("{prefix}{}", appended_total(100))).unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let first = run_usage_passes(&fixture, now, 2);
        assert_eq!(first.daily[&now.date()].observed_tokens, 100);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        file.write_all(appended_total(200).as_bytes()).unwrap();
        drop(file);

        let second = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(second.daily[&now.date()].observed_tokens, 200);
    }

    #[test]
    fn sqlite_index_subtracts_one_complete_parent_snapshot_from_a_copied_child() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/copied-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("parent-fixture", 1_000, 1_200),
        )
        .unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        fs::write(
            &child,
            copied_child_rollout("child-fixture", "parent-fixture", 1_100),
        )
        .unwrap();

        let indexed = run_usage_passes(&fixture, now, 3);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        assert_eq!(indexed.daily[&now.date()].observed_tokens, 1_300);
        assert!(!indexed.has_excluded_usage);
    }

    #[test]
    fn sqlite_index_ignores_invalid_last_usage_in_an_unowned_copied_prefix() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let rollout = jsonl([
            session_meta_line(
                "2026-08-06T10:05:00Z",
                "child-with-invalid-prefix",
                Some("parent-with-invalid-prefix"),
                true,
            ),
            session_meta_line(
                "2026-08-06T10:00:00Z",
                "parent-with-invalid-prefix",
                None,
                false,
            ),
            token_count_line("2026-08-06T10:01:00Z", 1_000, 1_000),
            json!({
                "timestamp": "2026-08-06T10:02:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 35,
                            "cached_input_tokens": 10,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 15,
                            "reasoning_output_tokens": 5,
                            "total_tokens": 49
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": 735,
                            "cached_input_tokens": 0,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 315,
                            "reasoning_output_tokens": 0,
                            "total_tokens": 1_050
                        }
                    },
                    "rate_limits": null
                }
            }),
            json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:05:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            token_count_line("2026-08-06T10:06:00Z", 1_150, 100),
        ]);
        fs::write(&fixture.rollout, rollout).unwrap();

        run_usage_passes(&fixture, now, 3);

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
        let (completion_state, parser_error_seen): (String, bool) =
            Connection::open(&fixture.database)
                .unwrap()
                .query_row(
                    "SELECT completion_state, parser_error_seen FROM codex_usage_files",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
        assert_eq!(completion_state, "complete");
        assert!(!parser_error_seen);
    }

    #[test]
    fn sqlite_index_restarts_from_a_strong_counter_after_a_copied_prefix() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            copied_child_strong_reset_rollout("reset-child", "reset-parent"),
        )
        .unwrap();

        run_usage_passes(&fixture, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        writeln!(
            file,
            "{}",
            token_count_line("2026-08-06T10:08:00Z", 200, 100)
        )
        .unwrap();
        drop(file);

        run_usage_passes(&fixture, now, 2);
        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            200
        );
        let (completion_state, accounting_ready, parser_error_seen): (String, bool, bool) =
            Connection::open(&fixture.database)
                .unwrap()
                .query_row(
                    "SELECT completion_state, accounting_ready, parser_error_seen
                     FROM codex_usage_files WHERE path = ?1",
                    [fixture.rollout.to_string_lossy().as_ref()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
        assert_eq!(completion_state, "complete");
        assert!(accounting_ready);
        assert!(!parser_error_seen);
    }

    #[test]
    fn sqlite_index_keeps_unresolved_parent_snapshots_excluded() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let missing = TempUsage::new();
        fs::write(
            &missing.rollout,
            copied_child_rollout("missing-child", "missing-parent", 1_100),
        )
        .unwrap();
        let missing_result = run_usage_passes(&missing, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&missing.database, &missing.rollout),
            0
        );
        assert!(missing_result.has_excluded_usage);

        let ambiguous = TempUsage::new();
        let duplicate_parent = ambiguous.root.join("sessions/duplicate-parent.jsonl");
        let ambiguous_child = ambiguous.root.join("sessions/ambiguous-child.jsonl");
        fs::write(
            &ambiguous.rollout,
            parent_snapshot_rollout("shared-parent", 1_000, 1_200),
        )
        .unwrap();
        fs::write(
            &duplicate_parent,
            parent_snapshot_rollout("shared-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&ambiguous, now, 2);
        fs::write(
            &ambiguous_child,
            copied_child_rollout("ambiguous-child", "shared-parent", 1_100),
        )
        .unwrap();
        let ambiguous_result = run_usage_passes(&ambiguous, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&ambiguous.database, &ambiguous_child),
            0
        );
        assert!(ambiguous_result.has_excluded_usage);

        let stale = TempUsage::new();
        let stale_child = stale.root.join("sessions/stale-child.jsonl");
        fs::write(
            &stale.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "stale-parent", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
            ]),
        )
        .unwrap();
        run_usage_passes(&stale, now, 2);
        fs::write(
            &stale_child,
            copied_child_rollout("stale-child", "stale-parent", 1_100),
        )
        .unwrap();
        let stale_result = run_usage_passes(&stale, now, 3);
        assert_eq!(indexed_tokens_for_path(&stale.database, &stale_child), 100);
        assert!(!stale_result.has_excluded_usage);
    }

    #[test]
    fn old_excluded_rollouts_do_not_reduce_recent_usage_coverage() {
        let fixture = TempUsage::new();
        let current_rollout = fixture.root.join("sessions/current.jsonl");
        let now = OffsetDateTime::parse("2026-08-10T12:00:00Z", &Rfc3339).unwrap();
        let old_modified_at = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            copied_child_rollout("missing-child", "missing-parent", 1_100),
        )
        .unwrap();
        set_modified_at(&fixture.rollout, old_modified_at);
        fs::write(
            &current_rollout,
            root_rollout(100).replace("2026-08-06", "2026-08-10"),
        )
        .unwrap();
        set_modified_at(&current_rollout, now - Duration::minutes(1));

        let local = run_usage_passes(&fixture, now, 3);
        let periods = project_usage_periods(None, Some(&local), now);
        let UsageTotal::Current { coverage, .. } = periods.today else {
            panic!("recent local usage must remain available");
        };

        assert!(local.has_excluded_usage);
        assert_eq!(coverage, UsageCoverage::Complete);
    }

    #[test]
    fn recent_excluded_rollouts_reduce_recent_usage_coverage() {
        let fixture = TempUsage::new();
        let current_rollout = fixture.root.join("sessions/current.jsonl");
        let now = OffsetDateTime::parse("2026-08-10T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            copied_child_rollout("missing-child", "missing-parent", 1_100),
        )
        .unwrap();
        set_modified_at(&fixture.rollout, now - Duration::minutes(1));
        fs::write(
            &current_rollout,
            root_rollout(100).replace("2026-08-06", "2026-08-10"),
        )
        .unwrap();
        set_modified_at(&current_rollout, now - Duration::minutes(1));

        let local = run_usage_passes(&fixture, now, 3);
        let periods = project_usage_periods(None, Some(&local), now);
        let UsageTotal::Current { coverage, .. } = periods.today else {
            panic!("recent local usage must remain available");
        };

        assert!(local.has_excluded_usage);
        assert_eq!(coverage, UsageCoverage::Partial);
    }

    #[test]
    fn sqlite_index_rejects_incomplete_and_arithmetic_invalid_parent_sources() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();

        let incomplete = TempUsage::new();
        let incomplete_child = incomplete.root.join("sessions/incomplete-child.jsonl");
        let mut partial_parent = parent_snapshot_rollout("partial-parent", 1_000, 1_200);
        partial_parent.pop();
        fs::write(&incomplete.rollout, partial_parent).unwrap();
        run_usage_passes(&incomplete, now, 2);
        fs::write(
            &incomplete_child,
            copied_child_rollout("partial-child", "partial-parent", 1_100),
        )
        .unwrap();
        let incomplete_result = run_usage_passes(&incomplete, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&incomplete.database, &incomplete_child),
            0
        );
        assert!(incomplete_result.has_excluded_usage);

        let invalid = TempUsage::new();
        let invalid_child = invalid.root.join("sessions/invalid-child.jsonl");
        fs::write(
            &invalid.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "invalid-parent", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
                token_count_line("2026-08-06T10:06:00Z", 900, 100),
            ]),
        )
        .unwrap();
        run_usage_passes(&invalid, now, 2);
        fs::write(
            &invalid_child,
            copied_child_rollout("invalid-child", "invalid-parent", 1_100),
        )
        .unwrap();
        let invalid_result = run_usage_passes(&invalid, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&invalid.database, &invalid_child),
            0
        );
        assert!(invalid_result.has_excluded_usage);
    }

    #[test]
    fn sqlite_index_rebuilds_a_dependent_child_when_its_parent_changes() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/restart-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("restart-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        fs::write(
            &child,
            copied_child_rollout("restart-child", "restart-parent", 1_100),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 3);
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        let first_dependency: String = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT parent_dependency_key FROM codex_usage_files WHERE path = ?1",
                [child.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();

        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("restart-parent", 900, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 4);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 200);
        let second_dependency: String = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT parent_dependency_key FROM codex_usage_files WHERE path = ?1",
                [child.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(first_dependency, second_dependency);
    }

    #[test]
    fn sqlite_index_rechecks_a_parent_confirmed_marker_when_its_parent_changes() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/parent-confirmed-marker.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("marker-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        fs::write(
            &child,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:05:00Z",
                    "marker-child",
                    Some("marker-parent"),
                    true,
                ),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:05:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                token_count_line("2026-08-06T10:07:00Z", 1_100, 50),
            ]),
        )
        .unwrap();
        set_modified_at(&child, now - Duration::minutes(2));

        run_usage_passes(&fixture, now, 3);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        let connection = Connection::open(&fixture.database).unwrap();
        let (lineage_mode, has_dependency, accounting_ready): (String, bool, bool) = connection
            .query_row(
                "SELECT lineage_mode, parent_dependency_key IS NOT NULL, accounting_ready
                 FROM codex_usage_files WHERE path = ?1",
                [child.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(lineage_mode, "explicit-boundary");
        assert!(has_dependency);
        assert!(accounting_ready);
        drop(connection);

        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("marker-parent", 900, 1_200),
        )
        .unwrap();
        set_modified_at(&fixture.rollout, now - Duration::minutes(1));

        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 0);
        let connection = Connection::open(&fixture.database).unwrap();
        let (lineage_mode, accounting_ready): (String, bool) = connection
            .query_row(
                "SELECT lineage_mode, accounting_ready
                 FROM codex_usage_files WHERE path = ?1",
                [child.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(lineage_mode, "independent");
        assert!(!accounting_ready);
        drop(connection);

        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 1_100);
    }

    #[test]
    fn sqlite_index_waits_for_full_metadata_before_classifying_a_subagent_counter() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/late-ancestor.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("late-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        let child_contents = copied_child_rollout("late-child", "late-parent", 1_100);
        let first_line_bytes = child_contents.lines().next().unwrap().len() as u64 + 1;
        fs::write(&child, child_contents).unwrap();

        index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: first_line_bytes,
                max_file_bytes: first_line_bytes,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 0);

        run_usage_passes(&fixture, now, 3);
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
    }

    #[test]
    fn sqlite_index_classifies_one_full_metadata_identity_as_an_independent_counter() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let independent = TempUsage::new();
        fs::write(
            &independent.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "independent-child", None, true),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 100, 100),
            ]),
        )
        .unwrap();
        run_usage_passes(&independent, now, 2);
        assert_eq!(
            indexed_tokens_for_path(&independent.database, &independent.rollout),
            100
        );

        let hostile = TempUsage::new();
        fs::write(
            &hostile.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "hostile-child", None, true),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 200, 100),
                token_count_line("2026-08-06T10:02:00Z", 250, 50),
            ]),
        )
        .unwrap();
        run_usage_passes(&hostile, now, 2);
        assert_eq!(
            indexed_tokens_for_path(&hostile.database, &hostile.rollout),
            50
        );
    }

    #[test]
    fn parent_dependency_data_is_absent_from_the_sanitized_debug_report() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/private-child-name.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("private-parent-key", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        fs::write(
            &child,
            copied_child_rollout("private-child-key", "private-parent-key", 1_100),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 3);

        let report = debug_usage_pass(&fixture.database, &fixture.root, now).unwrap();

        for private_value in [
            "private-parent-key",
            "private-child-key",
            "private-child-name",
        ] {
            assert!(!report.contains(private_value));
        }
    }

    #[test]
    fn independent_replay_stays_hidden_until_the_full_file_is_ready() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let rollout = jsonl([
            session_meta_line("2026-08-06T10:00:00Z", "hidden-child", None, true),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            token_count_line("2026-08-06T10:01:00Z", 100, 100),
        ]);
        let full_size = rollout.len() as u64;
        let first_line = rollout.lines().next().unwrap().len() as u64 + 1;
        fs::write(&fixture.rollout, rollout).unwrap();

        let discovered = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: full_size,
                max_file_bytes: full_size,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert!(discovered.daily.is_empty());
        let partial_replay = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: first_line,
                max_file_bytes: first_line,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert!(partial_replay.daily.is_empty());

        let ready = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(ready.daily[&now.date()].observed_tokens, 100);
    }

    #[test]
    fn fresh_index_resolves_a_current_child_from_an_older_parent_file() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/current-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("retained-parent", 1_000, 1_200),
        )
        .unwrap();
        set_modified_at(
            &fixture.rollout,
            OffsetDateTime::parse("2026-06-01T00:00:00Z", &Rfc3339).unwrap(),
        );
        fs::write(
            &child,
            copied_child_rollout("current-child", "retained-parent", 1_100),
        )
        .unwrap();

        let first = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(first.scan_status, UsageScanStatus::Indexing);
        let ready = run_usage_passes(&fixture, now, 2);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        assert!(!ready.has_excluded_usage);
    }

    #[test]
    fn parent_resolved_child_keeps_the_inherited_baseline_across_small_post_fork_totals() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/pre-fork-reset-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("pre-fork-reset-parent", 1_000, 1_200),
        )
        .unwrap();
        fs::write(
            &child,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:05:00Z",
                    "pre-fork-reset-child",
                    Some("pre-fork-reset-parent"),
                    true,
                ),
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "pre-fork-reset-parent",
                    None,
                    false,
                ),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:03:00Z", 1_500, 500),
                token_count_line("2026-08-06T10:04:00Z", 1_200, 200),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:06:00Z", 25, 25),
                token_count_line("2026-08-06T10:07:00Z", 60, 60),
            ]),
        )
        .unwrap();

        run_usage_passes(&fixture, now, 3);
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 0);

        let mut child_file = fs::OpenOptions::new().append(true).open(&child).unwrap();
        writeln!(
            child_file,
            "{}",
            token_count_line("2026-08-06T10:08:00Z", 1_050, 990)
        )
        .unwrap();
        writeln!(
            child_file,
            "{}",
            token_count_line("2026-08-06T10:09:00Z", 1_100, 50)
        )
        .unwrap();
        drop(child_file);

        run_usage_passes(&fixture, now, 2);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        let (completion_state, parser_error_seen, parent_baseline_retained): (String, bool, bool) =
            Connection::open(&fixture.database)
                .unwrap()
                .query_row(
                    "SELECT completion_state, parser_error_seen,
                            parent_baseline_total IS NOT NULL
                     FROM codex_usage_files WHERE path = ?1",
                    [child.to_string_lossy().as_ref()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
        assert_eq!(completion_state, "complete");
        assert!(!parser_error_seen);
        assert!(parent_baseline_retained);
    }

    #[test]
    fn parent_resolution_uses_the_contained_cumulative_watermark() {
        let fixture = TempUsage::new();
        let child = fixture
            .root
            .join("sessions/contained-watermark-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "contained-watermark-parent",
                    None,
                    false,
                ),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:03:00Z", 1_000, 1_000),
                token_count_line("2026-08-06T10:04:00Z", 100, 100),
            ]),
        )
        .unwrap();
        fs::write(
            &child,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:05:00Z",
                    "contained-watermark-child",
                    Some("contained-watermark-parent"),
                    true,
                ),
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "contained-watermark-parent",
                    None,
                    false,
                ),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:06:00Z", 1_100, 100),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 3);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        assert_eq!(observation.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn missing_parent_child_accepts_a_last_only_pre_boundary_record() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-11T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line(
                    "2026-08-11T10:00:00Z",
                    "last-only-child",
                    Some("missing-parent"),
                    true,
                ),
                json!({"timestamp":"2026-08-11T10:00:00.100Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"cache_write_input_tokens":0,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":1100},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"cache_write_input_tokens":0,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":1100},"model_context_window":1050000},"rate_limits":null}}),
                json!({"timestamp":"2026-08-11T10:00:00.200Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":7,"cached_input_tokens":3,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":9},"model_context_window":1050000},"rate_limits":null}}),
                json!({"timestamp":"2026-08-11T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-11T10:00:01Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                json!({"timestamp":"2026-08-11T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":55},"total_token_usage":{"input_tokens":1050,"cached_input_tokens":910,"cache_write_input_tokens":0,"output_tokens":105,"reasoning_output_tokens":0,"total_tokens":1155},"model_context_window":1050000},"rate_limits":null}}),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 3);

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            55
        );
        assert!(!observation.has_excluded_usage);
    }

    #[test]
    fn owned_last_only_snapshot_does_not_report_complete() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-11T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-11T10:00:00Z", "last-only-root", None, false),
                json!({"timestamp":"2026-08-11T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-11T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":0,"total_tokens":100},"model_context_window":1050000},"rate_limits":null}}),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 2);
        let (completion_state, parser_error_seen): (String, bool) =
            Connection::open(&fixture.database)
                .unwrap()
                .query_row(
                    "SELECT completion_state, parser_error_seen
                     FROM codex_usage_files WHERE path = ?1",
                    [fixture.rollout.to_string_lossy().as_ref()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();

        assert_ne!(observation.scan_status, UsageScanStatus::Complete);
        assert_eq!(completion_state, "error");
        assert!(parser_error_seen);
    }

    #[test]
    fn root_rollout_contains_interleaved_cumulative_snapshots() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-12T12:00:00Z", &Rfc3339).unwrap();
        let mut contained = token_count_line("2026-08-12T10:00:02Z", 5_000, 5_000);
        contained["payload"]["info"]["model"] = json!("interleaved-model");
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-12T10:00:00Z", "interleaved-root", None, false),
                json!({"timestamp":"2026-08-12T10:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-12T10:00:01Z", 100_000, 100_000),
                contained,
                token_count_line("2026-08-12T10:00:03Z", 101_000, 101_000),
                token_count_line("2026-08-12T10:00:04Z", 6_000, 6_000),
                token_count_line("2026-08-12T10:00:05Z", 102_000, 102_000),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 2);

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            102_000
        );
        let model_rows = Connection::open(&fixture.database)
            .unwrap()
            .prepare(
                "SELECT model, SUM(observed_tokens) FROM codex_usage_file_model_days
                 WHERE path = ?1 GROUP BY model ORDER BY model",
            )
            .unwrap()
            .query_map([fixture.rollout.to_string_lossy().as_ref()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(model_rows, vec![("gpt-5.6-sol".to_owned(), 102_000)]);
        assert_eq!(observation.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn token_event_accepts_known_model_metadata_without_a_turn_context() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-12T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-12T10:00:00Z", "event-model-root", None, false),
                json!({
                    "timestamp": "2026-08-12T10:00:01Z",
                    "type": "event_msg",
                    "model": "root-fallback",
                    "payload": {
                        "type": "token_count",
                        "model": "payload-fallback",
                        "model_name": "accepted-but-not-selected",
                        "info": {
                            "model": " gpt-5.6-sol ",
                            "model_name": "info-fallback",
                            "last_token_usage": {"input_tokens":70,"cached_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":0,"total_tokens":100},
                            "model_context_window": 1_050_000,
                            "total_token_usage": {"input_tokens":70,"cached_input_tokens":0,"output_tokens":30,"reasoning_output_tokens":0,"total_tokens":100}
                        },
                        "rate_limits": null
                    }
                }),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 2);
        let (model, tokens, complete): (String, u64, bool) = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT model, observed_tokens, complete
                 FROM codex_usage_file_model_days WHERE path = ?1",
                [fixture.rollout.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(model, "gpt-5.6-sol");
        assert_eq!(tokens, 100);
        assert!(complete);
        assert_eq!(observation.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn token_event_ignores_a_null_usage_snapshot() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-12T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-12T10:00:00Z", "null-usage-root", None, false),
                json!({"timestamp":"2026-08-12T10:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-12T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}),
                token_count_line("2026-08-12T10:00:02Z", 100, 100),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 2);
        let (completion_state, parser_error_seen): (String, bool) =
            Connection::open(&fixture.database)
                .unwrap()
                .query_row(
                    "SELECT completion_state, parser_error_seen
                     FROM codex_usage_files WHERE path = ?1",
                    [fixture.rollout.to_string_lossy().as_ref()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
        assert_eq!(completion_state, "complete");
        assert!(!parser_error_seen);
        assert_eq!(observation.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn cumulative_snapshot_without_last_uses_its_delta_for_pricing() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-12T12:00:00Z", &Rfc3339).unwrap();
        let total_only = |timestamp, input| json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":1050000,"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":input}},"rate_limits":null}});
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-12T10:00:00Z", "total-only-root", None, false),
                json!({"timestamp":"2026-08-12T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                total_only("2026-08-12T10:01:00Z", 100_000),
                total_only("2026-08-12T10:02:00Z", 400_000),
            ]),
        )
        .unwrap();

        let observation = run_usage_passes(&fixture, now, 2);
        let day = &observation.daily[&now.date()];
        assert_eq!(day.observed_tokens, 400_000);
        assert!((day.api_equivalent_cost_usd.unwrap() - 3.5).abs() < 1e-12);
        assert_eq!(observation.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn current_parent_filename_is_prioritized_before_newer_rollouts() {
        let fixture = TempUsage::new();
        let sessions = fixture.root.join("sessions");
        let child = sessions.join("current-priority-child.jsonl");
        let parent = sessions.join("rollout-current-priority-parent.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let parent_modified_at = OffsetDateTime::parse("2026-08-01T00:00:00Z", &Rfc3339).unwrap();
        let newer_modified_at = OffsetDateTime::parse("2026-08-06T11:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &child,
            copied_child_rollout("current-priority-child", "current-priority-parent", 1_100),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 0);

        for index in 0..16 {
            let path = sessions.join(format!("newer-{index:02}.jsonl"));
            fs::write(
                &path,
                parent_snapshot_rollout(&format!("newer-{index:02}"), 1_000, 1_200),
            )
            .unwrap();
            set_modified_at(&path, newer_modified_at);
        }
        fs::write(
            &parent,
            parent_snapshot_rollout("current-priority-parent", 1_000, 1_200),
        )
        .unwrap();
        set_modified_at(&parent, parent_modified_at);

        let fixed_budget = 4_096_u64;
        let mut resolved_after = None;
        for pass in 1..=3 {
            index_local_usage_with_budget(
                &fixture.database,
                &fixture.root,
                now,
                ScanBudget {
                    max_bytes: fixed_budget,
                    max_file_bytes: fixed_budget,
                    max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                    max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
                },
            )
            .unwrap();
            if indexed_tokens_for_path(&fixture.database, &child) == 100 {
                resolved_after = Some(pass);
                break;
            }
        }

        assert!(resolved_after.is_some_and(|passes| passes <= 3));
    }

    #[test]
    fn fixed_budget_required_parent_discovery_converges() {
        let fixture = TempUsage::new();
        let sessions = fixture.root.join("sessions");
        let required_parent = sessions.join("zz-required-parent.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let old_modified_at = OffsetDateTime::parse("2026-06-01T00:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            copied_child_rollout("budget-child", "required-parent", 1_100),
        )
        .unwrap();
        for index in 0..16 {
            let path = sessions.join(format!("old-{index:02}.jsonl"));
            fs::write(
                &path,
                parent_snapshot_rollout(&format!("unrelated-{index:02}"), 1_000, 1_200),
            )
            .unwrap();
            set_modified_at(&path, old_modified_at);
        }
        fs::write(
            &required_parent,
            parent_snapshot_rollout("required-parent", 1_000, 1_200),
        )
        .unwrap();
        set_modified_at(&required_parent, old_modified_at);

        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let fixed_budget = 1_024_u64;
        let mut resolved_after = None;
        for pass in 1..=4 {
            reset_required_parent_probe_bytes();
            let observation = index_local_usage_with_budget(
                &fixture.database,
                &fixture.root,
                now,
                ScanBudget {
                    max_bytes: fixed_budget,
                    max_file_bytes: fixed_budget,
                    max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                    max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
                },
            )
            .unwrap();
            assert!(required_parent_probe_bytes() <= fixed_budget);
            if indexed_tokens_for_path(&fixture.database, &fixture.rollout) == 100
                && !observation.has_excluded_usage
            {
                resolved_after = Some(pass);
                break;
            }
        }

        assert!(resolved_after.is_some_and(|passes| passes <= 4));
        reset_required_parent_probe_bytes();
        let settled = index_local_usage_with_budget(
            &fixture.database,
            &fixture.root,
            now,
            ScanBudget {
                max_bytes: fixed_budget,
                max_file_bytes: fixed_budget,
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
            },
        )
        .unwrap();
        assert_eq!(required_parent_probe_bytes(), 0);
        assert_eq!(settled.scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn stable_partial_parent_requires_clean_monotonic_evidence_through_the_fork() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let clean = TempUsage::new();
        let clean_child = clean.root.join("sessions/clean-partial-child.jsonl");
        let mut clean_parent = parent_snapshot_rollout("clean-partial-parent", 1_000, 1_200);
        clean_parent.push_str("{\"partial\"");
        fs::write(&clean.rollout, clean_parent).unwrap();
        run_usage_passes(&clean, now, 2);
        fs::write(
            &clean_child,
            copied_child_rollout("clean-partial-child", "clean-partial-parent", 1_100),
        )
        .unwrap();
        run_usage_passes(&clean, now, 3);
        assert_eq!(indexed_tokens_for_path(&clean.database, &clean_child), 100);

        let bad = TempUsage::new();
        let bad_child = bad.root.join("sessions/bad-partial-child.jsonl");
        let mut bad_parent = parent_snapshot_rollout("bad-partial-parent", 1_000, 1_200);
        bad_parent.push_str("not-json\n{\"partial\"");
        fs::write(&bad.rollout, bad_parent).unwrap();
        run_usage_passes(&bad, now, 2);
        fs::write(
            &bad_child,
            copied_child_rollout("bad-partial-child", "bad-partial-parent", 1_100),
        )
        .unwrap();
        let excluded = run_usage_passes(&bad, now, 3);
        assert_eq!(indexed_tokens_for_path(&bad.database, &bad_child), 0);
        assert!(excluded.has_excluded_usage);
    }

    #[test]
    fn appended_same_leaf_lineage_replaces_an_independent_accounting_pass() {
        let fixture = TempUsage::new();
        let parent = fixture.root.join("sessions/enriched-parent.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "enriched-child", None, true),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 100, 100),
            ]),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
        fs::write(
            &parent,
            parent_snapshot_rollout("enriched-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.rollout)
            .unwrap();
        file.write_all(
            jsonl([
                session_meta_line(
                    "2026-08-06T10:05:00Z",
                    "enriched-child",
                    Some("enriched-parent"),
                    true,
                ),
                session_meta_line("2026-08-06T10:00:00Z", "enriched-parent", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:07:00Z", 1_100, 100),
            ])
            .as_bytes(),
        )
        .unwrap();
        drop(file);

        run_usage_passes(&fixture, now, 3);
        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
    }

    #[test]
    fn parent_subtraction_saturates_cached_and_reasoning_components_independently() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/mixed-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let baseline = TokenUsage {
            input: 700,
            cached_input: 500,
            cache_write_input: 0,
            output: 300,
            reasoning_output: 200,
            total: 1_000,
        };
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "mixed-parent", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_usage_line("2026-08-06T10:04:00Z", baseline, baseline),
            ]),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        let child_total = TokenUsage {
            input: 770,
            cached_input: 400,
            cache_write_input: 0,
            output: 330,
            reasoning_output: 100,
            total: 1_100,
        };
        fs::write(
            &child,
            jsonl([
                session_meta_line("2026-08-06T10:05:00Z", "mixed-child", Some("mixed-parent"), true),
                session_meta_line("2026-08-06T10:00:00Z", "mixed-parent", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_usage_line("2026-08-06T10:04:00Z", baseline, baseline),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_usage_line(
                    "2026-08-06T10:07:00Z",
                    child_total,
                    token_usage(70, 0, 0, 30),
                ),
            ]),
        )
        .unwrap();

        run_usage_passes(&fixture, now, 3);
        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
    }

    #[test]
    fn self_and_direct_cycle_parent_lineage_stays_excluded() {
        let self_parent = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &self_parent.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "same-key", Some("same-key"), true),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 100, 100),
            ]),
        )
        .unwrap();
        let self_result = run_usage_passes(&self_parent, now, 2);
        assert_eq!(
            indexed_tokens_for_path(&self_parent.database, &self_parent.rollout),
            0
        );
        assert!(self_result.has_excluded_usage);

        let cycle = TempUsage::new();
        let cycle_child = cycle.root.join("sessions/cycle-child.jsonl");
        fs::write(
            &cycle.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:00:00Z", "cycle-parent", Some("cycle-child"), true),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
            ]),
        )
        .unwrap();
        run_usage_passes(&cycle, now, 2);
        fs::write(
            &cycle_child,
            copied_child_rollout("cycle-child", "cycle-parent", 1_100),
        )
        .unwrap();
        let cycle_result = run_usage_passes(&cycle, now, 3);
        assert_eq!(indexed_tokens_for_path(&cycle.database, &cycle_child), 0);
        assert!(cycle_result.has_excluded_usage);
    }

    #[test]
    fn malformed_parent_ids_fail_closed_on_initial_and_appended_metadata() {
        let malformed: RawSessionMeta = serde_json::from_value(json!({
            "cli_version": "0.146.0-alpha.3.1",
            "id": "malformed-child",
            "forked_from_id": "",
            "source": {"subagent": {"thread_spawn": {}}}
        }))
        .unwrap();
        let mut initial = RolloutScanState::default();
        apply_session_metadata(&mut initial, malformed, "2026-08-06T10:05:00Z").unwrap();
        assert!(initial.lineage_invalid);
        assert!(initial.exclude_usage);

        let malformed_append: RawSessionMeta = serde_json::from_value(json!({
            "cli_version": "0.146.0-alpha.3.1",
            "id": "malformed-child",
            "forked_from_id": ""
        }))
        .unwrap();
        let mut appended = RolloutScanState {
            baseline_is_inherited: Some(true),
            schema_supported: true,
            lineage_mode: LineageMode::Independent,
            leaf_session_id: Some("malformed-child".to_owned()),
            fork_timestamp_ns: timestamp_ns("2026-08-06T10:05:00Z").ok(),
            ..RolloutScanState::default()
        };
        apply_session_metadata(&mut appended, malformed_append, "2026-08-06T10:06:00Z").unwrap();
        assert_eq!(appended.lineage_mode, LineageMode::Discovering);
        assert!(appended.lineage_invalid);
        assert!(appended.exclude_usage);
    }

    #[test]
    fn changed_same_parent_fork_time_restarts_lineage_classification() {
        let corrected: RawSessionMeta = serde_json::from_value(json!({
            "cli_version": "0.146.0-alpha.3.1",
            "id": "corrected-child",
            "forked_from_id": "corrected-parent"
        }))
        .unwrap();
        let old_fork = timestamp_ns("2026-08-06T10:05:00Z").unwrap();
        let new_fork = timestamp_ns("2026-08-06T10:07:00Z").unwrap();
        let mut state = RolloutScanState {
            baseline_is_inherited: Some(true),
            schema_supported: true,
            lineage_mode: LineageMode::ParentResolved,
            leaf_session_id: Some("corrected-child".to_owned()),
            parent_session_id: Some("corrected-parent".to_owned()),
            fork_timestamp_ns: Some(old_fork),
            parent_dependency_key: Some("stale-dependency".to_owned()),
            parent_baseline: Some(token_usage(700, 0, 0, 300)),
            ..RolloutScanState::default()
        };

        apply_session_metadata(&mut state, corrected, "2026-08-06T10:07:00Z").unwrap();

        assert_eq!(state.lineage_mode, LineageMode::Discovering);
        assert_eq!(state.fork_timestamp_ns, Some(new_fork));
        assert!(state.exclude_usage);
        assert!(state.parent_dependency_key.is_none());
        assert!(state.parent_baseline.is_none());
    }

    #[test]
    fn explicit_direct_parent_stays_authoritative_through_nested_ancestor_metadata() {
        let fixture = TempUsage::new();
        let child = fixture.root.join("sessions/nested-child.jsonl");
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            parent_snapshot_rollout("nested-parent", 1_000, 1_200),
        )
        .unwrap();
        run_usage_passes(&fixture, now, 2);
        fs::write(
            &child,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:05:00Z",
                    "nested-child",
                    Some("nested-parent"),
                    true,
                ),
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "nested-parent",
                    Some("nested-grandparent"),
                    true,
                ),
                session_meta_line(
                    "2026-08-06T09:00:00Z",
                    "nested-grandparent",
                    None,
                    false,
                ),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
                json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:07:00Z", 1_100, 100),
            ]),
        )
        .unwrap();

        let result = run_usage_passes(&fixture, now, 3);

        assert_eq!(indexed_tokens_for_path(&fixture.database, &child), 100);
        assert!(!result.has_excluded_usage);
    }

    #[test]
    fn two_inferred_ancestor_identities_stay_ambiguous_and_excluded() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line("2026-08-06T10:05:00Z", "ambiguous-leaf", None, true),
                session_meta_line("2026-08-06T10:00:00Z", "ancestor-one", None, false),
                session_meta_line("2026-08-06T09:00:00Z", "ancestor-two", None, false),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
                token_count_line("2026-08-06T10:07:00Z", 1_100, 100),
            ]),
        )
        .unwrap();

        let result = run_usage_passes(&fixture, now, 3);

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            0
        );
        assert!(result.has_excluded_usage);
    }

    #[test]
    fn first_confirmed_leaf_marker_owns_all_later_leaf_turns() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "multi-marker-child",
                    Some("multi-marker-parent"),
                    true,
                ),
                token_count_line("2026-08-06T10:01:00Z", 1_000, 1_000),
                json!({"timestamp":"2026-08-06T10:01:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:01:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                token_count_line("2026-08-06T10:02:00Z", 1_100, 100),
                json!({"timestamp":"2026-08-06T10:02:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:02:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                token_count_line("2026-08-06T10:03:00Z", 1_200, 100),
            ]),
        )
        .unwrap();

        let result = run_usage_passes(&fixture, now, 2);
        let history_start: i64 = Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT history_start_ordinal FROM codex_usage_files WHERE path = ?1",
                [fixture.rollout.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(result.daily[&now.date()].observed_tokens, 200);
        assert_eq!(history_start, 4);
    }

    #[test]
    fn marker_after_final_ancestor_replaces_a_tentative_copied_prefix_marker() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        fs::write(
            &fixture.rollout,
            jsonl([
                session_meta_line(
                    "2026-08-06T10:00:00Z",
                    "late-marker-child",
                    Some("late-marker-parent"),
                    true,
                ),
                token_count_line("2026-08-06T10:01:00Z", 1_000, 1_000),
                json!({"timestamp":"2026-08-06T10:01:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:01:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                token_count_line("2026-08-06T10:02:00Z", 1_100, 100),
                session_meta_line(
                    "2026-08-06T10:02:01Z",
                    "late-marker-parent",
                    None,
                    false,
                ),
                token_count_line("2026-08-06T10:03:00Z", 2_000, 900),
                json!({"timestamp":"2026-08-06T10:03:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:03:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
                token_count_line("2026-08-06T10:04:00Z", 2_100, 100),
            ]),
        )
        .unwrap();

        let result = run_usage_passes(&fixture, now, 3);

        assert_eq!(
            indexed_tokens_for_path(&fixture.database, &fixture.rollout),
            100
        );
        assert_eq!(result.daily[&now.date()].observed_tokens, 100);
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
                max_discovery_millis: 0,
                max_parse_millis: 0,
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
                max_discovery_millis: MAX_ROLLOUT_SCAN_MILLIS,
                max_parse_millis: MAX_ROLLOUT_SCAN_MILLIS,
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
            Some(&account.observed_at_by_day),
        );
        let connection = Connection::open(&fixture.database).unwrap();

        let report =
            render_debug_usage_report(&connection, Some(&account), &local, &periods, now.date())
                .unwrap();

        assert!(report.contains("retention_days=60"));
        assert!(report.contains(
            "model=gpt-5.6-sol pricing_mode=standard observed_tokens=100 input_tokens=70"
        ));
        assert!(report.contains("output_tokens=30"));
        assert!(report.contains("pricing_mode=standard"));
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
    fn partial_repricing_keeps_the_unprocessed_days_stored_basis() {
        let fixture = TempUsage::new();
        fs::write(&fixture.rollout, root_rollout(100)).unwrap();
        let now = OffsetDateTime::parse("2026-08-07T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let path: String = connection
            .query_row("SELECT path FROM codex_usage_files", [], |row| row.get(0))
            .unwrap();
        let original_day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let unprocessed_day = Date::from_calendar_date(2026, Month::August, 7).unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens, cache_write_input_tokens,
                   output_tokens, reasoning_output_tokens, observed_tokens,
                   cost_usd, pricing_basis, pricing_fingerprint, complete, observed_through
                 )
                 SELECT path, ?1, model, pricing_input_tokens, pricing_mode,
                        input_tokens, cached_input_tokens, cache_write_input_tokens,
                        output_tokens, reasoning_output_tokens, observed_tokens,
                        cost_usd, pricing_basis, pricing_fingerprint, complete, ?2
                 FROM codex_usage_file_model_days WHERE day = ?3",
                params![
                    unprocessed_day.to_string(),
                    "2026-08-07T10:01:00Z",
                    original_day.to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_days(
                   path, day, observed_tokens, priced_tokens, cost_usd, complete,
                   observed_through, priced_observed_through, pricing_fingerprint
                 )
                 SELECT path, ?1, observed_tokens, priced_tokens, cost_usd, complete,
                        ?2, ?2, pricing_fingerprint
                 FROM codex_usage_file_days WHERE day = ?3",
                params![
                    unprocessed_day.to_string(),
                    "2026-08-07T10:01:00Z",
                    original_day.to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE codex_usage_file_model_days SET pricing_basis = 'stored-pricing-v1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE codex_usage_index_meta SET value = 'stored-pricing-v1'
                 WHERE key = 'pricing_basis'",
                [],
            )
            .unwrap();
        let stored_cost = connection
            .query_row(
                "SELECT cost_usd FROM codex_usage_file_days WHERE day = ?1",
                [unprocessed_day.to_string()],
                |row| row.get::<_, f64>(0),
            )
            .unwrap();
        let changed = changed_pricing_manifest("test-price-basis-v2", 60.0);

        assert!(
            !reprice_index_batch_with_manifest(&connection, &changed, original_day, now.date(), 1,)
                .unwrap()
        );
        let local = read_indexed_usage(
            &connection,
            original_day,
            now.date(),
            UsageScanStatus::Indexing,
            true,
            None,
        )
        .unwrap();

        assert_eq!(
            local.daily[&unprocessed_day].pricing_basis.as_deref(),
            Some("stored-pricing-v1")
        );
        assert_eq!(
            local.daily[&unprocessed_day].api_equivalent_cost_usd,
            Some(stored_cost)
        );
        let history = load_daily_usage_history(&connection, now, now.date(), 2).unwrap();
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            ..
        } = &history[&unprocessed_day]
        else {
            panic!("the unprocessed day must remain available");
        };
        assert_eq!(*api_equivalent_cost_usd, Some(stored_cost));
        assert_eq!(
            api_equivalent_cost_basis.as_deref(),
            Some("stored-pricing-v1")
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_basis FROM codex_usage_file_model_days
                     WHERE path = ?1 AND day = ?2",
                    params![path, unprocessed_day.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "stored-pricing-v1"
        );
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
        let changed = changed_pricing_manifest("openai-standard-2026-08-26-v2", 60.0);
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
            PricingMode::Standard,
        )
        .unwrap();
        let terra_fingerprint = pricing_rule_fingerprint(
            manifest,
            "gpt-5.6-terra",
            day,
            usage,
            pricing_input_tokens,
            PricingMode::Standard,
        );
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                   complete, observed_through
                 ) VALUES(?1, ?2, 'gpt-5.6-terra', ?3, 'standard', ?4, ?5, ?6, ?7, ?8, ?9,
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

        let changed = changed_pricing_manifest("openai-standard-2026-08-26-v2", 60.0);
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
            pricing_rule_fingerprint(
                &changed,
                "gpt-5.6-terra",
                day,
                usage,
                pricing_input_tokens,
                PricingMode::Standard,
            )
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
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, pricing_fingerprint,
                   complete, observed_through
                 ) VALUES (?1, '2026-07-07', 'gpt-5.6-sol', 70, 'standard', 70, 20, 0, 30, 10,
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
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({"timestamp":"2026-01-01T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
                json!({"timestamp":"2026-01-01T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-01-01T10:01:00Z", 100, 100),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 200, 100),
            ]),
        )
        .unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let path: String = connection
            .query_row("SELECT path FROM codex_usage_files", [], |row| row.get(0))
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, complete, observed_through
                 ) VALUES (?1, '2026-01-01', 'gpt-5.6-sol', 7, 'standard', 7, 0, 0, 3, 0, 10,
                           0.000125, 'old-basis', 1, '2026-01-01T12:00:00Z')",
                [path.as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO codex_usage_file_model_days(
                   path, day, model, pricing_input_tokens, pricing_mode,
                   input_tokens, cached_input_tokens,
                   cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                   observed_tokens, cost_usd, pricing_basis, complete, observed_through
                 ) VALUES (?1, '2026-08-07', 'gpt-5.6-sol', 7, 'standard', 7, 0, 0, 3, 0, 10,
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
        for (record_ordinal, timestamp) in [
            (100_i64, "2026-01-01T12:00:00Z"),
            (101_i64, "2026-08-07T12:00:00Z"),
        ] {
            let timestamp_ns = OffsetDateTime::parse(timestamp, &Rfc3339)
                .unwrap()
                .unix_timestamp_nanos();
            connection
                .execute(
                    "INSERT INTO codex_usage_token_snapshots(
                       path, record_ordinal, timestamp_ns, input_tokens,
                       cached_input_tokens, cache_write_input_tokens, output_tokens,
                       reasoning_output_tokens, total_tokens
                     ) VALUES(?1, ?2, ?3, 7, 0, 0, 3, 0, 10)",
                    params![
                        path.as_str(),
                        record_ordinal,
                        i64::try_from(timestamp_ns).unwrap()
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO codex_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   parser_version, completion_state, schema_supported
                 ) VALUES('expired-rollout', 'old-file', 0, 0, 0, ?1, 'complete', 1)",
                [ROLLOUT_PARSER_VERSION],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE codex_usage_files SET parser_version = 0 WHERE path = ?1",
                [path.as_str()],
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
                    "SELECT COUNT(*) FROM codex_usage_token_snapshots
                     WHERE timestamp_ns < ?1 OR timestamp_ns >= ?2",
                    params![
                        i64::try_from(
                            Date::from_calendar_date(2026, Month::July, 8)
                                .unwrap()
                                .midnight()
                                .assume_utc()
                                .unix_timestamp_nanos()
                        )
                        .unwrap(),
                        i64::try_from(
                            Date::from_calendar_date(2026, Month::August, 7)
                                .unwrap()
                                .midnight()
                                .assume_utc()
                                .unix_timestamp_nanos()
                        )
                        .unwrap(),
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
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
    fn retention_keeps_sixty_day_tokens_and_thirty_day_private_cost_detail() {
        let fixture = TempUsage::new();
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                token_count_line("2026-08-06T10:01:00Z", 100, 100),
            ]),
        )
        .unwrap();
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        let path: String = connection
            .query_row("SELECT path FROM codex_usage_files", [], |row| row.get(0))
            .unwrap();
        for (day, ordinal) in [
            ("2026-07-08", 201_i64),
            ("2026-07-07", 202),
            ("2026-06-08", 203),
            ("2026-06-07", 204),
        ] {
            connection
                .execute(
                    "INSERT INTO codex_usage_file_days(
                       path, day, observed_tokens, priced_tokens, cost_usd, complete,
                       observed_through, priced_observed_through, pricing_fingerprint
                     ) VALUES(?1, ?2, 10, 10, 1.0, 1, ?3, ?3, 'pricing-v1')",
                    params![path, day, format!("{day}T12:00:00Z")],
                )
                .unwrap();
            let timestamp_ns = OffsetDateTime::parse(&format!("{day}T12:00:00Z"), &Rfc3339)
                .unwrap()
                .unix_timestamp_nanos();
            connection
                .execute(
                    "INSERT INTO codex_usage_token_snapshots(
                       path, record_ordinal, timestamp_ns, input_tokens,
                       cached_input_tokens, cache_write_input_tokens, output_tokens,
                       reasoning_output_tokens, total_tokens
                     ) VALUES(?1, ?2, ?3, 7, 0, 0, 3, 0, 10)",
                    params![path, ordinal, i64::try_from(timestamp_ns).unwrap()],
                )
                .unwrap();
        }
        for day in ["2026-07-08", "2026-07-07"] {
            connection
                .execute(
                    "INSERT INTO codex_usage_file_model_days(
                       path, day, model, pricing_input_tokens, pricing_mode,
                       input_tokens, cached_input_tokens, cache_write_input_tokens,
                       output_tokens, reasoning_output_tokens, observed_tokens,
                       cost_usd, pricing_basis, pricing_fingerprint, complete,
                       observed_through
                     ) VALUES(?1, ?2, 'gpt-5.6-sol', 7, 'standard', 7, 0, 0,
                              3, 0, 10, 1.0, 'retained-pricing-v1', 'pricing-v1', 1, ?3)",
                    params![path, day, format!("{day}T12:00:00Z")],
                )
                .unwrap();
        }
        let history_cutoff = Date::from_calendar_date(2026, Month::June, 8).unwrap();
        let detail_cutoff = Date::from_calendar_date(2026, Month::July, 8).unwrap();
        let cutoff_modified_ns = i64::try_from(
            history_cutoff
                .midnight()
                .assume_utc()
                .unix_timestamp_nanos(),
        )
        .unwrap();
        while !prune_expired_index(
            &connection,
            history_cutoff,
            detail_cutoff,
            now.date(),
            cutoff_modified_ns,
        )
        .unwrap()
        {}

        let stored = |day: &str| {
            connection
                .query_row(
                    "SELECT observed_tokens, priced_tokens, cost_usd
                     FROM codex_usage_file_days WHERE path = ?1 AND day = ?2",
                    params![path, day],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, f64>(2)?,
                        ))
                    },
                )
                .optional()
                .unwrap()
        };
        assert_eq!(stored("2026-06-07"), None);
        assert_eq!(stored("2026-06-08"), Some((10, 0, 0.0)));
        assert_eq!(stored("2026-07-07"), Some((10, 0, 0.0)));
        assert_eq!(stored("2026-07-08"), Some((10, 10, 1.0)));
        let retained_bases = connection
            .prepare(
                "SELECT day, pricing_basis FROM codex_usage_file_model_days
                 WHERE day IN ('2026-07-07', '2026-07-08') ORDER BY day",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            retained_bases,
            vec![("2026-07-08".to_owned(), "retained-pricing-v1".to_owned())]
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
        assert_eq!(manifest.basis, "openai-standard-2026-08-26-v2");
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
        pricing_model_mut(&mut duplicate, "gpt-5.6-sol")["aliases"] = json!(["gpt-5.5"]);
        assert!(parse_pricing_manifest(&duplicate.to_string()).is_err());

        let mut overlap: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        let model = pricing_model_mut(&mut overlap, "gpt-5.5");
        let period = model["periods"][0].clone();
        model["periods"].as_array_mut().unwrap().push(period);
        assert!(parse_pricing_manifest(&overlap.to_string()).is_err());

        let mut bad_date: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut bad_date, "gpt-5.2")["periods"][0]["effectiveFrom"] =
            json!("2026-02-30");
        assert!(parse_pricing_manifest(&bad_date.to_string()).is_err());

        let mut negative: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut negative, "gpt-5.2")["periods"][0]["inputUsdPerMillion"] =
            json!(-1.0);
        assert!(parse_pricing_manifest(&negative.to_string()).is_err());

        let mut multiplier: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut multiplier, "gpt-5.2")["periods"][0]["longContext"]["inputMultiplier"] =
            json!(0.0);
        assert!(parse_pricing_manifest(&multiplier.to_string()).is_err());

        let mut invalid_fast_date: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut invalid_fast_date, "gpt-5.6-sol")["periods"][0]["fastLongContext"]
            ["effectiveFrom"] = json!("2026-06-25");
        assert!(parse_pricing_manifest(&invalid_fast_date.to_string()).is_err());

        let mut invalid_fast_price: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut invalid_fast_price, "gpt-5.6-sol")["periods"][0]["fastLongContext"]
            ["outputUsdPerMillion"] = json!(-1.0);
        assert!(parse_pricing_manifest(&invalid_fast_price.to_string()).is_err());
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
        assert!(is_supported_cli_version("0.148.0-alpha.21"));
        assert!(is_supported_cli_version("0.149.1"));
        assert!(is_supported_cli_version("0.150.0-alpha.8"));
        assert!(is_supported_cli_version("0.151.0-alpha.7.2"));
        assert!(!is_supported_cli_version("0.129.9"));
        assert!(!is_supported_cli_version("0.152.0"));
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
        let assert_price = |model: &str, day: Date, expected: f64| {
            let actual = price_usage(model, day, usage).expect("known effective price");
            assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        };
        assert_price("gpt-5.6-sol", after_release, 0.62);
        assert_price(
            "gpt-5.6-sol",
            Date::from_calendar_date(2026, Month::August, 20).unwrap(),
            0.62,
        );
        assert_price(
            "gpt-5.6-sol",
            Date::from_calendar_date(2026, Month::August, 21).unwrap(),
            0.456,
        );
        for (model, old_price, new_price) in [
            ("gpt-5.6-terra", 0.31, 0.248),
            ("gpt-5.6-luna", 0.124, 0.0248),
        ] {
            assert_price(
                model,
                Date::from_calendar_date(2026, Month::July, 29).unwrap(),
                old_price,
            );
            assert_price(
                model,
                Date::from_calendar_date(2026, Month::July, 30).unwrap(),
                new_price,
            );
        }
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
        let current_day = Date::from_calendar_date(2026, Month::July, 30).unwrap();
        let current_cost = price_usage("gpt-5.6-terra", current_day, usage).unwrap();
        assert!((current_cost - 2.69).abs() < 1e-12);
        assert!(price_usage("gpt-5.5", day, usage).is_none());
    }

    #[test]
    fn gpt_5_6_standard_prices_match_the_published_catalog() {
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let cases = [
            ("gpt-5.6-sol", 5.0, 0.5, 6.25, 30.0),
            ("gpt-5.6-terra", 2.0, 0.2, 2.5, 12.0),
            ("gpt-5.6-luna", 0.2, 0.02, 0.25, 1.2),
        ];

        for (model, input, cached, cache_write, output) in cases {
            let short_context = |usage| price_usage(model, day, usage).unwrap() * 10.0;
            assert!((short_context(token_usage(100_000, 0, 0, 0)) - input).abs() < 1e-12);
            assert!((short_context(token_usage(100_000, 100_000, 0, 0)) - cached).abs() < 1e-12);
            assert!(
                (short_context(token_usage(100_000, 0, 100_000, 0)) - cache_write).abs() < 1e-12
            );
            assert!((short_context(token_usage(0, 0, 0, 100_000)) - output).abs() < 1e-12);
        }
    }

    #[test]
    fn codex_model_names_accept_provider_prefixes_and_dated_snapshots() {
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let usage = token_usage(100_000, 90_000, 0, 10_000);
        let sol = price_usage("gpt-5.6-sol", day, usage);

        assert_eq!(price_usage("gpt-5.6", day, usage), sol);
        assert_eq!(price_usage("openai/gpt-5.6-sol", day, usage), sol);
        assert_eq!(price_usage("gpt-5.6-sol-2026-08-09", day, usage), sol);
        assert!(price_usage("gpt-5.6-unknown", day, usage).is_none());
    }

    #[test]
    fn long_context_pricing_uses_the_last_request_input_not_the_cumulative_delta() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
            json!({"timestamp":"2026-08-09T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300000,"cached_input_tokens":270000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":310000},"model_context_window":1050000,"total_token_usage":{"input_tokens":200000,"cached_input_tokens":180000,"output_tokens":20000,"reasoning_output_tokens":0,"total_tokens":220000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let detail = &local.daily[&day];

        assert_eq!(detail.observed_tokens, 220_000);
        assert!((detail.api_equivalent_cost_usd.unwrap() - 1.035).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        let pricing_inputs = connection
            .prepare(
                "SELECT pricing_input_tokens FROM codex_usage_file_model_days
                 ORDER BY pricing_input_tokens",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, u64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(pricing_inputs, vec![100_000, 300_000]);
    }

    #[test]
    fn malformed_last_request_usage_cannot_select_a_pricing_tier() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
            json!({"timestamp":"2026-08-09T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300000,"cached_input_tokens":300001,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":310000},"model_context_window":1050000,"total_token_usage":{"input_tokens":200000,"cached_input_tokens":180000,"output_tokens":20000,"reasoning_output_tokens":0,"total_tokens":220000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(local.scan_status, UsageScanStatus::Unavailable);
        assert_eq!(local.daily[&day].observed_tokens, 110_000);
        let connection = Connection::open(&fixture.database).unwrap();
        let malformed_tier_rows = connection
            .query_row(
                "SELECT COUNT(*) FROM codex_usage_file_model_days
                 WHERE pricing_input_tokens = 300000",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(malformed_tier_rows, 0);
    }

    #[test]
    fn proved_fast_turn_uses_the_published_fast_price() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-fast","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    concat!(
                        "private=ignored turn.id=turn-fast websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\",\"input\":\"private\"}\n",
                        "private=ignored turn.id=turn-fast websocket event: {\"type\":\"response.created\",\"response\":{\"private\":\"ignored\"}}\n",
                        "private=ignored turn.id=turn-fast websocket event: {\"type\":\"response.output_item.done\",\"private\":\"ignored\"}\n",
                        "private=ignored turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-sol\",\"private\":\"ignored\"}}\n",
                        "private=ignored turn.id=turn-fast websocket event: {\"type\":\"response.done\",\"private\":\"ignored\"}\n",
                    ),
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let cost = local.daily[&day].api_equivalent_cost_usd.unwrap();

        assert!((cost - 0.79).abs() < 1e-12);
    }

    #[test]
    fn requested_fast_model_with_a_multiplier_precedes_the_rollout_model() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","model":"gpt-5.5","service_tier":"fast"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.9875).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT model || ':' || pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "gpt-5.5:fast"
        );
    }

    #[test]
    fn requested_fast_model_without_a_multiplier_keeps_the_rollout_model() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","model":"request-alias","service_tier":"fast"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT model || ':' || pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "gpt-5.6-sol:fast"
        );
    }

    #[test]
    fn mixed_fast_and_downgraded_completions_use_standard_price() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    concat!(
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"priority\"}}\n",
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"default\"}}\n",
                    ),
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.395).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "standard"
        );
    }

    #[test]
    fn conflicting_completed_fast_models_use_the_standard_rollout_model() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    concat!(
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"priority\"}}\n",
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-terra\",\"service_tier\":\"priority\"}}\n",
                    ),
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.395).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT model || ':' || pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "gpt-5.6-sol:standard"
        );
    }

    #[test]
    fn completed_fast_model_replaces_the_rollout_model_when_it_has_a_fast_price() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-fast","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    concat!(
                        "turn.id=turn-fast websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\"}\n",
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-terra\"}}",
                    ),
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let cost = local.daily[&day].api_equivalent_cost_usd.unwrap();

        assert!((cost - 0.316).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT model FROM codex_usage_file_model_days", [], |row| {
                    row.get::<_, String>(0)
                },)
                .unwrap(),
            "gpt-5.6-terra"
        );
    }

    #[test]
    fn fast_pricing_uses_published_context_rates_at_the_272k_boundary() {
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let short_usage = token_usage(100_000, 90_000, 0, 10_000);

        let gpt_5_5_standard = price_usage_tier(
            "gpt-5.5",
            day,
            short_usage,
            short_usage.input,
            PricingMode::Standard,
        )
        .unwrap();
        let gpt_5_5_fast = price_usage_tier(
            "gpt-5.5",
            day,
            short_usage,
            short_usage.input,
            PricingMode::Fast,
        )
        .unwrap();
        assert!((gpt_5_5_fast - gpt_5_5_standard * 2.5).abs() < 1e-12);

        let usage = token_usage(400_000, 100_000, 100_000, 100_000);
        let cases = [
            ("gpt-5.6-sol", 9.35, 15.7),
            ("gpt-5.6-terra", 3.74, 628.0 / 100.0),
            ("gpt-5.6-luna", 0.374, 0.628),
        ];
        for (model, expected_short, expected_long) in cases {
            let short = price_usage_tier(model, day, usage, 272_000, PricingMode::Fast).unwrap();
            let long = price_usage_tier(model, day, usage, 272_001, PricingMode::Fast).unwrap();

            assert!((short - expected_short).abs() < 1e-12, "{model} short");
            assert!((long - expected_long).abs() < 1e-12, "{model} long");
            assert_ne!(
                pricing_rule_fingerprint(
                    pricing_manifest().unwrap(),
                    model,
                    day,
                    usage,
                    272_000,
                    PricingMode::Fast,
                ),
                pricing_rule_fingerprint(
                    pricing_manifest().unwrap(),
                    model,
                    day,
                    usage,
                    272_001,
                    PricingMode::Fast,
                ),
                "{model} pricing fingerprint"
            );
        }

        let gpt_5_5_long_usage = token_usage(400_000, 100_000, 0, 100_000);
        assert!(
            price_usage_tier(
                "gpt-5.5",
                day,
                gpt_5_5_long_usage,
                272_001,
                PricingMode::Standard,
            )
            .is_some()
        );
        assert!(
            price_usage_tier(
                "gpt-5.5",
                day,
                gpt_5_5_long_usage,
                272_001,
                PricingMode::Fast,
            )
            .is_none()
        );
    }

    #[test]
    fn gpt_5_2_uses_one_all_context_rate_for_standard_and_fast_pricing() {
        let day = Date::from_calendar_date(2026, Month::August, 26).unwrap();
        let usage = token_usage(400_000, 100_000, 0, 100_000);

        for pricing_input_tokens in [272_000, 272_001] {
            let standard = price_usage_tier(
                "gpt-5.2",
                day,
                usage,
                pricing_input_tokens,
                PricingMode::Standard,
            )
            .expect("GPT-5.2 Standard pricing applies to every supported context size");
            let fast = price_usage_tier(
                "gpt-5.2",
                day,
                usage,
                pricing_input_tokens,
                PricingMode::Fast,
            )
            .expect("GPT-5.2 Fast pricing applies to every supported context size");

            assert!((standard - 1.9425).abs() < 1e-12);
            assert!((fast - 3.885).abs() < 1e-12);
        }

        let cache_write_usage = token_usage(400_000, 100_000, 100_000, 100_000);
        assert!(
            price_usage_tier(
                "gpt-5.2",
                day,
                cache_write_usage,
                272_001,
                PricingMode::Standard,
            )
            .is_none()
        );
        assert!(
            price_usage_tier(
                "gpt-5.2",
                day,
                cache_write_usage,
                272_001,
                PricingMode::Fast,
            )
            .is_none()
        );

        assert!(price_usage_tier("gpt-5.5", day, usage, 272_001, PricingMode::Fast,).is_none());
    }

    #[test]
    fn fast_all_context_fallback_does_not_override_dated_context_rates() {
        let day = Date::from_calendar_date(2026, Month::August, 26).unwrap();
        let usage = token_usage(400_000, 100_000, 0, 100_000);
        let mut source: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut source, "gpt-5.2")["periods"][0]["fastLongContext"] = json!({
            "effectiveFrom": "2026-09-01",
            "effectiveUntil": null,
            "inputUsdPerMillion": 3.5,
            "cachedInputUsdPerMillion": 0.35,
            "cacheWriteUsdPerMillion": null,
            "outputUsdPerMillion": 28.0
        });
        let manifest = parse_pricing_manifest(&source.to_string()).unwrap();

        assert!(
            price_usage_tier_with_manifest(
                &manifest,
                "gpt-5.2",
                day,
                usage,
                272_001,
                PricingMode::Fast,
            )
            .is_none()
        );
    }

    #[test]
    fn gpt_5_6_fast_long_context_pricing_starts_on_august_5() {
        let before_release = Date::from_calendar_date(2026, Month::August, 4).unwrap();
        let release_day = Date::from_calendar_date(2026, Month::August, 5).unwrap();
        let usage = token_usage(400_000, 100_000, 100_000, 100_000);

        assert!(
            price_usage_tier(
                "gpt-5.6-sol",
                before_release,
                usage,
                272_001,
                PricingMode::Fast,
            )
            .is_none()
        );
        let release_day_cost = price_usage_tier(
            "gpt-5.6-sol",
            release_day,
            usage,
            272_001,
            PricingMode::Fast,
        )
        .unwrap();
        assert!((release_day_cost - 15.7).abs() < 1e-12);
    }

    #[test]
    fn fast_long_context_rates_participate_in_pricing_fingerprints() {
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let usage = token_usage(300_000, 0, 0, 100_000);
        let original = pricing_manifest().unwrap();
        let mut changed: serde_json::Value =
            serde_json::from_str(OPENAI_STANDARD_PRICING_JSON).unwrap();
        pricing_model_mut(&mut changed, "gpt-5.6-sol")["periods"][0]["fastLongContext"]["outputUsdPerMillion"] =
            json!(91.0);
        let changed = parse_pricing_manifest(&changed.to_string()).unwrap();

        assert_ne!(original.fingerprint, changed.fingerprint);
        assert_eq!(
            pricing_rule_fingerprint(
                original,
                "gpt-5.6-sol",
                day,
                usage,
                272_000,
                PricingMode::Fast,
            ),
            pricing_rule_fingerprint(
                &changed,
                "gpt-5.6-sol",
                day,
                usage,
                272_000,
                PricingMode::Fast,
            )
        );
        assert_ne!(
            pricing_rule_fingerprint(
                original,
                "gpt-5.6-sol",
                day,
                usage,
                272_001,
                PricingMode::Fast,
            ),
            pricing_rule_fingerprint(
                &changed,
                "gpt-5.6-sol",
                day,
                usage,
                272_001,
                PricingMode::Fast,
            )
        );
    }

    #[test]
    fn token_count_turn_id_precedes_the_active_task_and_supports_nested_ids() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-fast","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","turn_id":"turn-standard","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
            json!({"timestamp":"2026-08-09T10:01:01Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-standard","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"turnId":"turn-fast","last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":200000,"cached_input_tokens":180000,"output_tokens":20000,"reasoning_output_tokens":0,"total_tokens":220000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        let detail = &local.daily[&day];

        assert_eq!(detail.observed_tokens, 220_000);
        assert!((detail.api_equivalent_cost_usd.unwrap() - 1.185).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        let modes = connection
            .prepare("SELECT pricing_mode FROM codex_usage_file_model_days ORDER BY pricing_mode")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(modes, vec!["fast", "standard"]);
    }

    #[test]
    fn changed_fast_evidence_reindexes_an_unchanged_rollout() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let rollout = [
            json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
            json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-later-fast","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
            json!({"timestamp":"2026-08-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000},"model_context_window":1050000,"total_token_usage":{"input_tokens":100000,"cached_input_tokens":90000,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":110000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        fs::write(&fixture.rollout, rollout).unwrap();

        let standard = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert!((standard.daily[&day].api_equivalent_cost_usd.unwrap() - 0.395).abs() < 1e-12);

        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-later-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let fast = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert_eq!(fast.daily[&day].observed_tokens, 110_000);
        assert!((fast.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "fast"
        );
    }

    #[test]
    fn failed_fast_evidence_invalidation_does_not_reuse_stored_fast_turns() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let fast = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert!((fast.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);

        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace.execute("DELETE FROM logs", []).unwrap();
        drop(trace);
        let connection = Connection::open(&fixture.database).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_fast_evidence_invalidation
                 BEFORE DELETE ON codex_usage_fast_turns
                 BEGIN
                   SELECT RAISE(ABORT, 'blocked by test');
                 END;",
            )
            .unwrap();
        drop(connection);

        assert!(index_local_usage_at(&fixture.database, &fixture.root, now).is_none());
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM codex_usage_fast_turns", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "fast"
        );
    }

    #[test]
    fn fast_evidence_invalidates_only_the_referencing_rollout() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let unaffected = fixture.root.join("sessions/unaffected.jsonl");
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-later-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        fs::write(
            &unaffected,
            turn_rollout("turn-standard", "gpt-5.6-terra", 220_000),
        )
        .unwrap();
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-later-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let cutoff = Date::from_calendar_date(2026, Month::July, 11).unwrap();
        let today = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        let evidence = load_fast_turn_evidence(&fixture.root, cutoff, today).unwrap();
        let connection = Connection::open(&fixture.database).unwrap();
        reconcile_fast_turn_evidence(&connection, &evidence).unwrap();

        let retained = connection
            .prepare("SELECT path FROM codex_usage_files ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(retained, vec![unaffected.to_string_lossy()]);
    }

    #[test]
    fn fast_evidence_read_failure_preserves_committed_fast_turns() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);

        let initial = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();
        assert!((initial.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);

        fs::write(fixture.root.join("logs_2.sqlite"), b"not a SQLite database").unwrap();
        let retained = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(retained.daily[&day].observed_tokens, 110_000);
        assert!((retained.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT model || ':' || pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "gpt-5.6-sol:fast"
        );
    }

    #[test]
    fn multi_day_rollout_drops_only_day_thirty_one_fast_detail_on_trace_failure() {
        let fixture = TempUsage::new();
        let expired_at = OffsetDateTime::parse("2026-08-09T10:01:00Z", &Rfc3339).unwrap();
        let recent_at = OffsetDateTime::parse("2026-09-07T10:01:00Z", &Rfc3339).unwrap();
        let initial_at = OffsetDateTime::parse("2026-09-07T12:00:00Z", &Rfc3339).unwrap();
        let rollover_at = OffsetDateTime::parse("2026-09-08T12:00:00Z", &Rfc3339).unwrap();
        let one_turn = TokenUsage {
            input: 100_000,
            cached_input: 90_000,
            cache_write_input: 0,
            output: 10_000,
            reasoning_output: 0,
            total: 110_000,
        };
        let two_turns = TokenUsage {
            input: 200_000,
            cached_input: 180_000,
            cache_write_input: 0,
            output: 20_000,
            reasoning_output: 0,
            total: 220_000,
        };
        fs::write(
            &fixture.rollout,
            jsonl([
                json!({"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.145.0"}}),
                json!({"timestamp":"2026-08-09T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-09T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-expired","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
                token_count_usage_line("2026-08-09T10:01:00Z", one_turn, one_turn),
                json!({"timestamp":"2026-09-07T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-09-07T10:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-recent","model_context_window":1050000,"collaboration_mode_kind":"default"}}),
                token_count_usage_line("2026-09-07T10:01:00Z", two_turns, one_turn),
            ]),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        for (observed_at, turn_id) in [(expired_at, "turn-expired"), (recent_at, "turn-recent")] {
            trace
                .execute(
                    "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                    params![
                        observed_at.unix_timestamp(),
                        format!(
                            r#"turn.id={turn_id} websocket request: {{"type":"response.create","service_tier":"priority"}}"#
                        ),
                    ],
                )
                .unwrap();
        }
        drop(trace);
        let initial = index_local_usage_at(&fixture.database, &fixture.root, initial_at).unwrap();
        assert_eq!(initial.daily[&expired_at.date()].priced_tokens, 110_000);
        assert_eq!(initial.daily[&recent_at.date()].priced_tokens, 110_000);

        fs::write(fixture.root.join("logs_2.sqlite"), b"not a SQLite database").unwrap();
        let retained = index_local_usage_at(&fixture.database, &fixture.root, rollover_at).unwrap();

        assert_eq!(retained.daily[&expired_at.date()].observed_tokens, 110_000);
        assert_eq!(retained.daily[&expired_at.date()].priced_tokens, 0);
        assert_eq!(
            retained.daily[&expired_at.date()].api_equivalent_cost_usd,
            None
        );
        assert_eq!(retained.daily[&recent_at.date()].observed_tokens, 110_000);
        assert_eq!(retained.daily[&recent_at.date()].priced_tokens, 110_000);
        assert!(
            retained.daily[&recent_at.date()]
                .api_equivalent_cost_usd
                .is_some()
        );
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT turn_id FROM codex_usage_fast_turns", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "turn-recent"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT turn_id || ':' || day FROM codex_usage_file_turns",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "turn-recent:2026-09-07"
        );
        assert_eq!(
            connection
                .query_row("SELECT day FROM codex_usage_file_model_days", [], |row| row
                    .get::<_, String>(0),)
                .unwrap(),
            "2026-09-07"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM codex_usage_token_snapshots",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT SUM(observed_tokens) FROM codex_usage_file_days",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            220_000
        );
    }

    #[test]
    fn authoritative_empty_fast_evidence_removes_committed_fast_turns() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    r#"turn.id=turn-fast websocket request: {"type":"response.create","service_tier":"priority"}"#,
                ],
            )
            .unwrap();
        drop(trace);
        index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace.execute("DELETE FROM logs", []).unwrap();
        drop(trace);
        let standard = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert!((standard.daily[&day].api_equivalent_cost_usd.unwrap() - 0.395).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM codex_usage_fast_turns", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "standard"
        );
    }

    #[test]
    fn fast_evidence_read_failure_without_stored_turns_keeps_standard_usage() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-standard", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        fs::write(fixture.root.join("logs_2.sqlite"), b"not a SQLite database").unwrap();

        let local = index_local_usage_at(&fixture.database, &fixture.root, now)
            .expect("an empty stored classification must keep observed usage");

        assert_eq!(local.daily[&day].observed_tokens, 110_000);
        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.395).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_mode FROM codex_usage_file_model_days",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "standard"
        );
    }

    #[test]
    fn fast_evidence_unknown_model_keeps_the_priced_rollout_model() {
        let fixture = TempUsage::new();
        let now = OffsetDateTime::parse("2026-08-09T12:00:00Z", &Rfc3339).unwrap();
        let day = Date::from_calendar_date(2026, Month::August, 9).unwrap();
        fs::write(
            &fixture.rollout,
            turn_rollout("turn-fast", "gpt-5.6-sol", 110_000),
        )
        .unwrap();
        let trace = Connection::open(fixture.root.join("logs_2.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE logs(ts INTEGER NOT NULL, feedback_log_body TEXT NOT NULL);",
            )
            .unwrap();
        trace
            .execute(
                "INSERT INTO logs(ts, feedback_log_body) VALUES(?1, ?2)",
                params![
                    now.unix_timestamp(),
                    concat!(
                        "turn.id=turn-fast websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\"}\n",
                        "turn.id=turn-fast websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-future-private\"}}",
                    ),
                ],
            )
            .unwrap();
        drop(trace);

        let local = index_local_usage_at(&fixture.database, &fixture.root, now).unwrap();

        assert_eq!(local.daily[&day].observed_tokens, 110_000);
        assert!((local.daily[&day].api_equivalent_cost_usd.unwrap() - 0.79).abs() < 1e-12);
        let connection = Connection::open(&fixture.database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT model FROM codex_usage_file_model_days", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "gpt-5.6-sol"
        );
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
    fn rollout_scan_normalizes_the_provider_last_usage_total() {
        let fixture = jsonl([
            session_meta_line("2026-08-06T10:00:00Z", "root-total-mismatch", None, false),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({
                "timestamp": "2026-08-06T10:01:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 70,
                            "cached_input_tokens": 20,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 10,
                            "total_tokens": 99
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": 70,
                            "cached_input_tokens": 20,
                            "cache_write_input_tokens": 0,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 10,
                            "total_tokens": 100
                        }
                    },
                    "rate_limits": null
                }
            }),
        ]);
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
        assert!(days[&day].complete);
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
    fn rollout_scan_counts_only_child_deltas_after_an_exact_subagent_boundary() {
        let token_count = |timestamp: &str, total: u64| {
            let input = total * 7 / 10;
            let output = total - input;
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 70,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 10,
                            "total_tokens": 100
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": input * 2 / 7,
                            "output_tokens": output,
                            "reasoning_output_tokens": output / 3,
                            "total_tokens": total
                        }
                    },
                    "rate_limits": null
                }
            })
        };
        let fixture = [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","forked_from_id":"private-parent","thread_source":"subagent"}}),
            token_count("2026-08-06T09:01:00Z", 1_000),
            token_count("2026-08-06T09:02:00Z", 1_100),
            json!({"timestamp":"2026-08-06T09:03:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"{\"type\":\"inter_agent_communication_metadata\",\"payload\":{\"trigger_turn\":true}}"}]}}),
            token_count("2026-08-06T09:04:00Z", 1_200),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:00:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            token_count("2026-08-06T10:01:00Z", 1_300),
            token_count("2026-08-06T10:02:00Z", 1_400),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 200);
    }

    #[test]
    fn rollout_scan_counts_a_self_contained_first_counter_after_the_boundary() {
        let fixture = [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","thread_source":"subagent"}}),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:00:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            json!({"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
    }

    #[test]
    fn rollout_scan_restarts_a_strong_counter_below_the_copied_prefix() {
        let fixture = jsonl([
            session_meta_line(
                "2026-08-06T10:05:00Z",
                "reset-child",
                Some("reset-parent"),
                true,
            ),
            session_meta_line("2026-08-06T10:00:00Z", "reset-parent", None, false),
            token_count_line("2026-08-06T10:04:00Z", 1_000, 1_000),
            json!({"timestamp":"2026-08-06T10:05:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:05:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            token_count_line("2026-08-06T10:07:00Z", 100, 100),
        ]);
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
        assert!(days[&day].complete);
    }

    #[test]
    fn rollout_scan_keeps_a_copied_first_counter_as_the_boundary_baseline() {
        let fixture = [
            json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","thread_source":"subagent"}}),
            json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
            json!({"timestamp":"2026-08-06T10:00:02Z","type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
            json!({"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert!(days.is_empty());
    }

    #[test]
    fn rollout_scan_rejects_false_missing_and_prompt_text_subagent_boundaries() {
        let marker_payloads = [json!({"trigger_turn":false}), json!({})];
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();

        for marker_payload in marker_payloads {
            let fixture = [
                json!({"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","thread_source":"subagent"}}),
                json!({"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
                json!({"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}),
                json!({"timestamp":"2026-08-06T10:01:30Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"inter_agent_communication_metadata trigger_turn=true"}]}}),
                json!({"timestamp":"2026-08-06T10:02:00Z","type":"inter_agent_communication_metadata","payload":marker_payload}),
                json!({"timestamp":"2026-08-06T10:03:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}),
            ]
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n";
            let mut days = BTreeMap::new();

            assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
            assert!(days.is_empty());
        }
    }

    #[test]
    fn rollout_scan_rejects_a_root_level_trigger_turn_spoof() {
        let fixture = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"cli_version":"0.146.0-alpha.3.1","thread_source":"subagent"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000},"model_context_window":1050000,"total_token_usage":{"input_tokens":700,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":1000}},"rate_limits":null}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:02:00Z","type":"inter_agent_communication_metadata","trigger_turn":true,"payload":{"trigger_turn":false}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:03:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":70,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":10,"total_tokens":100},"model_context_window":1050000,"total_token_usage":{"input_tokens":770,"cached_input_tokens":220,"output_tokens":330,"reasoning_output_tokens":110,"total_tokens":1100}},"rate_limits":null}}"#,
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
    fn rollout_scan_uses_nonzero_ordinals_for_paginated_subagent_history() {
        let fixture = [
            json!({
                "ordinal": 500,
                "timestamp": "2026-08-06T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "history_base": {
                        "thread_id": "fixture-thread",
                        "end_ordinal_exclusive": 500,
                        "end_byte_offset": 0
                    },
                    "history_mode": "paginated",
                    "subagent_history_start_ordinal": 503,
                    "thread_source": "subagent"
                }
            }),
            json!({
                "ordinal": 501,
                "timestamp": "2026-08-06T09:00:00Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
            json!({
                "ordinal": 502,
                "timestamp": "2026-08-06T09:01:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 700,
                            "cached_input_tokens": 200,
                            "output_tokens": 300,
                            "reasoning_output_tokens": 100,
                            "total_tokens": 1000
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": 700,
                            "cached_input_tokens": 200,
                            "output_tokens": 300,
                            "reasoning_output_tokens": 100,
                            "total_tokens": 1000
                        }
                    },
                    "rate_limits": null
                }
            }),
            json!({
                "ordinal": 503,
                "timestamp": "2026-08-06T10:01:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 70,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 10,
                            "total_tokens": 100
                        },
                        "model_context_window": 1_050_000,
                        "total_token_usage": {
                            "input_tokens": 770,
                            "cached_input_tokens": 220,
                            "output_tokens": 330,
                            "reasoning_output_tokens": 110,
                            "total_tokens": 1100
                        }
                    },
                    "rate_limits": null
                }
            }),
        ]
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        let mut days = BTreeMap::new();

        assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
        assert_eq!(days[&day].observed_tokens, 100);
    }

    #[test]
    fn rollout_scan_rejects_malformed_or_mismatched_provider_ordinals() {
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        for invalid_ordinal in [json!("501"), json!(502), json!(null)] {
            let fixture = [
                json!({
                    "ordinal": 500,
                    "timestamp": "2026-08-06T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "cli_version": "0.148.0-alpha.21",
                        "history_base": {
                            "thread_id": "fixture-thread",
                            "end_ordinal_exclusive": 500,
                            "end_byte_offset": 7
                        },
                        "history_mode": "paginated"
                    }
                }),
                json!({
                    "ordinal": invalid_ordinal,
                    "timestamp": "2026-08-06T10:00:01Z",
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                }),
            ]
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n";
            let mut days = BTreeMap::new();

            assert!(!scan_rollout_reader(
                fixture.as_bytes(),
                day,
                day,
                &mut days
            ));
        }

        let missing_ordinal = jsonl([
            json!({
                "ordinal": 0,
                "timestamp": "2026-08-06T10:00:00Z",
                "type": "session_meta",
                "payload": {
                    "cli_version": "0.148.0-alpha.21",
                    "history_mode": "paginated"
                }
            }),
            json!({
                "timestamp": "2026-08-06T10:00:01Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5.6-sol" }
            }),
        ]);
        let mut days = BTreeMap::new();
        assert!(!scan_rollout_reader(
            missing_ordinal.as_bytes(),
            day,
            day,
            &mut days,
        ));
    }

    #[test]
    fn rollout_scan_rejects_unreviewed_provider_ordinal_origins() {
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        for payload in [
            json!({
                "cli_version": "0.148.0-alpha.21",
                "history_mode": "legacy"
            }),
            json!({
                "cli_version": "0.148.0-alpha.21",
                "history_mode": "paginated"
            }),
            json!({
                "cli_version": "0.148.0-alpha.21",
                "history_base": null,
                "history_mode": "paginated"
            }),
            json!({
                "cli_version": "0.148.0-alpha.21",
                "history_base": {
                    "thread_id": "fixture-thread",
                    "end_ordinal_exclusive": 499,
                    "end_byte_offset": 7
                },
                "history_mode": "paginated"
            }),
        ] {
            let fixture = jsonl([json!({
                "ordinal": 500,
                "timestamp": "2026-08-06T10:00:00Z",
                "type": "session_meta",
                "payload": payload
            })]);
            let mut days = BTreeMap::new();
            assert!(!scan_rollout_reader(
                fixture.as_bytes(),
                day,
                day,
                &mut days,
            ));
        }

        let non_session_origin = jsonl([json!({
            "ordinal": 500,
            "timestamp": "2026-08-06T10:00:00Z",
            "type": "turn_context",
            "payload": { "model": "gpt-5.6-sol" }
        })]);
        let mut days = BTreeMap::new();
        assert!(!scan_rollout_reader(
            non_session_origin.as_bytes(),
            day,
            day,
            &mut days,
        ));

        let legacy_non_session_origin = jsonl([json!({
            "timestamp": "2026-08-06T10:00:00Z",
            "type": "turn_context",
            "payload": { "model": "gpt-5.6-sol" }
        })]);
        let mut days = BTreeMap::new();
        assert!(!scan_rollout_reader(
            legacy_non_session_origin.as_bytes(),
            day,
            day,
            &mut days,
        ));

        let current_without_provider_ordinal = jsonl([json!({
            "timestamp": "2026-08-06T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "cli_version": "0.148.0-alpha.21",
                "history_mode": "paginated"
            }
        })]);
        let mut days = BTreeMap::new();
        assert!(!scan_rollout_reader(
            current_without_provider_ordinal.as_bytes(),
            day,
            day,
            &mut days,
        ));

        let codex_0_150_legacy_child = jsonl([json!({
            "timestamp": "2026-08-26T10:00:00Z",
            "type": "session_meta",
            "payload": {
                "cli_version": "0.150.0-alpha.8",
                "history_mode": "legacy",
                "thread_source": "subagent"
            }
        })]);
        let mut days = BTreeMap::new();
        assert!(!scan_rollout_reader(
            codex_0_150_legacy_child.as_bytes(),
            day,
            day,
            &mut days,
        ));
    }

    #[test]
    fn rollout_scan_keeps_reviewed_codex_0_148_legacy_records_without_ordinals() {
        let day = Date::from_calendar_date(2026, Month::August, 6).unwrap();
        for version in ["0.148.0-alpha.9", "0.148.0-alpha.15", "0.148.0-alpha.21"] {
            let fixture = root_rollout(100).replace(
                r#""cli_version":"0.145.0""#,
                &format!(r#""cli_version":"{version}","history_mode":"legacy""#),
            );
            let mut days = BTreeMap::new();

            assert!(scan_rollout_reader(fixture.as_bytes(), day, day, &mut days));
            assert_eq!(days[&day].observed_tokens, 100);
        }
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
    fn provider_reported_usage_remains_authoritative_after_local_scan_completes() {
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
                    modeled: false,
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            scan_scope_known: true,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            evidence_basis,
            coverage,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 1_000);
        assert!((api_equivalent_cost_usd.unwrap() - (1.0 / 0.6)).abs() < 1e-12);
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(60.0));
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(coverage, UsageCoverage::Complete);
    }

    #[test]
    fn thirty_day_sync_fixture_prefers_account_days_without_summing_local_tokens() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let day = now.date() - Duration::days(1);
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(day, 1_000)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                day,
                LocalUsageDay {
                    observed_tokens: 400,
                    priced_tokens: 400,
                    api_equivalent_cost_usd: Some(0.8),
                    modeled: false,
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: Some(pricing_manifest().unwrap().basis.clone()),
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            scan_scope_known: true,
            ..LocalUsageObservation::default()
        };
        let evidence = provider_usage_evidence(Some(&account), Some(&local), now, now, None);
        let daily = calculate_daily_usage_aggregates(&evidence, now, now.date(), 30);
        let UsageTotal::Current {
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = &daily[&day]
        else {
            panic!("the account day must be available");
        };
        assert_eq!(*evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(*observed_tokens, 1_000);
        assert_eq!(*api_equivalent_cost_usd, Some(2.0));
    }

    #[test]
    fn unpriced_local_scan_does_not_replace_provider_reported_tokens() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let day = now.date();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(day, 467_600)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                day,
                LocalUsageDay {
                    observed_tokens: 1_100_000_000,
                    priced_tokens: 0,
                    api_equivalent_cost_usd: None,
                    modeled: false,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: None,
                    pricing_basis: None,
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            scan_scope_known: true,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            evidence_basis,
            coverage,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 467_600);
        assert_eq!(api_equivalent_cost_usd, None);
        assert_eq!(api_equivalent_cost_quality, None);
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(coverage, UsageCoverage::Complete);
    }

    #[test]
    fn excluded_local_usage_does_not_replace_provider_reported_tokens() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let day = now.date();
        let account = AccountUsageObservation {
            daily_tokens: BTreeMap::from([(day, 467_600)]),
        };
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                day,
                LocalUsageDay {
                    observed_tokens: 1_100_000_000,
                    priced_tokens: 1_100_000_000,
                    api_equivalent_cost_usd: Some(675.78),
                    modeled: false,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            has_excluded_usage: true,
            scan_scope_known: true,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            evidence_basis,
            coverage,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 467_600);
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(coverage, UsageCoverage::Complete);
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
                    modeled: false,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
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
                    modeled: false,
                    complete: true,
                    observed_through: Some(now - Duration::minutes(2)),
                    priced_observed_through: Some(now - Duration::minutes(2)),
                    pricing_basis: None,
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
                    modeled: false,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
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
    fn matching_complete_local_scan_reconciles_provider_reported_cost() {
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
                    modeled: false,
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
                },
            )]),
            scan_status: UsageScanStatus::Complete,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(Some(&account), Some(&local), now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            evidence_basis,
            coverage,
            ..
        } = projected.today
        else {
            panic!("expected current usage");
        };
        assert_eq!(observed_tokens, 1_000);
        assert_eq!(api_equivalent_cost_usd, Some(1.25));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Reconciled)
        );
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(coverage, UsageCoverage::Complete);
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
    fn local_rollouts_fill_a_missing_account_day_without_becoming_provider_evidence() {
        let now = OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap();
        let local = LocalUsageObservation {
            daily: BTreeMap::from([(
                now.date(),
                LocalUsageDay {
                    observed_tokens: 600,
                    priced_tokens: 600,
                    api_equivalent_cost_usd: Some(1.0),
                    modeled: false,
                    complete: true,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: Some(now - Duration::minutes(1)),
                    pricing_basis: None,
                },
            )]),
            scan_status: UsageScanStatus::Indexing,
            ..LocalUsageObservation::default()
        };

        let projected = project_usage_periods(None, Some(&local), now);
        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            ..
        } = projected.today
        else {
            panic!("local fallback must remain visible while indexing");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 600);
        let evidence = provider_usage_evidence(None, Some(&local), now, now, None);
        let daily = calculate_daily_usage_aggregates(&evidence, now, now.date(), 30);
        assert!(matches!(
            daily[&now.date()],
            UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::LocallyDerived,
                observed_tokens: 600,
                ..
            }
        ));
    }

    #[test]
    fn account_usage_remains_authoritative_while_local_scan_is_incomplete() {
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
                    modeled: false,
                    complete: true,
                    observed_through: Some(now + Duration::minutes(1)),
                    priced_observed_through: Some(now + Duration::minutes(1)),
                    pricing_basis: None,
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
            modeled: false,
            complete: true,
            observed_through: Some(now - Duration::minutes(1)),
            priced_observed_through: Some(now - Duration::minutes(1)),
            pricing_basis: None,
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
