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
    pub(crate) modeled: bool,
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
            modeled: false,
            complete: true,
            observed_through: None,
            priced_observed_through: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DailyUsageEvidence {
    pub(crate) observed_tokens: u64,
    pub(crate) coverage: UsageCoverage,
    pub(crate) observed_through: Option<OffsetDateTime>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderUsageEvidence {
    pub(crate) provider_reported_tokens: Option<BTreeMap<Date, u64>>,
    pub(crate) provider_observed_at: Option<OffsetDateTime>,
    pub(crate) local_usage_evidence: BTreeMap<Date, DailyUsageEvidence>,
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

fn selected_source_previous_tokens(
    evidence: &ProviderUsageEvidence,
    evidence_basis: UsageEvidenceBasis,
    today: Date,
    length: i64,
) -> Option<u64> {
    match evidence_basis {
        UsageEvidenceBasis::ProviderReported => {
            let provider_previous = evidence
                .provider_reported_tokens
                .as_ref()
                .and_then(|daily| {
                    observed_period_sum(period_days(today, length, length), |day| {
                        daily.get(&day).copied()
                    })
                });
            provider_previous.or_else(|| {
                if length != 1 || !evidence.local_evidence_available {
                    return None;
                }
                observed_period_sum(period_days(today, length, length), |day| {
                    evidence
                        .local_usage_evidence
                        .get(&day)
                        .map(|detail| detail.observed_tokens)
                })
            })
        }
        UsageEvidenceBasis::LocallyDerived => {
            observed_period_sum(period_days(today, length, length), |day| {
                evidence
                    .local_usage_evidence
                    .get(&day)
                    .map(|detail| detail.observed_tokens)
            })
        }
        UsageEvidenceBasis::Mixed => None,
    }
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
        modeled |= detail.modeled;
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
    total_tokens: u64,
) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut priced_tokens = 0_u64;
    let mut modeled = false;
    for detail in days.filter_map(|day| local.get(&day)) {
        if detail.priced_tokens == 0 {
            continue;
        }
        if detail.observed_tokens == 0 || detail.priced_tokens > detail.observed_tokens {
            return None;
        }
        usd += detail.api_equivalent_cost_usd?;
        priced_tokens = priced_tokens.checked_add(detail.priced_tokens)?;
        modeled |= detail.modeled;
    }
    if priced_tokens == 0 || priced_tokens > total_tokens || !usd.is_finite() {
        return None;
    }
    modeled |= priced_tokens < total_tokens;
    if modeled {
        usd *= total_tokens as f64 / priced_tokens as f64;
    }
    usd.is_finite().then_some(CostProjection {
        usd,
        quality: if modeled {
            ApiEquivalentCostQuality::Modeled
        } else {
            ApiEquivalentCostQuality::LocalOnly
        },
        coverage_percent: modeled
            .then(|| ((priced_tokens as f64 / total_tokens as f64) * 100.0).clamp(0.0, 100.0)),
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
                .filter(|day| evidence.local_usage_evidence.contains_key(day))
                .count();
            let Some(tokens) = checked_sum(period_days(today, length, 0).filter_map(|day| {
                evidence
                    .local_usage_evidence
                    .get(&day)
                    .map(|detail| &detail.observed_tokens)
            })) else {
                return UsageTotal::Unavailable;
            };
            if observed_days == 0 {
                return UsageTotal::Unavailable;
            }
            let expected = usize::try_from(length).unwrap_or(usize::MAX);
            let coverage = if observed_days == expected
                && period_days(today, length, 0).all(|day| {
                    evidence
                        .local_usage_evidence
                        .get(&day)
                        .is_some_and(|detail| detail.coverage == UsageCoverage::Complete)
                }) {
                UsageCoverage::Complete
            } else {
                UsageCoverage::Partial
            };
            let observed_at = evidence
                .local_observed_at
                .or_else(|| {
                    period_days(today, length, 0)
                        .filter_map(|day| {
                            evidence
                                .local_usage_evidence
                                .get(&day)
                                .and_then(|detail| detail.observed_through)
                        })
                        .max()
                })
                .unwrap_or(now);
            (
                tokens,
                UsageEvidenceBasis::LocallyDerived,
                coverage,
                observed_at,
                locally_derived_cost(
                    &evidence.local_cost_evidence,
                    period_days(today, length, 0),
                    tokens,
                ),
            )
        } else {
            return UsageTotal::Unavailable;
        };

    let trend_previous_tokens =
        selected_source_previous_tokens(evidence, evidence_basis, today, length);
    let trend = trend_previous_tokens.and_then(|previous| trend_percent(tokens, previous));
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
        trend_previous_tokens,
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

fn combined_scan_status(
    statuses: impl Iterator<Item = (UsageScanStatus, bool)>,
) -> UsageScanStatus {
    let statuses = statuses.collect::<Vec<_>>();
    let has_contributor = statuses.iter().any(|(_, contributes)| *contributes);
    scan_status(
        statuses
            .into_iter()
            .filter(|(_, contributes)| !has_contributor || *contributes)
            .map(|(status, _)| status),
    )
}

fn usage_total_is_available(total: &UsageTotal) -> bool {
    !matches!(total, UsageTotal::Unavailable)
}

fn usage_periods_have_available_total(periods: &UsagePeriods) -> bool {
    usage_total_is_available(&periods.today)
        || usage_total_is_available(&periods.seven_days)
        || usage_total_is_available(&periods.thirty_days)
}

struct AvailableTotal<'a> {
    stale: bool,
    evidence_basis: UsageEvidenceBasis,
    coverage: UsageCoverage,
    observed_at: &'a str,
    observed_tokens: u64,
    api_equivalent_cost_usd: Option<f64>,
    trend_percent: Option<f64>,
    trend_previous_tokens: Option<u64>,
    api_equivalent_cost_basis: Option<&'a str>,
    api_equivalent_cost_quality: Option<ApiEquivalentCostQuality>,
    api_equivalent_cost_coverage_percent: Option<f64>,
}

struct PricedAvailableTotal<'a> {
    usd: f64,
    basis: &'a str,
    quality: ApiEquivalentCostQuality,
    coverage_percent: f64,
}

fn available_total(total: &UsageTotal) -> Option<AvailableTotal<'_>> {
    let (
        stale,
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        api_equivalent_cost_usd,
        trend_percent,
        trend_previous_tokens,
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
            trend_percent,
            trend_previous_tokens,
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
            *trend_percent,
            *trend_previous_tokens,
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
            trend_percent,
            trend_previous_tokens,
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
            *trend_percent,
            *trend_previous_tokens,
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
        trend_percent,
        trend_previous_tokens,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    })
}

fn priced_available_total<'a>(total: &AvailableTotal<'a>) -> Option<PricedAvailableTotal<'a>> {
    let usd = total.api_equivalent_cost_usd?;
    let basis = total.api_equivalent_cost_basis?;
    let quality = total.api_equivalent_cost_quality?;
    let coverage_percent = match quality {
        ApiEquivalentCostQuality::Modeled => total.api_equivalent_cost_coverage_percent?,
        ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly => 100.0,
    };
    (usd.is_finite()
        && usd >= 0.0
        && coverage_percent.is_finite()
        && (0.0..=100.0).contains(&coverage_percent))
    .then_some(PricedAvailableTotal {
        usd,
        basis,
        quality,
        coverage_percent,
    })
}

fn combined_trend(totals: &[AvailableTotal<'_>]) -> (Option<f64>, Option<u64>) {
    if totals.len() == 1 {
        return (totals[0].trend_percent, totals[0].trend_previous_tokens);
    }
    let Some((current_tokens, previous_tokens)) = totals
        .iter()
        .filter_map(|total| {
            let previous_tokens = total.trend_previous_tokens?;
            (previous_tokens > 0 && total.trend_percent.is_some())
                .then_some((total.observed_tokens, previous_tokens))
        })
        .try_fold((0_u64, 0_u64), |(current_sum, previous_sum), current| {
            Some((
                current_sum.checked_add(current.0)?,
                previous_sum.checked_add(current.1)?,
            ))
        })
    else {
        return (None, None);
    };
    if previous_tokens == 0 {
        return (None, None);
    }
    (
        trend_percent(current_tokens, previous_tokens),
        Some(previous_tokens),
    )
}

fn combined_cost(totals: &[AvailableTotal<'_>]) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut total_tokens = 0_u64;
    let mut covered_tokens = 0.0;
    let mut quality = ApiEquivalentCostQuality::Reconciled;
    let mut has_priced_evidence = false;
    let mut has_unpriced_tokens = false;
    for total in totals {
        total_tokens = total_tokens.checked_add(total.observed_tokens)?;
        if total.observed_tokens == 0 && total.api_equivalent_cost_usd.is_none() {
            continue;
        }
        let Some(priced) = priced_available_total(total) else {
            has_unpriced_tokens |= total.observed_tokens > 0;
            continue;
        };
        has_priced_evidence = true;
        usd += priced.usd;
        quality = weakest_cost_quality(quality, priced.quality);
        covered_tokens += priced.coverage_percent * total.observed_tokens as f64;
    }
    if !has_priced_evidence || !usd.is_finite() {
        return None;
    }
    if has_unpriced_tokens {
        quality = ApiEquivalentCostQuality::Modeled;
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
        (ApiEquivalentCostQuality::Modeled, _) | (_, ApiEquivalentCostQuality::Modeled) => {
            ApiEquivalentCostQuality::Modeled
        }
        (ApiEquivalentCostQuality::LocalOnly, _) | (_, ApiEquivalentCostQuality::LocalOnly) => {
            ApiEquivalentCostQuality::LocalOnly
        }
        (ApiEquivalentCostQuality::Reconciled, ApiEquivalentCostQuality::Reconciled) => {
            ApiEquivalentCostQuality::Reconciled
        }
    }
}

fn combined_basis(totals: &[AvailableTotal<'_>]) -> Option<String> {
    let mut bases = BTreeSet::new();
    for total in totals {
        if let Some(priced) = priced_available_total(total) {
            bases.insert(priced.basis);
        }
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
    let (trend, trend_previous_tokens) = combined_trend(&available);
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
            trend_percent: trend,
            trend_previous_tokens,
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
            trend_percent: trend,
            trend_previous_tokens,
            api_equivalent_cost_basis: fields.5,
            api_equivalent_cost_quality: fields.6,
            api_equivalent_cost_coverage_percent: fields.7,
        }
    }
}

pub(crate) fn combine_usage_periods(periods: &[&UsagePeriods]) -> UsagePeriods {
    UsagePeriods {
        scan_status: combined_scan_status(periods.iter().map(|periods| {
            (
                periods.scan_status,
                usage_periods_have_available_total(periods),
            )
        })),
        today_scan_status: combined_scan_status(periods.iter().map(|periods| {
            (
                periods.today_scan_status,
                usage_total_is_available(&periods.today),
            )
        })),
        seven_day_scan_status: combined_scan_status(periods.iter().map(|periods| {
            (
                periods.seven_day_scan_status,
                usage_total_is_available(&periods.seven_days),
            )
        })),
        thirty_day_scan_status: combined_scan_status(periods.iter().map(|periods| {
            (
                periods.thirty_day_scan_status,
                usage_total_is_available(&periods.thirty_days),
            )
        })),
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
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        }
        | UsageTotal::Stale {
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
                let previous_coverage = match previous.quality {
                    Some(ApiEquivalentCostQuality::Modeled) => {
                        previous.coverage_percent.unwrap_or(0.0)
                    }
                    Some(
                        ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly,
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
            modeled: false,
            complete: true,
            observed_through: Some(now - Duration::minutes(1)),
            priced_observed_through: Some(now - Duration::minutes(1)),
        }
    }

    fn usage_detail(
        now: OffsetDateTime,
        observed_tokens: u64,
        coverage: UsageCoverage,
    ) -> DailyUsageEvidence {
        DailyUsageEvidence {
            observed_tokens,
            coverage,
            observed_through: Some(now - Duration::minutes(1)),
        }
    }

    #[test]
    fn local_period_is_used_when_provider_has_no_bucket_for_that_period() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(1), 900)])),
            provider_observed_at: Some(now),
            local_usage_evidence: BTreeMap::from([(
                now.date(),
                usage_detail(now, 100, UsageCoverage::Partial),
            )]),
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
            local_usage_evidence: BTreeMap::new(),
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
    fn provider_today_uses_local_previous_day_when_account_bucket_is_missing() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), 200)])),
            provider_observed_at: Some(now),
            local_usage_evidence: BTreeMap::from([
                (
                    now.date() - Duration::days(1),
                    usage_detail(now, 100, UsageCoverage::Partial),
                ),
                (
                    now.date() - Duration::days(7),
                    usage_detail(now, 50, UsageCoverage::Partial),
                ),
            ]),
            local_cost_evidence: BTreeMap::new(),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: None,
            scan_status: UsageScanStatus::Unavailable,
            today_scan_status: UsageScanStatus::Unavailable,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            evidence_basis,
            observed_tokens,
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.today
        else {
            panic!("provider Today usage must remain available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(observed_tokens, 200);
        assert_eq!(trend_previous_tokens, Some(100));
        assert_eq!(trend_percent, Some(100.0));

        let UsageTotal::Current { trend_percent, .. } = periods.seven_days else {
            panic!("provider 7-day usage must remain available");
        };
        assert_eq!(trend_percent, None);
    }

    #[test]
    fn local_today_fallback_uses_local_yesterday_for_its_trend() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(2), 900)])),
            provider_observed_at: Some(now),
            local_usage_evidence: BTreeMap::from([
                (now.date(), usage_detail(now, 100, UsageCoverage::Partial)),
                (
                    now.date() - Duration::days(1),
                    usage_detail(now, 200, UsageCoverage::Partial),
                ),
            ]),
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
    fn complete_local_daily_evidence_produces_complete_period_coverage() {
        let now = now();
        let local_usage_evidence = period_days(now.date(), 7, 0)
            .enumerate()
            .map(|(index, day)| {
                (
                    day,
                    usage_detail(
                        now,
                        if index == 0 { 0 } else { 100 },
                        UsageCoverage::Complete,
                    ),
                )
            })
            .collect();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: None,
            provider_observed_at: None,
            local_usage_evidence,
            local_cost_evidence: BTreeMap::new(),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: None,
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            coverage,
            observed_tokens,
            ..
        } = periods.today
        else {
            panic!("an observed zero-token day must remain available");
        };
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 0);

        let UsageTotal::Current {
            coverage,
            observed_tokens,
            ..
        } = periods.seven_days
        else {
            panic!("complete local seven-day evidence must remain available");
        };
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 600);
    }

    #[test]
    fn sixty_day_token_history_supports_a_trend_with_thirty_day_cost_detail() {
        let now = now();
        let local_usage_evidence = period_days(now.date(), 60, 0)
            .map(|day| (day, usage_detail(now, 10, UsageCoverage::Complete)))
            .collect();
        let local_cost_evidence = period_days(now.date(), 30, 0)
            .map(|day| (day, priced_detail(now, 10, 1.0)))
            .collect();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: None,
            provider_observed_at: None,
            local_usage_evidence,
            local_cost_evidence,
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
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.thirty_days
        else {
            panic!("complete local 30-day evidence must remain available");
        };
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 300);
        assert_eq!(api_equivalent_cost_usd, Some(30.0));
        assert_eq!(trend_percent, Some(0.0));
        assert_eq!(trend_previous_tokens, Some(300));
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
            local_usage_evidence: BTreeMap::new(),
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
            local_usage_evidence: BTreeMap::new(),
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
            local_usage_evidence: BTreeMap::new(),
            local_cost_evidence: BTreeMap::from([(
                covered_day,
                DailyCostEvidence {
                    observed_tokens: 100,
                    priced_tokens: 0,
                    api_equivalent_cost_usd: None,
                    modeled: false,
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
            local_usage_evidence: BTreeMap::new(),
            local_cost_evidence: BTreeMap::from([
                (priced_day, priced_detail(now, 100, 2.0)),
                (
                    unknown_price_day,
                    DailyCostEvidence {
                        observed_tokens: 300,
                        priced_tokens: 0,
                        api_equivalent_cost_usd: None,
                        modeled: false,
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
    fn local_only_period_models_unknown_price_detail_from_a_known_rate() {
        let now = now();
        let priced_day = now.date() - Duration::days(1);
        let unknown_price_day = now.date() - Duration::days(2);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: None,
            provider_observed_at: None,
            local_usage_evidence: BTreeMap::from([
                (priced_day, usage_detail(now, 100, UsageCoverage::Partial)),
                (
                    unknown_price_day,
                    usage_detail(now, 300, UsageCoverage::Partial),
                ),
            ]),
            local_cost_evidence: BTreeMap::from([
                (priced_day, priced_detail(now, 100, 2.0)),
                (
                    unknown_price_day,
                    DailyCostEvidence {
                        observed_tokens: 300,
                        priced_tokens: 0,
                        api_equivalent_cost_usd: None,
                        modeled: false,
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
            panic!("local evidence must remain available");
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
    fn provider_reported_tokens_are_not_added_to_local_tokens() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), 100)])),
            provider_observed_at: Some(now),
            local_usage_evidence: BTreeMap::new(),
            local_cost_evidence: BTreeMap::from([(
                now.date(),
                DailyCostEvidence {
                    observed_tokens: 40,
                    priced_tokens: 40,
                    api_equivalent_cost_usd: Some(2.0),
                    modeled: false,
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
                trend_previous_tokens: None,
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
            trend_percent,
            ..
        } = combined.today
        else {
            panic!("combined today must be available");
        };
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
        assert_eq!(trend_percent, Some(5.0));
    }

    #[test]
    fn combined_scan_status_ignores_indexing_providers_without_period_evidence() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let available_total = || UsageTotal::Current {
            evidence_basis: UsageEvidenceBasis::ProviderReported,
            coverage: UsageCoverage::Complete,
            observed_at: observed_at.clone(),
            observed_tokens: 100,
            api_equivalent_cost_usd: Some(2.0),
            trend_percent: None,
            trend_previous_tokens: None,
            api_equivalent_cost_basis: Some("openai-v1".to_owned()),
            api_equivalent_cost_quality: Some(ApiEquivalentCostQuality::Reconciled),
            api_equivalent_cost_coverage_percent: None,
        };
        let observed = UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: available_total(),
            seven_days: available_total(),
            thirty_days: available_total(),
        };
        let indexing_without_evidence = UsagePeriods {
            scan_status: UsageScanStatus::Indexing,
            today_scan_status: UsageScanStatus::Indexing,
            seven_day_scan_status: UsageScanStatus::Indexing,
            thirty_day_scan_status: UsageScanStatus::Indexing,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };

        for combined in [
            combine_usage_periods(&[&observed, &indexing_without_evidence]),
            combine_usage_periods(&[&indexing_without_evidence, &observed]),
        ] {
            assert_eq!(combined.scan_status, UsageScanStatus::Complete);
            assert_eq!(combined.today_scan_status, UsageScanStatus::Complete);
            assert_eq!(combined.seven_day_scan_status, UsageScanStatus::Complete);
            assert_eq!(combined.thirty_day_scan_status, UsageScanStatus::Complete);
        }

        let combined = combine_usage_periods(&[&indexing_without_evidence]);
        assert_eq!(combined.scan_status, UsageScanStatus::Indexing);
        assert_eq!(combined.today_scan_status, UsageScanStatus::Indexing);
        assert_eq!(combined.seven_day_scan_status, UsageScanStatus::Indexing);
        assert_eq!(combined.thirty_day_scan_status, UsageScanStatus::Indexing);

        let indexing_today_only = UsagePeriods {
            scan_status: UsageScanStatus::Indexing,
            today_scan_status: UsageScanStatus::Indexing,
            seven_day_scan_status: UsageScanStatus::Indexing,
            thirty_day_scan_status: UsageScanStatus::Indexing,
            today: available_total(),
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let combined = combine_usage_periods(&[&observed, &indexing_today_only]);
        assert_eq!(combined.scan_status, UsageScanStatus::Indexing);
        assert_eq!(combined.today_scan_status, UsageScanStatus::Indexing);
        assert_eq!(combined.seven_day_scan_status, UsageScanStatus::Complete);
        assert_eq!(combined.thirty_day_scan_status, UsageScanStatus::Complete);
    }

    #[test]
    fn combined_cost_keeps_priced_evidence_and_reports_modeled_coverage() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let periods = |tokens, cost, basis, quality| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens: tokens,
                api_equivalent_cost_usd: cost,
                trend_percent: None,
                trend_previous_tokens: None,
                api_equivalent_cost_basis: basis,
                api_equivalent_cost_quality: quality,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let priced = periods(
            100,
            Some(2.0),
            Some("openai-v1".to_owned()),
            Some(ApiEquivalentCostQuality::Reconciled),
        );
        let unpriced = periods(300, None, None, None);

        for combined in [
            combine_usage_periods(&[&priced, &unpriced]),
            combine_usage_periods(&[&unpriced, &priced]),
        ] {
            let UsageTotal::Current {
                observed_tokens,
                api_equivalent_cost_usd,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = combined.today
            else {
                panic!("combined Today usage must be available");
            };
            assert_eq!(observed_tokens, 400);
            assert_eq!(api_equivalent_cost_usd, Some(2.0));
            assert_eq!(api_equivalent_cost_basis.as_deref(), Some("openai-v1"));
            assert_eq!(
                api_equivalent_cost_quality,
                Some(ApiEquivalentCostQuality::Modeled)
            );
            assert_eq!(api_equivalent_cost_coverage_percent, Some(25.0));
        }
    }

    #[test]
    fn combined_cost_keeps_a_local_only_subtotal_when_a_peer_is_unpriced() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let total = |tokens, cost, basis, quality| UsageTotal::Current {
            evidence_basis: UsageEvidenceBasis::LocallyDerived,
            coverage: UsageCoverage::Complete,
            observed_at: observed_at.clone(),
            observed_tokens: tokens,
            api_equivalent_cost_usd: cost,
            trend_percent: None,
            trend_previous_tokens: None,
            api_equivalent_cost_basis: basis,
            api_equivalent_cost_quality: quality,
            api_equivalent_cost_coverage_percent: None,
        };
        let periods = |today| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let claude = periods(total(
            300,
            Some(6.0),
            Some("anthropic-v1".to_owned()),
            Some(ApiEquivalentCostQuality::LocalOnly),
        ));
        let unpriced_peer = periods(total(100, None, None, None));

        for combined in [
            combine_usage_periods(&[&claude, &unpriced_peer]),
            combine_usage_periods(&[&unpriced_peer, &claude]),
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = combined.today
            else {
                panic!("combined Today usage must be available");
            };
            assert_eq!(api_equivalent_cost_usd, Some(6.0));
            assert_eq!(api_equivalent_cost_basis.as_deref(), Some("anthropic-v1"));
            assert_eq!(
                api_equivalent_cost_quality,
                Some(ApiEquivalentCostQuality::Modeled)
            );
            assert_eq!(api_equivalent_cost_coverage_percent, Some(75.0));
        }
    }

    #[test]
    fn combined_cost_stays_unavailable_when_no_provider_has_priced_evidence() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let unpriced = |tokens| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Unavailable,
            thirty_day_scan_status: UsageScanStatus::Unavailable,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens: tokens,
                api_equivalent_cost_usd: None,
                trend_percent: None,
                trend_previous_tokens: None,
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let codex = unpriced(100);
        let claude = unpriced(300);

        for combined in [
            combine_usage_periods(&[&codex, &claude]),
            combine_usage_periods(&[&claude, &codex]),
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = combined.today
            else {
                panic!("combined Today usage must be available");
            };
            assert_eq!(api_equivalent_cost_usd, None);
            assert_eq!(api_equivalent_cost_basis, None);
            assert_eq!(api_equivalent_cost_quality, None);
            assert_eq!(api_equivalent_cost_coverage_percent, None);
        }
    }

    #[test]
    fn combined_usage_weights_provider_trends_by_previous_tokens() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let periods = |observed_tokens, trend_percent, trend_previous_tokens| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens,
                api_equivalent_cost_usd: None,
                trend_percent: Some(trend_percent),
                trend_previous_tokens: Some(trend_previous_tokens),
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let codex = periods(200, 100.0, 100);
        let claude = periods(900, 0.0, 900);

        let combined = combine_usage_periods(&[&codex, &claude]);
        let UsageTotal::Current {
            observed_tokens,
            trend_percent,
            ..
        } = combined.today
        else {
            panic!("combined today must be available");
        };
        assert_eq!(observed_tokens, 1_100);
        assert!((trend_percent.unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn combined_usage_keeps_a_valid_trend_when_another_provider_has_no_previous_period() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let periods = |observed_tokens, trend_percent, trend_previous_tokens| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens,
                api_equivalent_cost_usd: None,
                trend_percent,
                trend_previous_tokens,
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let claude = periods(200, Some(100.0), Some(100));
        let current_only_codex = periods(50, None, None);

        let combined = combine_usage_periods(&[&current_only_codex, &claude]);
        let UsageTotal::Current {
            observed_tokens,
            trend_percent,
            trend_previous_tokens,
            ..
        } = combined.today
        else {
            panic!("combined Today usage must be available");
        };

        assert_eq!(observed_tokens, 250);
        assert_eq!(trend_percent, Some(100.0));
        assert_eq!(trend_previous_tokens, Some(100));
    }

    #[test]
    fn combined_usage_keeps_previous_weight_when_one_provider_drops_to_zero() {
        let observed_at = now().format(&Rfc3339).unwrap();
        let periods = |observed_tokens, trend_percent, previous_tokens| UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: observed_at.clone(),
                observed_tokens,
                api_equivalent_cost_usd: None,
                trend_percent: Some(trend_percent),
                trend_previous_tokens: Some(previous_tokens),
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        };
        let codex = periods(0, -100.0, 100);
        let claude = periods(900, 0.0, 900);

        let combined = combine_usage_periods(&[&codex, &claude]);
        let UsageTotal::Current { trend_percent, .. } = combined.today else {
            panic!("combined today must be available");
        };
        assert_eq!(trend_percent, Some(-10.0));
    }

    #[test]
    fn combined_usage_keeps_modeled_coverage_with_local_only_cost_in_any_provider_order() {
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
                trend_previous_tokens: None,
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
                Some(ApiEquivalentCostQuality::Modeled)
            );
            assert_eq!(api_equivalent_cost_coverage_percent, Some(75.0));
        }
    }
}
