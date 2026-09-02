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
    /// The effective-dated pricing catalog that priced this UTC day.
    pub(crate) pricing_basis: Option<String>,
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
            pricing_basis: None,
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
    pub(crate) provider_observed_at_by_day: BTreeMap<Date, OffsetDateTime>,
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

#[derive(Clone, Copy)]
struct SelectedUsageDay {
    observed_tokens: u64,
    evidence_basis: UsageEvidenceBasis,
    coverage: UsageCoverage,
    observed_at: OffsetDateTime,
}

struct SelectedUsagePeriod {
    observed_tokens: u64,
    provider_tokens: u64,
    local_tokens: u64,
    evidence_basis: UsageEvidenceBasis,
    coverage: UsageCoverage,
    observed_at: OffsetDateTime,
    provider_days: Vec<Date>,
    local_days: Vec<Date>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RetainedCostProjection {
    pub(crate) amount: f64,
    pub(crate) quality: Option<ApiEquivalentCostQuality>,
    pub(crate) coverage_percent: Option<f64>,
}

/// Scale a retained cost when a newer cumulative token total has no cost.
///
/// `amount` can use any non-negative unit. Callers can use USD or integer-like
/// micros because the projection only applies a ratio.
pub(crate) fn project_retained_cost(
    previous_amount: f64,
    previous_observed_tokens: u64,
    previous_quality: Option<ApiEquivalentCostQuality>,
    previous_coverage_percent: Option<f64>,
    observed_tokens: u64,
) -> Option<RetainedCostProjection> {
    if !previous_amount.is_finite() || previous_amount < 0.0 {
        return None;
    }
    if observed_tokens == previous_observed_tokens {
        return Some(RetainedCostProjection {
            amount: previous_amount,
            quality: previous_quality,
            coverage_percent: previous_coverage_percent,
        });
    }
    if previous_observed_tokens == 0 || observed_tokens == 0 {
        return None;
    }
    let previous_coverage = match previous_quality {
        Some(ApiEquivalentCostQuality::Modeled) => previous_coverage_percent.unwrap_or(0.0),
        Some(ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly) => 100.0,
        None => return None,
    };
    let amount = previous_amount * (observed_tokens as f64 / previous_observed_tokens as f64);
    if !amount.is_finite() {
        return None;
    }
    Some(RetainedCostProjection {
        amount,
        quality: Some(ApiEquivalentCostQuality::Modeled),
        coverage_percent: Some(
            (previous_observed_tokens as f64 * previous_coverage / observed_tokens as f64)
                .clamp(0.0, 100.0),
        ),
    })
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

fn provider_observed_at_for_day(
    evidence: &ProviderUsageEvidence,
    day: Date,
) -> Option<OffsetDateTime> {
    evidence
        .provider_observed_at_by_day
        .get(&day)
        .copied()
        .or(evidence.provider_observed_at)
}

fn select_usage_day(
    evidence: &ProviderUsageEvidence,
    day: Date,
    observed_at_fallback: OffsetDateTime,
) -> Option<SelectedUsageDay> {
    if let Some((observed_tokens, observed_at)) = evidence
        .provider_reported_tokens
        .as_ref()
        .and_then(|daily| daily.get(&day).copied())
        .zip(provider_observed_at_for_day(evidence, day))
    {
        return Some(SelectedUsageDay {
            observed_tokens,
            evidence_basis: UsageEvidenceBasis::ProviderReported,
            coverage: UsageCoverage::Complete,
            observed_at,
        });
    }
    if !evidence.local_evidence_available {
        return None;
    }
    let local = evidence.local_usage_evidence.get(&day)?;
    Some(SelectedUsageDay {
        observed_tokens: local.observed_tokens,
        evidence_basis: UsageEvidenceBasis::LocallyDerived,
        coverage: local.coverage,
        observed_at: local
            .observed_through
            .or(evidence.local_observed_at)
            .unwrap_or(observed_at_fallback),
    })
}

fn select_usage_period(
    evidence: &ProviderUsageEvidence,
    days: impl Iterator<Item = Date>,
    observed_at_fallback: OffsetDateTime,
) -> Option<SelectedUsagePeriod> {
    let mut expected_days = 0_usize;
    let mut selected_days = 0_usize;
    let mut observed_tokens = 0_u64;
    let mut provider_tokens = 0_u64;
    let mut local_tokens = 0_u64;
    let mut complete = true;
    let mut observed_at = None;
    let mut provider_days = Vec::new();
    let mut local_days = Vec::new();

    for day in days {
        expected_days = expected_days.checked_add(1)?;
        let Some(selected) = select_usage_day(evidence, day, observed_at_fallback) else {
            complete = false;
            continue;
        };
        selected_days = selected_days.checked_add(1)?;
        observed_tokens = observed_tokens.checked_add(selected.observed_tokens)?;
        complete &= selected.coverage == UsageCoverage::Complete;
        observed_at = Some(
            observed_at.map_or(selected.observed_at, |current: OffsetDateTime| {
                current.min(selected.observed_at)
            }),
        );
        match selected.evidence_basis {
            UsageEvidenceBasis::ProviderReported => {
                provider_tokens = provider_tokens.checked_add(selected.observed_tokens)?;
                provider_days.push(day);
            }
            UsageEvidenceBasis::LocallyDerived => {
                local_tokens = local_tokens.checked_add(selected.observed_tokens)?;
                local_days.push(day);
            }
            UsageEvidenceBasis::Mixed => return None,
        }
    }
    if selected_days == 0 {
        return None;
    }
    complete &= selected_days == expected_days;
    let evidence_basis = match (provider_days.is_empty(), local_days.is_empty()) {
        (false, true) => UsageEvidenceBasis::ProviderReported,
        (true, false) => UsageEvidenceBasis::LocallyDerived,
        (false, false) => UsageEvidenceBasis::Mixed,
        (true, true) => return None,
    };
    Some(SelectedUsagePeriod {
        observed_tokens,
        provider_tokens,
        local_tokens,
        evidence_basis,
        coverage: if complete {
            UsageCoverage::Complete
        } else {
            UsageCoverage::Partial
        },
        observed_at: observed_at?,
        provider_days,
        local_days,
    })
}

fn provider_reported_cost(
    provider_tokens: &BTreeMap<Date, u64>,
    days: impl Iterator<Item = Date>,
    local: &BTreeMap<Date, DailyCostEvidence>,
    mut observed_at_for_day: impl FnMut(Date) -> Option<OffsetDateTime>,
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
        let provider_observed_at = observed_at_for_day(day)?;
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

fn combine_selected_period_cost(
    total_tokens: u64,
    parts: [(u64, Option<CostProjection>); 2],
) -> Option<CostProjection> {
    let mut usd = 0.0;
    let mut covered_tokens = 0.0;
    let mut quality = None;
    let mut has_unpriced_tokens = false;
    for (tokens, projection) in parts {
        let Some(projection) = projection else {
            has_unpriced_tokens |= tokens > 0;
            continue;
        };
        usd += projection.usd;
        covered_tokens += match projection.quality {
            ApiEquivalentCostQuality::Modeled => {
                projection.coverage_percent.unwrap_or(0.0) * tokens as f64 / 100.0
            }
            ApiEquivalentCostQuality::Reconciled | ApiEquivalentCostQuality::LocalOnly => {
                tokens as f64
            }
        };
        quality = Some(match quality {
            Some(current) => weakest_cost_quality(current, projection.quality),
            None => projection.quality,
        });
    }
    if !usd.is_finite() {
        return None;
    }
    let mut quality = quality?;
    if has_unpriced_tokens {
        quality = ApiEquivalentCostQuality::Modeled;
    }
    Some(CostProjection {
        usd,
        quality,
        coverage_percent: (quality == ApiEquivalentCostQuality::Modeled).then(|| {
            if total_tokens == 0 {
                100.0
            } else {
                ((covered_tokens / total_tokens as f64) * 100.0).clamp(0.0, 100.0)
            }
        }),
    })
}

fn selected_period_cost(
    evidence: &ProviderUsageEvidence,
    selected: &SelectedUsagePeriod,
) -> Option<CostProjection> {
    let provider_cost = (!selected.provider_days.is_empty())
        .then(|| {
            evidence
                .local_evidence_available
                .then(|| {
                    provider_reported_cost(
                        evidence.provider_reported_tokens.as_ref()?,
                        selected.provider_days.iter().copied(),
                        &evidence.local_cost_evidence,
                        |day| provider_observed_at_for_day(evidence, day),
                    )
                })
                .flatten()
        })
        .flatten();
    let local_cost = (!selected.local_days.is_empty())
        .then(|| {
            locally_derived_cost(
                &evidence.local_cost_evidence,
                selected.local_days.iter().copied(),
                selected.local_tokens,
            )
        })
        .flatten();

    match selected.evidence_basis {
        UsageEvidenceBasis::ProviderReported => provider_cost,
        UsageEvidenceBasis::LocallyDerived => local_cost,
        UsageEvidenceBasis::Mixed => combine_selected_period_cost(
            selected.observed_tokens,
            [
                (selected.provider_tokens, provider_cost),
                (selected.local_tokens, local_cost),
            ],
        ),
    }
}

fn project_period(
    evidence: &ProviderUsageEvidence,
    now: OffsetDateTime,
    length: i64,
) -> UsageTotal {
    let today = now.to_offset(UtcOffset::UTC).date();
    let Some(selected) = select_usage_period(evidence, period_days(today, length, 0), now) else {
        return UsageTotal::Unavailable;
    };
    let cost = selected_period_cost(evidence, &selected);
    let trend_previous_tokens =
        select_usage_period(evidence, period_days(today, length, length), now)
            .map(|previous| previous.observed_tokens);
    let trend = trend_previous_tokens
        .and_then(|previous| trend_percent(selected.observed_tokens, previous));
    let observed_at = selected
        .observed_at
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    UsageTotal::Current {
        evidence_basis: selected.evidence_basis,
        coverage: selected.coverage,
        observed_at,
        observed_tokens: selected.observed_tokens,
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

/// Project sparse, sanitized UTC-day facts without deriving them from rolling totals.
///
/// Provider-owned evidence wins for one provider and day. Local evidence can
/// still price the provider-owned token count, but the two token counts are
/// never added together.
pub(crate) fn calculate_daily_usage_aggregates(
    evidence: &ProviderUsageEvidence,
    observed_at_fallback: OffsetDateTime,
    anchor_day: Date,
    length: i64,
) -> BTreeMap<Date, UsageTotal> {
    period_days(anchor_day, length, 0)
        .filter_map(|day| {
            let local_cost = evidence.local_cost_evidence.get(&day);
            let selected = select_usage_day(evidence, day, observed_at_fallback)?;
            let cost = match selected.evidence_basis {
                UsageEvidenceBasis::ProviderReported => evidence
                    .local_evidence_available
                    .then(|| {
                        provider_reported_cost(
                            evidence.provider_reported_tokens.as_ref()?,
                            std::iter::once(day),
                            &evidence.local_cost_evidence,
                            |day| provider_observed_at_for_day(evidence, day),
                        )
                    })
                    .flatten(),
                UsageEvidenceBasis::LocallyDerived => locally_derived_cost(
                    &evidence.local_cost_evidence,
                    std::iter::once(day),
                    selected.observed_tokens,
                ),
                UsageEvidenceBasis::Mixed => None,
            };
            let observed_at = selected.observed_at.format(&Rfc3339).ok()?;
            let pricing_basis = local_cost.and_then(|detail| detail.pricing_basis.clone());
            // A day can only retain a numeric cost with that day's exact
            // effective-dated catalog. A current provider-wide basis cannot
            // relabel an older stored cost.
            let cost = cost.filter(|_| pricing_basis.is_some());
            Some((
                day,
                UsageTotal::Current {
                    evidence_basis: selected.evidence_basis,
                    coverage: selected.coverage,
                    observed_at,
                    observed_tokens: selected.observed_tokens,
                    api_equivalent_cost_usd: cost.map(|projection| projection.usd),
                    trend_percent: None,
                    trend_previous_tokens: None,
                    api_equivalent_cost_basis: cost.and(pricing_basis),
                    api_equivalent_cost_quality: cost.map(|projection| projection.quality),
                    api_equivalent_cost_coverage_percent: cost
                        .and_then(|projection| projection.coverage_percent),
                },
            ))
        })
        .collect()
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
            let Some(projection) = project_retained_cost(
                previous.usd,
                previous.observed_tokens,
                previous.quality,
                previous.coverage_percent,
                *observed_tokens,
            ) else {
                return;
            };
            *api_equivalent_cost_usd = Some(projection.amount);
            *api_equivalent_cost_basis = previous.basis;
            *api_equivalent_cost_quality = projection.quality;
            *api_equivalent_cost_coverage_percent = projection.coverage_percent;
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
            pricing_basis: Some("fixture-v1".to_owned()),
        }
    }

    #[test]
    fn thirty_day_daily_aggregates_keep_sparse_days_and_use_one_source() {
        let now = now();
        let today = now.date();
        let historical_fetch = now + Duration::minutes(2);
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([
                (today, 40),
                (today - Duration::days(1), 80),
                (today - Duration::days(30), 9_999),
            ])),
            provider_observed_at: Some(historical_fetch),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([
                (
                    today - Duration::days(1),
                    usage_detail(now, 20, UsageCoverage::Partial),
                ),
                (
                    today - Duration::days(29),
                    usage_detail(now, 10, UsageCoverage::Partial),
                ),
                (
                    today - Duration::days(30),
                    usage_detail(now, 10, UsageCoverage::Complete),
                ),
            ]),
            local_cost_evidence: BTreeMap::from([
                (today - Duration::days(1), priced_detail(now, 20, 2.0)),
                (today - Duration::days(29), priced_detail(now, 10, 1.0)),
            ]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let daily = calculate_daily_usage_aggregates(&evidence, now, today, 30);

        assert_eq!(daily.len(), 3);
        assert!(!daily.contains_key(&(today - Duration::days(30))));
        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_at,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            ..
        } = &daily[&(today - Duration::days(1))]
        else {
            panic!("the historical provider day must be available");
        };
        assert_eq!(*evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(*coverage, UsageCoverage::Complete);
        assert_eq!(*observed_tokens, 80);
        assert_eq!(observed_at, "2026-08-06T12:02:00Z");
        assert_eq!(*api_equivalent_cost_usd, Some(8.0));
        assert_eq!(api_equivalent_cost_basis.as_deref(), Some("fixture-v1"));

        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            ..
        } = &daily[&(today - Duration::days(29))]
        else {
            panic!("the sparse local day must be available");
        };
        assert_eq!(*evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(*coverage, UsageCoverage::Partial);
        assert_eq!(*observed_tokens, 10);
    }

    #[test]
    fn daily_cost_requires_that_days_exact_pricing_basis() {
        let now = now();
        let today = now.date();
        let mut cost = priced_detail(now, 100, 2.0);
        cost.pricing_basis = None;
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(today, 100)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::new(),
            local_cost_evidence: BTreeMap::from([(today, cost)]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("new-current-catalog-must-not-relabel-old-day".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
        };

        let daily = calculate_daily_usage_aggregates(&evidence, now, today, 1);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = &daily[&today]
        else {
            panic!("daily usage must remain available");
        };
        assert_eq!(*api_equivalent_cost_usd, None);
        assert_eq!(*api_equivalent_cost_basis, None);
        assert_eq!(*api_equivalent_cost_quality, None);
        assert_eq!(*api_equivalent_cost_coverage_percent, None);
    }

    #[test]
    fn retained_cost_projection_scales_amount_and_coverage() {
        assert_eq!(
            project_retained_cost(
                1_000_000.0,
                100,
                Some(ApiEquivalentCostQuality::Reconciled),
                None,
                125,
            ),
            Some(RetainedCostProjection {
                amount: 1_250_000.0,
                quality: Some(ApiEquivalentCostQuality::Modeled),
                coverage_percent: Some(80.0),
            })
        );
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
    fn missing_provider_today_uses_local_and_the_larger_period_is_mixed() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(1), 900)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
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
            evidence_basis,
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            trend_percent,
            ..
        } = periods.seven_days
        else {
            panic!("mixed seven-day usage must remain available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::Mixed);
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(observed_tokens, 1_000);
        assert_eq!(api_equivalent_cost_usd, Some(2.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(10.0));
        assert_eq!(trend_percent, None);
    }

    #[test]
    fn complete_mixed_periods_keep_token_trends() {
        let now = now();
        let provider_reported_tokens = period_days(now.date(), 6, 1)
            .chain(period_days(now.date(), 7, 7))
            .map(|day| (day, 100))
            .collect();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(provider_reported_tokens),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([(
                now.date(),
                usage_detail(now, 200, UsageCoverage::Complete),
            )]),
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
            evidence_basis,
            coverage,
            observed_tokens,
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.seven_days
        else {
            panic!("complete mixed seven-day usage must remain available");
        };

        assert_eq!(evidence_basis, UsageEvidenceBasis::Mixed);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 800);
        assert_eq!(trend_previous_tokens, Some(700));
        assert!((trend_percent.unwrap() - 14.285_714_285_714_286).abs() < 1e-9);
    }

    #[test]
    fn sparse_provider_windows_keep_best_effort_token_trends() {
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
            provider_observed_at_by_day: BTreeMap::new(),
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
            trend_previous_tokens,
            ..
        } = periods.seven_days
        else {
            panic!("seven-day provider usage must remain available");
        };
        assert_eq!(coverage, UsageCoverage::Partial);
        assert_eq!(trend_previous_tokens, Some(200));
        assert_eq!(trend_percent, Some(100.0));

        let UsageTotal::Current {
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.thirty_days
        else {
            panic!("30-day provider usage must remain available");
        };
        assert_eq!(trend_previous_tokens, Some(200));
        assert_eq!(trend_percent, Some(200.0));
    }

    #[test]
    fn provider_today_uses_local_previous_day_when_account_bucket_is_missing() {
        let now = now();
        let mut evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), 200)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([
                (
                    now.date() - Duration::days(1),
                    usage_detail(now, 100, UsageCoverage::Complete),
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

        evidence
            .local_usage_evidence
            .get_mut(&(now.date() - Duration::days(1)))
            .unwrap()
            .coverage = UsageCoverage::Partial;
        let periods_with_partial_previous = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods_with_partial_previous.today
        else {
            panic!("provider Today usage must remain available");
        };
        assert_eq!(trend_previous_tokens, Some(100));
        assert_eq!(trend_percent, Some(100.0));

        let UsageTotal::Current {
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.seven_days
        else {
            panic!("provider 7-day usage must remain available");
        };
        assert_eq!(trend_previous_tokens, Some(50));
        assert_eq!(trend_percent, Some(500.0));
    }

    #[test]
    fn local_today_fallback_uses_local_yesterday_for_its_trend() {
        let now = now();
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date() - Duration::days(2), 900)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([
                (now.date(), usage_detail(now, 100, UsageCoverage::Complete)),
                (
                    now.date() - Duration::days(1),
                    usage_detail(now, 200, UsageCoverage::Complete),
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
            provider_observed_at_by_day: BTreeMap::new(),
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
            provider_observed_at_by_day: BTreeMap::new(),
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
            provider_observed_at_by_day: BTreeMap::new(),
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
            provider_observed_at_by_day: BTreeMap::new(),
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
            provider_observed_at_by_day: BTreeMap::new(),
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
                    pricing_basis: None,
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
            provider_observed_at_by_day: BTreeMap::new(),
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
                        pricing_basis: None,
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
            provider_observed_at_by_day: BTreeMap::new(),
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
                        pricing_basis: None,
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
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), 0)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([(
                now.date(),
                usage_detail(now, 40, UsageCoverage::Complete),
            )]),
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
                    pricing_basis: Some("fixture-v1".to_owned()),
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
            evidence_basis,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = periods.today
        else {
            panic!("today must be available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(observed_tokens, 0);
        assert_eq!(api_equivalent_cost_usd, Some(0.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(100.0));
    }

    #[test]
    fn provider_reported_today_remains_authoritative_after_local_scan_completes() {
        let now = now();
        let provider_tokens = 467_600;
        let local_tokens = 1_100_000_000;
        let local_cost = 675.78;
        let evidence = ProviderUsageEvidence {
            provider_reported_tokens: Some(BTreeMap::from([(now.date(), provider_tokens)])),
            provider_observed_at: Some(now),
            provider_observed_at_by_day: BTreeMap::new(),
            local_usage_evidence: BTreeMap::from([(
                now.date(),
                usage_detail(now, local_tokens, UsageCoverage::Complete),
            )]),
            local_cost_evidence: BTreeMap::from([(
                now.date(),
                priced_detail(now, local_tokens, local_cost),
            )]),
            local_evidence_available: true,
            local_observed_at: Some(now),
            pricing_basis: Some("fixture-v1".to_owned()),
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Indexing,
            thirty_day_scan_status: UsageScanStatus::Indexing,
        };

        let periods = calculate_usage_periods(&evidence, now);
        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = periods.today
        else {
            panic!("provider-reported Today usage must be available");
        };

        assert_eq!(evidence_basis, UsageEvidenceBasis::ProviderReported);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, provider_tokens);
        let expected_cost = local_cost * provider_tokens as f64 / local_tokens as f64;
        assert!((api_equivalent_cost_usd.unwrap() - expected_cost).abs() < 1e-12);
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(100.0));
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
