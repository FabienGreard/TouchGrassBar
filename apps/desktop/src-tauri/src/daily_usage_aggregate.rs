use std::collections::{BTreeMap, BTreeSet};

use time::{Date, Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::sanitized::{
    ApiEquivalentCostQuality, UsageCoverage, UsageEvidenceBasis, UsagePeriods, UsageScanStatus,
    UsageTotal,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DailyCostEvidence {
    pub(crate) observed_tokens: u64,
    pub(crate) priced_tokens: u64,
    pub(crate) api_equivalent_cost_usd: Option<f64>,
    pub(crate) complete: bool,
    pub(crate) observed_through: Option<OffsetDateTime>,
    pub(crate) priced_observed_through: Option<OffsetDateTime>,
}

impl Default for DailyCostEvidence {
    fn default() -> Self {
        Self {
            observed_tokens: 0,
            priced_tokens: 0,
            api_equivalent_cost_usd: Some(0.0),
            complete: true,
            observed_through: None,
            priced_observed_through: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderUsageEvidence {
    pub(crate) provider_reported_tokens: Option<BTreeMap<Date, u64>>,
    pub(crate) provider_observed_at: Option<OffsetDateTime>,
    pub(crate) local_cost_evidence: BTreeMap<Date, DailyCostEvidence>,
    pub(crate) local_evidence_available: bool,
    pub(crate) local_observed_at: Option<OffsetDateTime>,
    pub(crate) pricing_basis: Option<String>,
    pub(crate) scan_status: UsageScanStatus,
    pub(crate) today_scan_status: UsageScanStatus,
    pub(crate) seven_day_scan_status: UsageScanStatus,
    pub(crate) thirty_day_scan_status: UsageScanStatus,
}

#[derive(Clone, Copy)]
struct CostProjection {
    usd: f64,
    quality: ApiEquivalentCostQuality,
    coverage_percent: Option<f64>,
}

pub(crate) fn period_days(today: Date, length: i64, offset: i64) -> impl Iterator<Item = Date> {
    (0..length).map(move |index| today - Duration::days(offset + index))
}

pub(crate) fn checked_sum<'a>(mut values: impl Iterator<Item = &'a u64>) -> Option<u64> {
    values.try_fold(0_u64, |total, value| total.checked_add(*value))
}

fn trend_percent(current: u64, previous: u64) -> Option<f64> {
    if previous == 0 {
        return None;
    }
    let trend = ((current as f64 - previous as f64) / previous as f64) * 100.0;
    trend.is_finite().then_some(trend)
}

fn observed_period_sum(
    mut days: impl Iterator<Item = Date>,
    mut tokens_for_day: impl FnMut(Date) -> Option<u64>,
) -> Option<u64> {
    let mut observed_day = false;
    let total = days.try_fold(0_u64, |total, day| {
        let observed_tokens = tokens_for_day(day);
        let tokens = observed_tokens.unwrap_or(0);
        observed_day |= observed_tokens.is_some();
        total.checked_add(tokens)
    })?;
    observed_day.then_some(total)
}

fn selected_source_trend(
    evidence: &ProviderUsageEvidence,
    evidence_basis: UsageEvidenceBasis,
    today: Date,
    length: i64,
    current_tokens: u64,
) -> Option<f64> {
    let previous_tokens = match evidence_basis {
        UsageEvidenceBasis::ProviderReported => evidence
            .provider_reported_tokens
            .as_ref()
            .and_then(|daily| {
                observed_period_sum(period_days(today, length, length), |day| {
                    daily.get(&day).copied()
                })
            }),
        UsageEvidenceBasis::LocallyDerived => {
            observed_period_sum(period_days(today, length, length), |day| {
                evidence
                    .local_cost_evidence
                    .get(&day)
                    .map(|detail| detail.observed_tokens)
            })
        }
        UsageEvidenceBasis::Mixed => None,
    }?;
    trend_percent(current_tokens, previous_tokens)
}

fn provider_reported_cost(
    provider_tokens: &BTreeMap<Date, u64>,
    days: impl Iterator<Item = Date>,
    local: &BTreeMap<Date, DailyCostEvidence>,
    provider_observed_at: OffsetDateTime,
) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut covered_tokens = 0_u64;
    let mut total_tokens = 0_u64;
    let mut fallback_cost_usd = 0.0;
    let mut fallback_priced_tokens = 0_u64;
    let mut missing_provider_tokens = 0_u64;
    let mut modeled = false;
    for (day, provider_tokens) in
        days.filter_map(|day| provider_tokens.get(&day).map(|tokens| (day, *tokens)))
    {
        total_tokens = total_tokens.checked_add(provider_tokens)?;
        if provider_tokens == 0 {
            modeled |= local
                .get(&day)
                .is_some_and(|detail| detail.observed_tokens > 0);
            continue;
        }
        let Some(detail) = local.get(&day) else {
            // Keep the account tokens and defer this day's cost until a valid
            // local rate is available from another day in the same period.
            modeled = true;
            missing_provider_tokens = missing_provider_tokens.checked_add(provider_tokens)?;
            continue;
        };
        if detail.priced_tokens == 0 {
            // Unknown model prices leave this detail uncovered. A priced day
            // in the same period can still supply a defensible modeled rate.
            modeled = true;
            missing_provider_tokens = missing_provider_tokens.checked_add(provider_tokens)?;
            continue;
        }
        if detail.observed_tokens == 0 || detail.priced_tokens > detail.observed_tokens {
            return None;
        }
        let observed_through = detail.observed_through?;
        let priced_observed_through = detail.priced_observed_through?;
        let local_cost = detail.api_equivalent_cost_usd?;
        covered_tokens = covered_tokens.checked_add(detail.priced_tokens.min(provider_tokens))?;
        fallback_cost_usd += local_cost;
        fallback_priced_tokens = fallback_priced_tokens.checked_add(detail.priced_tokens)?;
        if detail.complete
            && detail.observed_tokens == provider_tokens
            && detail.priced_tokens == provider_tokens
            && observed_through <= provider_observed_at
            && priced_observed_through <= provider_observed_at
        {
            usd += local_cost;
        } else {
            modeled = true;
            usd += local_cost * (provider_tokens as f64 / detail.priced_tokens as f64);
        }
    }
    if missing_provider_tokens > 0 {
        // A missing local day is different from invalid local pricing. Model
        // only when another account day supplied a valid, finite rate.
        if fallback_priced_tokens == 0 {
            return None;
        }
        usd += fallback_cost_usd * (missing_provider_tokens as f64 / fallback_priced_tokens as f64);
    }
    if !usd.is_finite() {
        return None;
    }
    let coverage_percent = modeled.then(|| {
        if total_tokens == 0 {
            100.0
        } else {
            ((covered_tokens as f64 / total_tokens as f64) * 100.0).clamp(0.0, 100.0)
        }
    });
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

fn locally_derived_cost(
    local: &BTreeMap<Date, DailyCostEvidence>,
    days: impl Iterator<Item = Date>,
) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut priced_tokens = 0_u64;
    for detail in days.filter_map(|day| local.get(&day)) {
        if detail.priced_tokens == 0 {
            continue;
        }
        if detail.observed_tokens == 0 || detail.priced_tokens > detail.observed_tokens {
            return None;
        }
        usd += detail.api_equivalent_cost_usd?;
        priced_tokens = priced_tokens.checked_add(detail.priced_tokens)?;
    }
    (priced_tokens > 0 && usd.is_finite()).then_some(CostProjection {
        usd,
        quality: ApiEquivalentCostQuality::LocalOnly,
        coverage_percent: None,
    })
}

fn project_period(
    evidence: &ProviderUsageEvidence,
    now: OffsetDateTime,
    length: i64,
) -> UsageTotal {
    let today = now.to_offset(UtcOffset::UTC).date();
    let provider_period = evidence
        .provider_reported_tokens
        .as_ref()
        .and_then(|provider_tokens| {
            let observed_days = period_days(today, length, 0)
                .filter(|day| provider_tokens.contains_key(day))
                .count();
            (observed_days > 0).then_some((provider_tokens, observed_days))
        });
    let (tokens, evidence_basis, coverage, observed_at, cost) =
        if let Some((provider_tokens, observed_days)) = provider_period {
            let expected = usize::try_from(length).unwrap_or(usize::MAX);
            let Some(tokens) = checked_sum(
                period_days(today, length, 0).filter_map(|day| provider_tokens.get(&day)),
            ) else {
                return UsageTotal::Unavailable;
            };
            let Some(observed_at) = evidence.provider_observed_at else {
                return UsageTotal::Unavailable;
            };
            (
                tokens,
                UsageEvidenceBasis::ProviderReported,
                if observed_days == expected {
                    UsageCoverage::Complete
                } else {
                    UsageCoverage::Partial
                },
                observed_at,
                evidence
                    .local_evidence_available
                    .then(|| {
                        provider_reported_cost(
                            provider_tokens,
                            period_days(today, length, 0),
                            &evidence.local_cost_evidence,
                            observed_at,
                        )
                    })
                    .flatten(),
            )
        } else if evidence.local_evidence_available {
            let observed_days = period_days(today, length, 0)
                .filter(|day| evidence.local_cost_evidence.contains_key(day))
                .count();
            let Some(tokens) = checked_sum(period_days(today, length, 0).filter_map(|day| {
                evidence
                    .local_cost_evidence
                    .get(&day)
                    .map(|detail| &detail.observed_tokens)
            })) else {
                return UsageTotal::Unavailable;
            };
            if observed_days == 0 {
                return UsageTotal::Unavailable;
            }
            (
                tokens,
                UsageEvidenceBasis::LocallyDerived,
                UsageCoverage::Partial,
                evidence.local_observed_at.unwrap_or(now),
                locally_derived_cost(&evidence.local_cost_evidence, period_days(today, length, 0)),
            )
        } else {
            return UsageTotal::Unavailable;
        };

    let trend = selected_source_trend(evidence, evidence_basis, today, length, tokens);
    let observed_at = observed_at
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    UsageTotal::Current {
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens: tokens,
        api_equivalent_cost_usd: cost.map(|cost| cost.usd),
        trend_percent: trend,
        api_equivalent_cost_basis: cost.and_then(|_| evidence.pricing_basis.clone()),
        api_equivalent_cost_quality: cost.map(|cost| cost.quality),
        api_equivalent_cost_coverage_percent: cost.and_then(|cost| cost.coverage_percent),
    }
}

pub(crate) fn calculate_usage_periods(
    evidence: &ProviderUsageEvidence,
    now: OffsetDateTime,
) -> UsagePeriods {
    UsagePeriods {
        scan_status: evidence.scan_status,
        today_scan_status: evidence.today_scan_status,
        seven_day_scan_status: evidence.seven_day_scan_status,
        thirty_day_scan_status: evidence.thirty_day_scan_status,
        today: project_period(evidence, now, 1),
        seven_days: project_period(evidence, now, 7),
        thirty_days: project_period(evidence, now, 30),
    }
}

fn scan_status(statuses: impl Iterator<Item = UsageScanStatus>) -> UsageScanStatus {
    let statuses = statuses.collect::<Vec<_>>();
    if statuses.is_empty() {
        return UsageScanStatus::Unavailable;
    }
    if statuses.contains(&UsageScanStatus::Indexing) {
        UsageScanStatus::Indexing
    } else if statuses
        .iter()
        .all(|status| *status == UsageScanStatus::Complete)
    {
        UsageScanStatus::Complete
    } else {
        UsageScanStatus::Unavailable
    }
}

struct AvailableTotal<'a> {
    stale: bool,
    evidence_basis: UsageEvidenceBasis,
    coverage: UsageCoverage,
    observed_at: &'a str,
    observed_tokens: u64,
    api_equivalent_cost_usd: Option<f64>,
    api_equivalent_cost_basis: Option<&'a str>,
    api_equivalent_cost_quality: Option<ApiEquivalentCostQuality>,
    api_equivalent_cost_coverage_percent: Option<f64>,
}

fn available_total(total: &UsageTotal) -> Option<AvailableTotal<'_>> {
    let (
        stale,
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        api_equivalent_cost_usd,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    ) = match total {
        UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_at,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            false,
            *evidence_basis,
            *coverage,
            observed_at,
            *observed_tokens,
            *api_equivalent_cost_usd,
            api_equivalent_cost_basis.as_deref(),
            *api_equivalent_cost_quality,
            *api_equivalent_cost_coverage_percent,
        ),
        UsageTotal::Stale {
            evidence_basis,
            coverage,
            observed_at,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            true,
            *evidence_basis,
            *coverage,
            observed_at,
            *observed_tokens,
            *api_equivalent_cost_usd,
            api_equivalent_cost_basis.as_deref(),
            *api_equivalent_cost_quality,
            *api_equivalent_cost_coverage_percent,
        ),
        UsageTotal::Unavailable => return None,
    };
    Some(AvailableTotal {
        stale,
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        api_equivalent_cost_usd,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    })
}

fn combined_cost(totals: &[AvailableTotal<'_>]) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut total_tokens = 0_u64;
    let mut covered_tokens = 0.0;
    let mut quality = ApiEquivalentCostQuality::Reconciled;
    for total in totals {
        if total.observed_tokens == 0 && total.api_equivalent_cost_usd.is_none() {
            continue;
        }
        let cost = total.api_equivalent_cost_usd?;
        let item_quality = total.api_equivalent_cost_quality?;
        usd += cost;
        total_tokens = total_tokens.checked_add(total.observed_tokens)?;
        quality = weakest_cost_quality(quality, item_quality);
        let coverage = match item_quality {
            ApiEquivalentCostQuality::Reconciled => 100.0,
            ApiEquivalentCostQuality::Modeled => total.api_equivalent_cost_coverage_percent?,
            ApiEquivalentCostQuality::LocalOnly => 0.0,
        };
        covered_tokens += coverage * total.observed_tokens as f64;
    }
    if !usd.is_finite() {
        return None;
    }
    let coverage_percent = match quality {
        ApiEquivalentCostQuality::Modeled if total_tokens > 0 => {
            Some((covered_tokens / total_tokens as f64).clamp(0.0, 100.0))
        }
        ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly => None,
        ApiEquivalentCostQuality::Modeled => Some(100.0),
    };
    Some(CostProjection {
        usd,
        quality,
        coverage_percent,
    })
}

fn weakest_cost_quality(
    left: ApiEquivalentCostQuality,
    right: ApiEquivalentCostQuality,
) -> ApiEquivalentCostQuality {
    match (left, right) {
        (ApiEquivalentCostQuality::LocalOnly, _) | (_, ApiEquivalentCostQuality::LocalOnly) => {
            ApiEquivalentCostQuality::LocalOnly
        }
        (ApiEquivalentCostQuality::Modeled, _) | (_, ApiEquivalentCostQuality::Modeled) => {
            ApiEquivalentCostQuality::Modeled
        }
        (ApiEquivalentCostQuality::Reconciled, ApiEquivalentCostQuality::Reconciled) => {
            ApiEquivalentCostQuality::Reconciled
        }
    }
}

fn combined_basis(totals: &[AvailableTotal<'_>]) -> Option<String> {
    let mut bases = BTreeSet::new();
    for total in totals {
        if total.observed_tokens == 0 && total.api_equivalent_cost_usd.is_none() {
            continue;
        }
        bases.insert(total.api_equivalent_cost_basis?);
    }
    (!bases.is_empty()).then(|| bases.into_iter().collect::<Vec<_>>().join(" + "))
}

fn combine_total(totals: &[&UsageTotal]) -> UsageTotal {
    if totals.len() == 1 {
        return totals[0].clone();
    }
    let available = totals
        .iter()
        .filter_map(|total| available_total(total))
        .collect::<Vec<_>>();
    if available.is_empty() {
        return UsageTotal::Unavailable;
    }
    let Some(observed_tokens) = available
        .iter()
        .try_fold(0_u64, |sum, total| sum.checked_add(total.observed_tokens))
    else {
        return UsageTotal::Unavailable;
    };
    let evidence_basis = available
        .iter()
        .map(|total| total.evidence_basis)
        .reduce(|left, right| {
            if left == right {
                left
            } else {
                UsageEvidenceBasis::Mixed
            }
        })
        .unwrap_or(UsageEvidenceBasis::Mixed);
    let coverage = if available.len() == totals.len()
        && available
            .iter()
            .all(|total| total.coverage == UsageCoverage::Complete)
    {
        UsageCoverage::Complete
    } else {
        UsageCoverage::Partial
    };
    let observed_at = available
        .iter()
        .map(|total| total.observed_at)
        .min()
        .unwrap_or("1970-01-01T00:00:00Z")
        .to_owned();
    let cost = combined_cost(&available);
    let basis = cost.and_then(|_| combined_basis(&available));
    let stale = available.iter().any(|total| total.stale);
    let fields = (
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        cost.map(|cost| cost.usd),
        basis,
        cost.map(|cost| cost.quality),
        cost.and_then(|cost| cost.coverage_percent),
    );
    if stale {
        UsageTotal::Stale {
            evidence_basis: fields.0,
            coverage: fields.1,
            observed_at: fields.2,
            observed_tokens: fields.3,
            api_equivalent_cost_usd: fields.4,
            trend_percent: None,
            api_equivalent_cost_basis: fields.5,
            api_equivalent_cost_quality: fields.6,
            api_equivalent_cost_coverage_percent: fields.7,
        }
    } else {
        UsageTotal::Current {
            evidence_basis: fields.0,
            coverage: fields.1,
            observed_at: fields.2,
            observed_tokens: fields.3,
            api_equivalent_cost_usd: fields.4,
            trend_percent: None,
            api_equivalent_cost_basis: fields.5,
            api_equivalent_cost_quality: fields.6,
            api_equivalent_cost_coverage_percent: fields.7,
        }
    }
}

pub(crate) fn combine_usage_periods(periods: &[&UsagePeriods]) -> UsagePeriods {
    UsagePeriods {
        scan_status: scan_status(periods.iter().map(|periods| periods.scan_status)),
        today_scan_status: scan_status(periods.iter().map(|periods| periods.today_scan_status)),
        seven_day_scan_status: scan_status(
            periods.iter().map(|periods| periods.seven_day_scan_status),
        ),
        thirty_day_scan_status: scan_status(
            periods.iter().map(|periods| periods.thirty_day_scan_status),
        ),
        today: combine_total(
            &periods
                .iter()
                .map(|periods| &periods.today)
                .collect::<Vec<_>>(),
        ),
        seven_days: combine_total(
            &periods
                .iter()
                .map(|periods| &periods.seven_days)
                .collect::<Vec<_>>(),
        ),
        thirty_days: combine_total(
            &periods
                .iter()
                .map(|periods| &periods.thirty_days)
                .collect::<Vec<_>>(),
        ),
    }
}

#[derive(Clone)]
struct PublishedCost {
    usd: f64,
    basis: Option<String>,
    quality: Option<ApiEquivalentCostQuality>,
    coverage_percent: Option<f64>,
    observed_tokens: u64,
}

fn published_cost(total: &UsageTotal) -> Option<PublishedCost> {
    match total {
        UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd: Some(usd),
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        }
        | UsageTotal::Stale {
            observed_tokens,
            api_equivalent_cost_usd: Some(usd),
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => Some(PublishedCost {
            usd: *usd,
            basis: api_equivalent_cost_basis.clone(),
            quality: *api_equivalent_cost_quality,
            coverage_percent: *api_equivalent_cost_coverage_percent,
            observed_tokens: *observed_tokens,
        }),
        UsageTotal::Unavailable
        | UsageTotal::Current {
            api_equivalent_cost_usd: None,
            ..
        }
        | UsageTotal::Stale {
            api_equivalent_cost_usd: None,
            ..
        } => None,
    }
}

fn preserve_cost_if_missing(current: &mut UsageTotal, previous: &UsageTotal) {
    let Some(previous) = published_cost(previous) else {
        return;
    };
    match current {
        UsageTotal::Current {
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        }
        | UsageTotal::Stale {
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } if api_equivalent_cost_usd.is_none() => {
            let same_total = *observed_tokens == previous.observed_tokens;
            let scaled = if same_total {
                previous.usd
            } else if previous.observed_tokens > 0 && *observed_tokens > 0 {
                previous.usd * (*observed_tokens as f64 / previous.observed_tokens as f64)
            } else {
                return;
            };
            if !scaled.is_finite() {
                return;
            }
            let (quality, coverage_percent) = if same_total {
                (previous.quality, previous.coverage_percent)
            } else {
                match evidence_basis {
                    UsageEvidenceBasis::ProviderReported | UsageEvidenceBasis::Mixed => {
                        let previous_coverage = match previous.quality {
                            Some(ApiEquivalentCostQuality::Modeled) => {
                                previous.coverage_percent.unwrap_or(0.0)
                            }
                            Some(
                                ApiEquivalentCostQuality::Reconciled
                                | ApiEquivalentCostQuality::LocalOnly,
                            ) => 100.0,
                            None => return,
                        };
                        (
                            Some(ApiEquivalentCostQuality::Modeled),
                            Some(
                                (previous.observed_tokens as f64 * previous_coverage
                                    / *observed_tokens as f64)
                                    .clamp(0.0, 100.0),
                            ),
                        )
                    }
                    UsageEvidenceBasis::LocallyDerived => {
                        (Some(ApiEquivalentCostQuality::LocalOnly), None)
                    }
                }
            };
            *api_equivalent_cost_usd = Some(scaled);
            *api_equivalent_cost_basis = previous.basis;
            *api_equivalent_cost_quality = quality;
            *api_equivalent_cost_coverage_percent = coverage_percent;
        }
        UsageTotal::Unavailable | UsageTotal::Current { .. } | UsageTotal::Stale { .. } => {}
    }
}

pub(crate) fn preserve_best_known_costs(
    mut current: UsagePeriods,
    previous: &UsagePeriods,
) -> UsagePeriods {
    if current.today_scan_status == UsageScanStatus::Indexing {
        preserve_cost_if_missing(&mut current.today, &previous.today);
    }
    if current.seven_day_scan_status == UsageScanStatus::Indexing {
        preserve_cost_if_missing(&mut current.seven_days, &previous.seven_days);
    }
    if current.thirty_day_scan_status == UsageScanStatus::Indexing {
        preserve_cost_if_missing(&mut current.thirty_days, &previous.thirty_days);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-06T12:00:00Z", &Rfc3339).unwrap()
    }

    fn priced_detail(now: OffsetDateTime, observed_tokens: u64, cost: f64) -> DailyCostEvidence {
        DailyCostEvidence {
            observed_tokens,
            priced_tokens: observed_tokens,
            api_equivalent_cost_usd: Some(cost),
            complete: true,
            observed_through: Some(now - Duration::minutes(1)),
            priced_observed_through: Some(now - Duration::minutes(1)),
        }
    }

    #[test]
    fn local_period_is_used_when_provider_has_no_bucket_for_that_period() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(1), 900)])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([(now.date(), priced_detail(now, 100, 2.0))]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            ..
        } = periods.today
        else {
            panic!("local Today evidence must remain available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::LocalOnly)
        );

        let UsageTotal::Current {
            observed_tokens, ..
        } = periods.seven_days
        else {
            panic!("provider-reported seven-day usage must remain available");
        };
        assert_eq!(observed_tokens, 900);
    }

    #[test]
    fn sparse_provider_windows_still_report_token_trends() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (now.date(), 300),
                (now.date() - Duration::days(2), 100),
                (now.date() - Duration::days(7), 100),
                (now.date() - Duration::days(10), 100),
                (now.date() - Duration::days(30), 100),
                (now.date() - Duration::days(40), 100),
            ])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::new(),
            local_evidence_available: false,
            local_observed_at: None,
            pricing_basis: None,
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            coverage,
            trend_percent,
            ..
        } = periods.seven_days
        else {
            panic!("seven-day provider usage must remain available");
        };
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(trend_percent, Some(100.0));

        let UsageTotal::Current { trend_percent, .. } = periods.thirty_days else {
            panic!("30-day provider usage must remain available");
        };
        assert_eq!(trend_percent, Some(200.0));
    }

    #[test]
    fn local_today_fallback_uses_local_yesterday_for_its_trend() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(2), 900)])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([
                (now.date(), priced_detail(now, 100, 2.0)),
                (now.date() - Duration::days(1), priced_detail(now, 200, 4.0)),
            ]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            evidence_basis,
            trend_percent,
            ..
        } = periods.today
        else {
            panic!("local Today evidence must remain available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(trend_percent, Some(-50.0));
    }

    #[test]
    fn period_selection_converts_offset_clocks_to_the_utc_ranking_day() {
        let now = OffsetDateTime::parse("2026-08-06T00:30:00+02:00", &Rfc3339).unwrap();
        let utc_today = Date::from_calendar_date(2026, time::Month::August, 5).unwrap();
        let local_calendar_day = Date::from_calendar_date(2026, time::Month::August, 6).unwrap();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (utc_today, 100),
                (local_calendar_day, 900),
            ])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::new(),
            local_evidence_available: false,
            local_observed_at: None,
            pricing_basis: None,
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens, ..
        } = periods.today
        else {
            panic!("the current UTC Ranking Day must be available");
        };
        assert_eq!(observed_tokens, 100);
    }

    #[test]
    fn provider_period_models_account_days_without_local_detail() {
        let now = now();
        let covered_day = now.date() - Duration::days(1);
        let missing_day = now.date() - Duration::days(8);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (covered_day, 100),
                (missing_day, 300),
            ])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([(covered_day, priced_detail(now, 100, 2.0))]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = periods.thirty_days
        else {
            panic!("account-reported 30-day usage must remain available");
        };
        assert_eq!(observed_tokens, 400);
        assert_eq!(api_equivalent_cost_usd, Some(8.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(25.0));
    }

    #[test]
    fn provider_period_does_not_model_from_unpriced_local_detail() {
        let now = now();
        let covered_day = now.date() - Duration::days(1);
        let missing_day = now.date() - Duration::days(8);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (covered_day, 100),
                (missing_day, 300),
            ])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([(
                covered_day,
                DailyCostEvidence {
                    observed_tokens: 100,
                    priced_tokens: 0,
                    api_equivalent_cost_usd: None,
                    complete: false,
                    observed_through: Some(now - Duration::minutes(1)),
                    priced_observed_through: None,
                },
            )]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = periods.thirty_days
        else {
            panic!("account-reported 30-day usage must remain available");
        };
        assert_eq!(observed_tokens, 400);
        assert_eq!(api_equivalent_cost_usd, None);
    }

    #[test]
    fn provider_period_models_unknown_price_detail_from_a_known_rate() {
        let now = now();
        let priced_day = now.date() - Duration::days(1);
        let unknown_price_day = now.date() - Duration::days(2);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (priced_day, 100),
                (unknown_price_day, 300),
            ])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([
                (priced_day, priced_detail(now, 100, 2.0)),
                (
                    unknown_price_day,
                    DailyCostEvidence {
                        observed_tokens: 300,
                        priced_tokens: 0,
                        api_equivalent_cost_usd: None,
                        complete: false,
                        observed_through: Some(now - Duration::minutes(1)),
                        priced_observed_through: None,
                    },
                ),
            ]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = periods.seven_days
        else {
            panic!("account-reported seven-day usage must remain available");
        };
        assert_eq!(observed_tokens, 400);
        assert_eq!(api_equivalent_cost_usd, Some(8.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(25.0));
    }

    #[test]
    fn local_only_period_keeps_priced_detail_when_another_model_is_unknown() {
        let now = now();
        let priced_day = now.date() - Duration::days(1);
        let unknown_price_day = now.date() - Duration::days(2);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: None,
            provider_observed_at: None,
            local_cost_evidence: BTreeMap::from([
                (priced_day, priced_detail(now, 100, 2.0)),
                (
                    unknown_price_day,
                    DailyCostEvidence {
                        observed_tokens: 300,
                        priced_tokens: 0,
                        api_equivalent_cost_usd: None,
                        complete: false,
                        observed_through: Some(now - Duration::minutes(1)),
                        priced_observed_through: None,
                    },
                ),
            ]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            ..
        } = periods.seven_days
        else {
            panic!("local evidence must remain available");
        };
        assert_eq!(observed_tokens, 400);
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::LocalOnly)
        );
    }

    #[test]
    fn provider_reported_tokens_are_not_added_to_local_tokens() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), 100)])),
            provider_observed_at: Some(now),
            local_cost_evidence: BTreeMap::from([(
                now.date(),
                DailyCostEvidence {
                    observed_tokens: 40,
                    priced_tokens: 40,
                    api_equivalent_cost_usd: Some(2.0),
                    complete: false,
                    observed_through: Some(now),
                    priced_observed_through: Some(now),
                },
            )]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Indexing,
            today_scan_status: UsageScanStatus::Indexing,
            seven_day_scan_status: UsageScanStatus::Indexing,
            thirty_day_scan_status: UsageScanStatus::Indexing,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = periods.today
        else {
            panic!("today must be available");
        };
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(5.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(40.0));
    }

    #[test]
    fn combined_usage_keeps_known_tokens_when_one_provider_is_unavailable() {
        let now = now().format(&Rfc3339).unwrap();
        let codex = UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: now,
                observed_tokens: 100,
                api_equivalent_cost_usd: Some(2.0),
                trend_percent: Some(5.0),
                api_equivalent_cost_basis: Some("openai-v1".to_owned()),
                api_equivalent_cost_quality: Some(ApiEquivalentCostQuality::Reconciled),
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let unavailable = UsagePeriods {
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };

        let combined = combine_usage_periods(&[&codex, &unavailable]);
        let UsageTotal::Current {
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = combined.today
        else {
            panic!("combined today must be available");
        };
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
    }

    #[test]
    fn combined_usage_inherits_the_weakest_cost_quality_in_any_provider_order() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let periods = |quality, coverage_percent, cost| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens: 100,
                api_equivalent_cost_usd: Some(cost),
                trend_percent: None,
                api_equivalent_cost_basis: Some("fixture-v1".to_owned()),
                api_equivalent_cost_quality: Some(quality),
                api_equivalent_cost_coverage_percent: coverage_percent,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let local_only = periods(ApiEquivalentCostQuality::LocalOnly, None, 1.0);
        let modeled = periods(ApiEquivalentCostQuality::Modeled, Some(50.0), 2.0);

        for combined in [
            combine_usage_periods(&[&local_only, &modeled]),
            combine_usage_periods(&[&modeled, &local_only]),
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = combined.today
            else {
                panic!("combined today must be available");
            };
            assert_eq!(api_equivalent_cost_usd, Some(3.0));
            assert_eq!(
                api_equivalent_cost_quality,
                Some(ApiEquivalentCostQuality::LocalOnly)
            );
            assert_eq!(api_equivalent_cost_coverage_percent, None);
        }
    }
}
