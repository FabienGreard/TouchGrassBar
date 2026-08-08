use std::{collections::BTreeSet, sync::OnceLock};

use serde::Deserialize;
use time::{Date, Month};

const ANTHROPIC_STANDARD_PRICING_JSON: &str =
    include_str!("../../../pricing/anthropic-standard.json");
const PRICING_RULES_FINGERPRINT: &str = "service-tier-default-standard;priority-standard-rate;fast-batch-unavailable;missing-paid-metadata-unavailable;web-fetch-no-extra-charge;missing-code-execution-counter-zero;positive-code-execution-unavailable;unknown-paid-tool-unavailable";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricingManifest {
    schema_version: u32,
    basis: String,
    batch_factor: f64,
    us_inference_factor: f64,
    web_search_usd_per_thousand: f64,
    models: Vec<RawPricedModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricedModel {
    name: String,
    aliases: Vec<String>,
    supports_us_inference: bool,
    standard_periods: Vec<RawPricePeriod>,
    fast_periods: Vec<RawPricePeriod>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPricePeriod {
    effective_from: String,
    effective_until: Option<String>,
    input_usd_per_million: f64,
    cache_write_5m_usd_per_million: f64,
    cache_write_1h_usd_per_million: f64,
    cache_read_usd_per_million: f64,
    output_usd_per_million: f64,
}

#[derive(Clone)]
pub(super) struct PricingCatalog {
    basis: String,
    semantic_fingerprint: String,
    batch_factor: f64,
    us_inference_factor: f64,
    web_search_usd_per_thousand: f64,
    models: Vec<PricedModel>,
}

#[derive(Clone)]
struct PricedModel {
    names: Vec<String>,
    supports_us_inference: bool,
    standard_periods: Vec<PriceCatalogEntry>,
    fast_periods: Vec<PriceCatalogEntry>,
}

#[derive(Clone, Copy)]
struct PriceCatalogEntry {
    effective_from: Date,
    effective_until: Option<Date>,
    input_usd_per_million: f64,
    cache_write_5m_usd_per_million: f64,
    cache_write_1h_usd_per_million: f64,
    cache_read_usd_per_million: f64,
    output_usd_per_million: f64,
}

impl PriceCatalogEntry {
    fn applies_to(self, day: Date) -> bool {
        day >= self.effective_from && self.effective_until.is_none_or(|until| day < until)
    }

    fn contains(self, other: Self) -> bool {
        other.effective_from >= self.effective_from
            && match (self.effective_until, other.effective_until) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(outer), Some(inner)) => inner <= outer,
            }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct BillableUsage<'a> {
    pub(super) input_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_creation_5m_input_tokens: Option<u64>,
    pub(super) cache_creation_1h_input_tokens: Option<u64>,
    pub(super) cache_read_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) service_tier: Option<&'a str>,
    pub(super) inference_geo: Option<&'a str>,
    pub(super) speed: Option<&'a str>,
    pub(super) web_search_requests: Option<u64>,
    pub(super) web_fetch_requests: u64,
    pub(super) code_execution_requests: Option<u64>,
    pub(super) has_unknown_paid_server_tool: bool,
}

impl BillableUsage<'_> {
    fn observed_tokens(self) -> Option<u64> {
        self.input_tokens
            .checked_add(self.cache_creation_input_tokens)?
            .checked_add(self.cache_read_input_tokens)?
            .checked_add(self.output_tokens)
    }

    fn cache_creation_split(self) -> Option<(u64, u64)> {
        match (
            self.cache_creation_5m_input_tokens,
            self.cache_creation_1h_input_tokens,
        ) {
            (Some(five_minutes), Some(one_hour))
                if five_minutes.checked_add(one_hour)? == self.cache_creation_input_tokens =>
            {
                Some((five_minutes, one_hour))
            }
            (None, None) if self.cache_creation_input_tokens == 0 => Some((0, 0)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PriceDecision {
    pub(super) cost_usd: Option<f64>,
    pub(super) modeled: bool,
    pub(super) priced_tokens: u64,
    pub(super) rule_fingerprint: String,
}

impl PriceDecision {
    fn unavailable(reason: &str) -> Self {
        Self {
            cost_usd: None,
            modeled: false,
            priced_tokens: 0,
            rule_fingerprint: stable_fingerprint(&format!("unavailable:{reason}")),
        }
    }
}

impl PricingCatalog {
    pub(super) fn basis(&self) -> &str {
        &self.basis
    }

    pub(super) fn semantic_fingerprint(&self) -> &str {
        &self.semantic_fingerprint
    }

    pub(super) fn canonical_model_name(&self, model_name: &str) -> Option<&str> {
        self.models
            .iter()
            .find(|model| model.names.iter().any(|name| name == model_name))
            .and_then(|model| model.names.first())
            .map(String::as_str)
    }

    pub(super) fn price_message(
        &self,
        model_name: &str,
        day: Date,
        usage: BillableUsage<'_>,
    ) -> PriceDecision {
        self.price_message_inner(model_name, day, usage)
            .unwrap_or_else(PriceDecision::unavailable)
    }

    fn price_message_inner(
        &self,
        model_name: &str,
        day: Date,
        usage: BillableUsage<'_>,
    ) -> Result<PriceDecision, &'static str> {
        let observed_tokens = usage.observed_tokens().ok_or("token-overflow")?;
        let (cache_write_5m, cache_write_1h) = usage
            .cache_creation_split()
            .ok_or("missing-cache-write-split")?;
        let web_search_requests = usage
            .web_search_requests
            .ok_or("missing-web-search-usage")?;
        let code_execution_requests = usage.code_execution_requests.unwrap_or(0);
        if code_execution_requests > 0 {
            return Err("unpriced-code-execution");
        }
        if usage.has_unknown_paid_server_tool {
            return Err("unknown-paid-server-tool");
        }
        let model = self
            .models
            .iter()
            .find(|model| model.names.iter().any(|name| name == model_name))
            .ok_or("unknown-model")?;
        let standard_entry = model
            .standard_periods
            .iter()
            .copied()
            .find(|entry| entry.applies_to(day))
            .ok_or("missing-effective-price")?;
        let tier_factor = match usage.service_tier {
            None | Some("standard" | "priority") => 1.0,
            Some("batch") => self.batch_factor,
            Some(_) => return Err("unknown-service-tier"),
        };
        // Claude Code subscription logs can omit the inference location. Use
        // the standard global rate, but retain that this rate was modeled.
        let (geo_factor, modeled) = match (model.supports_us_inference, usage.inference_geo) {
            (true, Some("global")) => (1.0, false),
            (true, Some("us")) => (self.us_inference_factor, false),
            (true, None | Some("not_available")) => (1.0, true),
            (false, None | Some("global" | "not_available")) => (1.0, false),
            (_, Some(_)) => return Err("unknown-inference-geo"),
        };
        let applicable_fast_entry = model
            .fast_periods
            .iter()
            .copied()
            .find(|entry| entry.applies_to(day));
        let entry = match usage.speed {
            Some("standard") => standard_entry,
            Some("fast") => {
                if usage.service_tier == Some("batch") {
                    return Err("fast-batch-combination");
                }
                applicable_fast_entry.ok_or("missing-fast-price")?
            }
            None if applicable_fast_entry.is_none() => standard_entry,
            None => return Err("missing-speed"),
            Some(_) => return Err("unknown-speed"),
        };
        let token_factor = tier_factor * geo_factor;
        let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
        let token_cost = token_factor
            * (per_million(usage.input_tokens, entry.input_usd_per_million)
                + per_million(cache_write_5m, entry.cache_write_5m_usd_per_million)
                + per_million(cache_write_1h, entry.cache_write_1h_usd_per_million)
                + per_million(
                    usage.cache_read_input_tokens,
                    entry.cache_read_usd_per_million,
                )
                + per_million(usage.output_tokens, entry.output_usd_per_million));
        let web_search_cost =
            (web_search_requests as f64 / 1_000.0) * self.web_search_usd_per_thousand;
        let cost_usd = token_cost + web_search_cost;
        if !cost_usd.is_finite() {
            return Err("non-finite-cost");
        }
        let applicable_rate = |tokens: u64, rate: f64| {
            (tokens > 0).then(|| format!("{:016x}", (rate * token_factor).to_bits()))
        };
        let rule_fingerprint = stable_fingerprint(&format!(
            "priced:geo={}:input={}:cache-write-5m={}:cache-write-1h={}:cache-read={}:output={}:web-search={}",
            if modeled {
                "assumed-global"
            } else {
                "reported"
            },
            applicable_rate(usage.input_tokens, entry.input_usd_per_million)
                .unwrap_or_else(|| "unused".to_owned()),
            applicable_rate(cache_write_5m, entry.cache_write_5m_usd_per_million)
                .unwrap_or_else(|| "unused".to_owned()),
            applicable_rate(cache_write_1h, entry.cache_write_1h_usd_per_million)
                .unwrap_or_else(|| "unused".to_owned()),
            applicable_rate(
                usage.cache_read_input_tokens,
                entry.cache_read_usd_per_million,
            )
            .unwrap_or_else(|| "unused".to_owned()),
            applicable_rate(usage.output_tokens, entry.output_usd_per_million)
                .unwrap_or_else(|| "unused".to_owned()),
            if web_search_requests > 0 {
                format!("{:016x}", self.web_search_usd_per_thousand.to_bits())
            } else {
                "unused".to_owned()
            },
        ));
        let _ = usage.web_fetch_requests;
        Ok(PriceDecision {
            cost_usd: Some(cost_usd),
            modeled,
            priced_tokens: observed_tokens,
            rule_fingerprint,
        })
    }
}

pub(super) fn catalog() -> Option<&'static PricingCatalog> {
    static CATALOG: OnceLock<Result<PricingCatalog, ()>> = OnceLock::new();
    CATALOG
        .get_or_init(|| parse_pricing_manifest(ANTHROPIC_STANDARD_PRICING_JSON))
        .as_ref()
        .ok()
}

#[cfg(test)]
pub(super) fn bundled_manifest_for_test() -> &'static str {
    ANTHROPIC_STANDARD_PRICING_JSON
}

#[cfg(test)]
pub(super) fn catalog_from_manifest_for_test(value: &str) -> Result<PricingCatalog, ()> {
    parse_pricing_manifest(value)
}

fn parse_pricing_manifest(source: &str) -> Result<PricingCatalog, ()> {
    let raw: RawPricingManifest = serde_json::from_str(source).map_err(|_| ())?;
    if raw.schema_version != 1
        || !valid_basis(&raw.basis)
        || !valid_factor(raw.batch_factor, 0.0, 1.0)
        || !valid_factor(raw.us_inference_factor, 1.0, f64::INFINITY)
        || !valid_rate(raw.web_search_usd_per_thousand)
    {
        return Err(());
    }
    let mut known_names = BTreeSet::new();
    let mut models = Vec::with_capacity(raw.models.len());
    for model in raw.models {
        let mut names = Vec::with_capacity(model.aliases.len() + 1);
        names.push(model.name);
        names.extend(model.aliases);
        if names
            .iter()
            .any(|name| !valid_model_name(name) || !known_names.insert(name.clone()))
        {
            return Err(());
        }
        let standard_periods = parse_price_periods(model.standard_periods, false)?;
        let fast_periods = parse_price_periods(model.fast_periods, true)?;
        if standard_periods.is_empty()
            || fast_periods.iter().any(|fast| {
                !standard_periods
                    .iter()
                    .copied()
                    .any(|standard| standard.contains(*fast))
            })
        {
            return Err(());
        }
        models.push(PricedModel {
            names,
            supports_us_inference: model.supports_us_inference,
            standard_periods,
            fast_periods,
        });
    }
    if models.is_empty() {
        return Err(());
    }
    let semantic_fingerprint = pricing_manifest_fingerprint(
        &raw.basis,
        raw.batch_factor,
        raw.us_inference_factor,
        raw.web_search_usd_per_thousand,
        &models,
    );
    Ok(PricingCatalog {
        basis: raw.basis,
        semantic_fingerprint,
        batch_factor: raw.batch_factor,
        us_inference_factor: raw.us_inference_factor,
        web_search_usd_per_thousand: raw.web_search_usd_per_thousand,
        models,
    })
}

fn parse_price_periods(
    periods: Vec<RawPricePeriod>,
    allow_empty: bool,
) -> Result<Vec<PriceCatalogEntry>, ()> {
    if periods.is_empty() {
        return allow_empty.then_some(Vec::new()).ok_or(());
    }
    let mut periods = periods
        .into_iter()
        .map(|period| {
            let effective_from = parse_ranking_day(&period.effective_from)?;
            let effective_until = period
                .effective_until
                .as_deref()
                .map(parse_ranking_day)
                .transpose()?;
            let rates = [
                period.input_usd_per_million,
                period.cache_write_5m_usd_per_million,
                period.cache_write_1h_usd_per_million,
                period.cache_read_usd_per_million,
                period.output_usd_per_million,
            ];
            if effective_until.is_some_and(|until| until <= effective_from)
                || !rates.into_iter().all(valid_rate)
                || !approximately_equal(
                    period.cache_write_5m_usd_per_million,
                    period.input_usd_per_million * 1.25,
                )
                || !approximately_equal(
                    period.cache_write_1h_usd_per_million,
                    period.input_usd_per_million * 2.0,
                )
                || !approximately_equal(
                    period.cache_read_usd_per_million,
                    period.input_usd_per_million * 0.1,
                )
            {
                return Err(());
            }
            Ok(PriceCatalogEntry {
                effective_from,
                effective_until,
                input_usd_per_million: period.input_usd_per_million,
                cache_write_5m_usd_per_million: period.cache_write_5m_usd_per_million,
                cache_write_1h_usd_per_million: period.cache_write_1h_usd_per_million,
                cache_read_usd_per_million: period.cache_read_usd_per_million,
                output_usd_per_million: period.output_usd_per_million,
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
    Ok(periods)
}

fn pricing_manifest_fingerprint(
    basis: &str,
    batch_factor: f64,
    us_inference_factor: f64,
    web_search_usd_per_thousand: f64,
    models: &[PricedModel],
) -> String {
    let mut model_parts = models
        .iter()
        .map(|model| {
            let mut names = model.names.clone();
            names.sort();
            format!(
                "{}|us={}|standard={}|fast={}",
                names.join(","),
                model.supports_us_inference,
                period_fingerprint_parts(&model.standard_periods),
                period_fingerprint_parts(&model.fast_periods),
            )
        })
        .collect::<Vec<_>>();
    model_parts.sort();
    stable_fingerprint(&format!(
        "basis={basis}|rules={PRICING_RULES_FINGERPRINT}|batch={:016x}|us={:016x}|web={:016x}|{}",
        batch_factor.to_bits(),
        us_inference_factor.to_bits(),
        web_search_usd_per_thousand.to_bits(),
        model_parts.join("||"),
    ))
}

fn period_fingerprint_parts(periods: &[PriceCatalogEntry]) -> String {
    periods
        .iter()
        .map(|period| {
            format!(
                "{}|{}|{:016x}|{:016x}|{:016x}|{:016x}|{:016x}",
                period.effective_from,
                period
                    .effective_until
                    .map_or_else(|| "open".to_owned(), |until| until.to_string()),
                period.input_usd_per_million.to_bits(),
                period.cache_write_5m_usd_per_million.to_bits(),
                period.cache_write_1h_usd_per_million.to_bits(),
                period.cache_read_usd_per_million.to_bits(),
                period.output_usd_per_million.to_bits(),
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn stable_fingerprint(canonical: &str) -> String {
    let hash = canonical
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("fnv1a64:{hash:016x}")
}

fn valid_factor(value: f64, lower_exclusive: f64, upper_inclusive: f64) -> bool {
    value.is_finite() && value > lower_exclusive && value <= upper_inclusive
}

fn valid_rate(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn approximately_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= 1e-12 * left.abs().max(right.abs()).max(1.0)
}

fn valid_basis(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_model_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn parse_ranking_day(value: &str) -> Result<Date, ()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(());
    }
    let year = value.get(0..4).ok_or(())?.parse::<i32>().map_err(|_| ())?;
    let month = value.get(5..7).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    let day = value.get(8..10).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    Date::from_calendar_date(year, Month::try_from(month).map_err(|_| ())?, day).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> Date {
        parse_ranking_day(value).expect("valid test date")
    }

    fn usage() -> BillableUsage<'static> {
        BillableUsage {
            input_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_creation_5m_input_tokens: Some(0),
            cache_creation_1h_input_tokens: Some(0),
            cache_read_input_tokens: 0,
            output_tokens: 1_000_000,
            service_tier: Some("standard"),
            inference_geo: Some("global"),
            speed: Some("standard"),
            web_search_requests: Some(0),
            web_fetch_requests: 0,
            code_execution_requests: Some(0),
            has_unknown_paid_server_tool: false,
        }
    }

    fn assert_cost(decision: PriceDecision, expected: f64) {
        let actual = decision.cost_usd.expect("priced usage");
        assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
        assert!(decision.rule_fingerprint.starts_with("fnv1a64:"));
    }

    #[test]
    fn bundled_manifest_is_valid_and_has_a_semantic_fingerprint() {
        let manifest = parse_pricing_manifest(ANTHROPIC_STANDARD_PRICING_JSON)
            .expect("valid bundled manifest");
        let changed_basis = parse_pricing_manifest(&ANTHROPIC_STANDARD_PRICING_JSON.replacen(
            "anthropic-standard-2026-08-07-v1",
            "anthropic-standard-2026-08-07-v2",
            1,
        ))
        .expect("valid changed basis");

        assert_eq!(manifest.basis(), "anthropic-standard-2026-08-07-v1");
        assert!(manifest.semantic_fingerprint().starts_with("fnv1a64:"));
        assert_ne!(
            manifest.semantic_fingerprint(),
            changed_basis.semantic_fingerprint()
        );
        assert_eq!(manifest.models.len(), 15);
    }

    #[test]
    fn manifest_rejects_unknown_fields_bad_cache_rates_and_overlaps() {
        let unknown_field = ANTHROPIC_STANDARD_PRICING_JSON.replacen(
            "\"schemaVersion\": 1,",
            "\"schemaVersion\": 1, \"unexpected\": true,",
            1,
        );
        let bad_cache_rate = ANTHROPIC_STANDARD_PRICING_JSON.replacen(
            "\"cacheWrite5mUsdPerMillion\": 12.5",
            "\"cacheWrite5mUsdPerMillion\": 12.6",
            1,
        );
        let overlap = ANTHROPIC_STANDARD_PRICING_JSON.replacen(
            "\"effectiveUntil\": \"2026-09-01\"",
            "\"effectiveUntil\": \"2026-09-02\"",
            1,
        );

        assert!(parse_pricing_manifest(&unknown_field).is_err());
        assert!(parse_pricing_manifest(&bad_cache_rate).is_err());
        assert!(parse_pricing_manifest(&overlap).is_err());
    }

    #[test]
    fn prices_five_minute_and_one_hour_cache_writes_separately() {
        let manifest = catalog().expect("bundled catalog");
        let decision = manifest.price_message(
            "claude-sonnet-4-6",
            date("2026-08-01"),
            BillableUsage {
                cache_creation_input_tokens: 2_000_000,
                cache_creation_5m_input_tokens: Some(1_000_000),
                cache_creation_1h_input_tokens: Some(1_000_000),
                cache_read_input_tokens: 1_000_000,
                ..usage()
            },
        );

        assert_cost(decision.clone(), 28.05);
        assert_eq!(decision.priced_tokens, 5_000_000);
    }

    #[test]
    fn applies_effective_dates_and_exclusive_end_dates() {
        let manifest = catalog().expect("bundled catalog");

        assert_cost(
            manifest.price_message("claude-sonnet-5", date("2026-08-31"), usage()),
            12.0,
        );
        assert_cost(
            manifest.price_message("claude-sonnet-5", date("2026-09-01"), usage()),
            18.0,
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-1-20250805",
                date("2026-08-04"),
                BillableUsage {
                    inference_geo: None,
                    ..usage()
                },
            ),
            90.0,
        );
        assert_eq!(
            manifest
                .price_message(
                    "claude-opus-4-1-20250805",
                    date("2026-08-05"),
                    BillableUsage {
                        inference_geo: None,
                        ..usage()
                    },
                )
                .cost_usd,
            None
        );
    }

    #[test]
    fn omits_cost_for_unknown_models_modifiers_and_missing_paid_metadata() {
        let manifest = catalog().expect("bundled catalog");
        let day = date("2026-08-01");
        let cases = [
            ("unknown-model", BillableUsage { ..usage() }),
            (
                "claude-sonnet-4-6",
                BillableUsage {
                    service_tier: Some("private"),
                    ..usage()
                },
            ),
            (
                "claude-sonnet-4-6",
                BillableUsage {
                    inference_geo: Some("eu"),
                    ..usage()
                },
            ),
            (
                "claude-sonnet-4-6",
                BillableUsage {
                    speed: Some("turbo"),
                    ..usage()
                },
            ),
            (
                "claude-sonnet-4-6",
                BillableUsage {
                    web_search_requests: None,
                    ..usage()
                },
            ),
            (
                "claude-sonnet-4-6",
                BillableUsage {
                    cache_creation_input_tokens: 1,
                    cache_creation_5m_input_tokens: None,
                    cache_creation_1h_input_tokens: None,
                    ..usage()
                },
            ),
        ];

        for (model, usage) in cases {
            let decision = manifest.price_message(model, day, usage);
            assert_eq!(decision.cost_usd, None);
            assert_eq!(decision.priced_tokens, 0);
            assert!(decision.rule_fingerprint.starts_with("fnv1a64:"));
        }
    }

    #[test]
    fn applies_fast_batch_and_us_factors() {
        let manifest = catalog().expect("bundled catalog");
        let day = date("2026-08-01");

        assert_cost(
            manifest.price_message("claude-opus-4-8", day, usage()),
            30.0,
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-8",
                day,
                BillableUsage {
                    speed: Some("fast"),
                    ..usage()
                },
            ),
            60.0,
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-8",
                day,
                BillableUsage {
                    service_tier: Some("batch"),
                    ..usage()
                },
            ),
            15.0,
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-8",
                day,
                BillableUsage {
                    inference_geo: Some("us"),
                    ..usage()
                },
            ),
            33.0,
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-8",
                day,
                BillableUsage {
                    service_tier: Some("batch"),
                    inference_geo: Some("us"),
                    ..usage()
                },
            ),
            16.5,
        );
        assert_eq!(
            manifest
                .price_message(
                    "claude-opus-4-8",
                    day,
                    BillableUsage {
                        service_tier: Some("batch"),
                        speed: Some("fast"),
                        ..usage()
                    },
                )
                .cost_usd,
            None
        );
    }

    #[test]
    fn uses_modeled_standard_price_when_supported_model_geo_is_unavailable() {
        let manifest = catalog().expect("bundled catalog");
        for inference_geo in [None, Some("not_available")] {
            let decision = manifest.price_message(
                "claude-opus-4-8",
                date("2026-08-01"),
                BillableUsage {
                    inference_geo,
                    ..usage()
                },
            );

            assert!(decision.modeled);
            assert_cost(decision, 30.0);
        }
    }

    #[test]
    fn applies_fast_mode_end_dates() {
        let manifest = catalog().expect("bundled catalog");
        let fast = BillableUsage {
            speed: Some("fast"),
            ..usage()
        };

        assert_cost(
            manifest.price_message("claude-opus-4-6", date("2026-06-28"), fast),
            180.0,
        );
        assert_eq!(
            manifest
                .price_message("claude-opus-4-6", date("2026-06-29"), fast)
                .cost_usd,
            None
        );
        assert_cost(
            manifest.price_message(
                "claude-opus-4-6",
                date("2026-06-29"),
                BillableUsage {
                    speed: Some("standard"),
                    ..usage()
                },
            ),
            30.0,
        );
    }

    #[test]
    fn adds_web_search_and_does_not_add_web_fetch() {
        let manifest = catalog().expect("bundled catalog");
        let day = date("2026-08-01");
        let with_search = manifest.price_message(
            "claude-sonnet-4-6",
            day,
            BillableUsage {
                web_search_requests: Some(2),
                web_fetch_requests: 99,
                ..usage()
            },
        );

        assert_cost(with_search, 18.02);
        assert_cost(
            manifest.price_message(
                "claude-sonnet-4-6",
                day,
                BillableUsage {
                    code_execution_requests: None,
                    ..usage()
                },
            ),
            18.0,
        );
        assert_eq!(
            manifest
                .price_message(
                    "claude-sonnet-4-6",
                    day,
                    BillableUsage {
                        code_execution_requests: Some(1),
                        ..usage()
                    },
                )
                .cost_usd,
            None
        );
        assert_eq!(
            manifest
                .price_message(
                    "claude-sonnet-4-6",
                    day,
                    BillableUsage {
                        has_unknown_paid_server_tool: true,
                        ..usage()
                    },
                )
                .cost_usd,
            None
        );
    }
}
