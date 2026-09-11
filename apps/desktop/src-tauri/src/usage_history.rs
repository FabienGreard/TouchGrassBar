//! Bounded chart projections from the same daily evidence used by the usage totals.
//! No provider content, private model names, paths, or inferred hourly distribution crosses IPC.
use crate::daily_usage_aggregate::combine_total;
use crate::providers::{self, CodingProvider, ProviderDailyUsage};
use crate::sanitized::{ProviderPresentation, TopModelUsage, UsageTotal};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::{Date, Duration, OffsetDateTime, UtcOffset};

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistory {
    #[schemars(length(min = 10, max = 10))]
    pub today: String,
    #[schemars(length(max = 3))]
    pub scopes: Vec<UsageHistoryScope>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryScope {
    /// None is the combined projection of visible, enabled providers.
    pub provider: Option<CodingProvider>,
    #[schemars(length(max = 30))]
    pub days: Vec<UsageHistoryDay>,
    pub top_models: UsageHistoryModels,
    #[schemars(length(max = 24))]
    pub hours: Vec<UsageHistoryHour>,
    pub hourly_matches_total: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryDay {
    #[schemars(length(min = 10, max = 10))]
    pub day: String,
    pub total: UsageTotal,
    pub top_model_usage: Option<TopModelUsage>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryModels {
    pub today: Option<TopModelUsage>,
    pub seven_days: Option<TopModelUsage>,
    pub thirty_days: Option<TopModelUsage>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryHour {
    #[schemars(range(min = 0, max = 23))]
    pub hour: u8,
    pub total: UsageTotal,
    pub top_model_usage: Option<TopModelUsage>,
}

#[derive(Default)]
pub(crate) struct ProviderHistoryDetail {
    pub models: Vec<ModelDay>,
    pub hours: Vec<HourPart>,
}

pub(crate) struct HourPart {
    pub provider: CodingProvider,
    pub hour: u8,
    pub key: String,
    pub display: Option<String>,
    pub tokens: u64,
    pub cost: Option<f64>,
    pub priced_tokens: u64,
    pub modeled: bool,
    pub complete: bool,
    pub observed_at: OffsetDateTime,
    pub basis: Option<String>,
}

/// Internal only: preserve separate unknown models while finding the winner.
pub(crate) struct ModelDay {
    pub provider: CodingProvider,
    pub day: Date,
    pub key: String,
    pub display: Option<String>,
    pub tokens: u64,
}

pub(crate) fn load(
    connection: &Connection,
    now: OffsetDateTime,
    providers: &[ProviderPresentation],
    enabled: &BTreeSet<CodingProvider>,
) -> Result<UsageHistory, ()> {
    let today = now.to_offset(UtcOffset::UTC).date();
    let daily = providers::load_daily_usage_history(connection, now, today, 30)?;
    let detail = providers::load_usage_history_detail(connection, today)?;
    let mut history = project(today, providers, enabled, &daily, &detail.models);
    project_hours(&mut history, now, providers, &detail.hours);
    Ok(history)
}

fn project(
    today: Date,
    providers: &[ProviderPresentation],
    enabled: &BTreeSet<CodingProvider>,
    daily: &[ProviderDailyUsage],
    models: &[ModelDay],
) -> UsageHistory {
    let visible: Vec<_> = providers
        .iter()
        .filter(|p| enabled.contains(&p.provider) && p.is_visible())
        .collect();
    let mut scopes = Vec::new();
    for provider in std::iter::once(None).chain(visible.iter().map(|p| Some(p.provider))) {
        let included: Vec<_> = visible
            .iter()
            .copied()
            .filter(|p| provider.is_none_or(|id| id == p.provider))
            .collect();
        let top = |start: Date, end: Date| {
            providers::select_top_model_usage(
                models
                    .iter()
                    .filter(|m| {
                        m.day >= start
                            && m.day <= end
                            && included.iter().any(|p| p.provider == m.provider)
                    })
                    .filter(|m| {
                        if m.day == today {
                            included.iter().any(|p| {
                                p.provider == m.provider
                                    && !matches!(p.usage.today, UsageTotal::Unavailable)
                            })
                        } else {
                            daily.iter().any(|d| {
                                d.provider == m.provider
                                    && d.day == m.day
                                    && !matches!(d.total, UsageTotal::Unavailable)
                            })
                        }
                    })
                    .map(|m| {
                        (
                            format!("{:?}:{}", m.provider, m.key),
                            m.display.clone(),
                            m.tokens,
                        )
                    }),
            )
        };
        let days = (0..30)
            .rev()
            .map(|ago| {
                let day = today - Duration::days(ago);
                let totals: Vec<_> = included
                    .iter()
                    .map(|p| {
                        // Use the published current-day total, including retained evidence.
                        if day == today {
                            &p.usage.today
                        } else {
                            daily
                                .iter()
                                .find(|d| d.day == day && d.provider == p.provider)
                                .map_or(&UsageTotal::Unavailable, |d| &d.total)
                        }
                    })
                    .collect();
                UsageHistoryDay {
                    day: day.to_string(),
                    total: combine_total(&totals),
                    top_model_usage: top(day, day),
                }
            })
            .collect();
        scopes.push(UsageHistoryScope {
            provider,
            days,
            hours: Vec::new(),
            hourly_matches_total: false,
            top_models: UsageHistoryModels {
                today: top(today, today),
                seven_days: top(today - Duration::days(6), today),
                thirty_days: top(today - Duration::days(29), today),
            },
        });
    }
    UsageHistory {
        today: today.to_string(),
        scopes,
    }
}

pub(crate) fn validate(history: &UsageHistory) -> Result<(), &'static str> {
    let parse_day = |day: &str| {
        Date::parse(
            day,
            &time::macros::format_description!("[year]-[month]-[day]"),
        )
        .map_err(|_| "native history unavailable")
    };
    let today = parse_day(&history.today)?;
    if history.scopes.len() > 3 {
        return Err("native history unavailable");
    }
    let mut seen = BTreeSet::new();
    for scope in &history.scopes {
        if !seen.insert(scope.provider) || scope.days.len() != 30 {
            return Err("native history unavailable");
        }
        if scope.hours.len() > 24 {
            return Err("native history unavailable");
        }
        for (index, hour) in scope.hours.iter().enumerate() {
            if hour.hour as usize != index {
                return Err("native history unavailable");
            }
            crate::sanitized::validate_usage_total(&hour.total)?;
            crate::sanitized::validate_top_model_usage(hour.top_model_usage.as_ref())?;
        }
        for (index, day) in scope.days.iter().enumerate() {
            if parse_day(&day.day)? != today - Duration::days(29 - index as i64) {
                return Err("native history unavailable");
            }
            crate::sanitized::validate_usage_total(&day.total)?;
            crate::sanitized::validate_top_model_usage(day.top_model_usage.as_ref())?;
        }
        for model in [
            &scope.top_models.today,
            &scope.top_models.seven_days,
            &scope.top_models.thirty_days,
        ] {
            crate::sanitized::validate_top_model_usage(model.as_ref())?;
        }
    }
    Ok(())
}

fn observed_tokens(total: &UsageTotal) -> Option<u64> {
    match total {
        UsageTotal::Current {
            observed_tokens, ..
        }
        | UsageTotal::Stale {
            observed_tokens, ..
        } => Some(*observed_tokens),
        UsageTotal::Unavailable => None,
    }
}

fn project_hours(
    history: &mut UsageHistory,
    now: OffsetDateTime,
    providers: &[ProviderPresentation],
    rows: &[HourPart],
) {
    use crate::sanitized::{
        ApiEquivalentCostQuality, UsageCoverage, UsageEvidenceBasis, UsageScanStatus,
    };
    let now = now.to_offset(UtcOffset::UTC);
    let visible: BTreeSet<_> = history.scopes.iter().filter_map(|s| s.provider).collect();
    for scope in &mut history.scopes {
        let included: Vec<_> = providers
            .iter()
            .filter(|p| {
                visible.contains(&p.provider) && scope.provider.is_none_or(|id| id == p.provider)
            })
            .collect();
        let selected: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.hour <= now.hour()
                    && r.observed_at <= now
                    && included.iter().any(|p| {
                        p.provider == r.provider && observed_tokens(&p.usage.today).is_some()
                    })
            })
            .collect();
        let all_tokens = selected
            .iter()
            .try_fold(0u64, |sum, r| sum.checked_add(r.tokens));
        let daily_tokens = scope.days.last().and_then(|d| observed_tokens(&d.total));
        scope.hourly_matches_total = daily_tokens.is_some() && all_tokens == daily_tokens;
        // Absence proves zero only after the local scan is complete and its hourly total reconciles.
        let zero_known = scope.hourly_matches_total
            && included
                .iter()
                .all(|p| p.usage.today_scan_status == UsageScanStatus::Complete);
        scope.hours = (0..=now.hour())
            .map(|hour| {
                let parts: Vec<_> = selected
                    .iter()
                    .copied()
                    .filter(|r| r.hour == hour)
                    .collect();
                let top_model_usage = providers::select_top_model_usage(parts.iter().map(|r| {
                    (
                        format!("{:?}:{}", r.provider, r.key),
                        r.display.clone(),
                        r.tokens,
                    )
                }));
                let tokens = parts
                    .iter()
                    .try_fold(0u64, |sum, r| sum.checked_add(r.tokens));
                let priced = parts
                    .iter()
                    .try_fold(0u64, |sum, r| sum.checked_add(r.priced_tokens));
                let total = match (tokens, priced) {
                    (Some(observed_tokens), Some(priced)) if !parts.is_empty() || zero_known => {
                        let modeled = priced < observed_tokens || parts.iter().any(|r| r.modeled);
                        let has_price = parts.iter().any(|r| r.cost.is_some())
                            || (zero_known && observed_tokens == 0);
                        let cost = parts.iter().filter_map(|r| r.cost).sum::<f64>();
                        let basis = parts
                            .iter()
                            .filter_map(|r| r.basis.as_deref())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(" + ");
                        let basis = if basis.is_empty() && zero_known {
                            included
                                .iter()
                                .filter_map(|p| providers::current_pricing_basis(p.provider))
                                .collect::<BTreeSet<_>>()
                                .into_iter()
                                .collect::<Vec<_>>()
                                .join(" + ")
                        } else {
                            basis
                        };
                        let has_price = has_price && !basis.is_empty();
                        let latest = parts
                            .iter()
                            .map(|r| r.observed_at)
                            .max()
                            .or_else(|| {
                                included
                                    .iter()
                                    .filter_map(|p| match &p.usage.today {
                                        UsageTotal::Current { observed_at, .. }
                                        | UsageTotal::Stale { observed_at, .. } => {
                                            OffsetDateTime::parse(
                                                observed_at,
                                                &time::format_description::well_known::Rfc3339,
                                            )
                                            .ok()
                                        }
                                        UsageTotal::Unavailable => None,
                                    })
                                    .max()
                            })
                            .unwrap_or(now.date().midnight().assume_utc());
                        let total = UsageTotal::Current {
                            evidence_basis: UsageEvidenceBasis::LocallyDerived,
                            coverage: if zero_known && parts.iter().all(|r| r.complete) {
                                UsageCoverage::Complete
                            } else {
                                UsageCoverage::Partial
                            },
                            observed_at: latest
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                            observed_tokens,
                            api_equivalent_cost_usd: (has_price && cost.is_finite())
                                .then_some(cost),
                            api_equivalent_cost_basis: has_price.then_some(basis),
                            api_equivalent_cost_quality: has_price.then_some(if modeled {
                                ApiEquivalentCostQuality::Modeled
                            } else {
                                ApiEquivalentCostQuality::LocalOnly
                            }),
                            api_equivalent_cost_coverage_percent: (has_price && modeled).then_some(
                                if observed_tokens == 0 {
                                    100.0
                                } else {
                                    (priced as f64 / observed_tokens as f64 * 100.0)
                                        .clamp(0.0, 100.0)
                                },
                            ),
                            trend_percent: None,
                            trend_previous_tokens: None,
                        };
                        total.transition_at(now).0
                    }
                    _ => UsageTotal::Unavailable,
                };
                UsageHistoryHour {
                    hour,
                    total,
                    top_model_usage,
                }
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitized::UsageScanStatus;
    use time::macros::datetime;

    fn total(tokens: u64) -> UsageTotal {
        serde_json::from_value(serde_json::json!({"availability":"current","evidenceBasis":"locally-derived","coverage":"complete","observedAt":"2026-09-11T12:00:00Z","observedTokens":tokens,"apiEquivalentCostUsd":null})).unwrap()
    }
    fn provider(id: CodingProvider, tokens: u64) -> ProviderPresentation {
        let mut p = ProviderPresentation::unavailable(id);
        p.usage.today = total(tokens);
        p.usage.today_scan_status = UsageScanStatus::Complete;
        p
    }
    fn part(provider: CodingProvider, hour: u8, tokens: u64) -> HourPart {
        HourPart {
            provider,
            hour,
            tokens,
            key: "known".into(),
            display: Some("Model".into()),
            cost: Some(tokens as f64 / 100.0),
            priced_tokens: tokens,
            modeled: false,
            complete: true,
            observed_at: datetime!(2026-09-11 12:00 UTC),
            basis: providers::current_pricing_basis(provider).map(str::to_owned),
        }
    }
    #[test]
    fn hourly_projection_keeps_missing_detail_unknown_and_reconciles_known_zeros() {
        let now = datetime!(2026-09-11 12:00 UTC);
        let providers = vec![
            provider(CodingProvider::Codex, 200),
            provider(CodingProvider::Claude, 100),
        ];
        let enabled = BTreeSet::from([CodingProvider::Codex, CodingProvider::Claude]);
        let mut history = project(now.date(), &providers, &enabled, &[], &[]);
        let mut rows = vec![
            part(CodingProvider::Codex, 9, 150),
            part(CodingProvider::Claude, 10, 100),
        ];
        project_hours(&mut history, now, &providers, &rows);
        assert_eq!(history.scopes[0].hours.len(), 13);
        assert!(!history.scopes[0].hourly_matches_total);
        assert!(matches!(
            history.scopes[0].hours[0].total,
            UsageTotal::Unavailable
        ));
        assert_eq!(
            observed_tokens(&history.scopes[0].hours[9].total),
            Some(150)
        );
        rows.push(part(CodingProvider::Codex, 10, 50));
        project_hours(&mut history, now, &providers, &rows);
        assert!(history.scopes[0].hourly_matches_total);
        assert_eq!(observed_tokens(&history.scopes[0].hours[0].total), Some(0));
        assert_eq!(
            observed_tokens(&history.scopes[0].hours[10].total),
            Some(150)
        );
        validate(&history).unwrap();
        let previous = history.clone();
        project_hours(&mut history, now + Duration::seconds(1), &providers, &rows);
        assert_eq!(
            previous, history,
            "status-only commits must not change zero-bucket timestamps"
        );
        let serialized = serde_json::to_string(&history).unwrap();
        assert!(!serialized.contains("known"));
        assert!(!serialized.contains("path"));
    }
    #[test]
    fn model_winners_use_the_full_period_and_disabled_providers_are_excluded() {
        let now = datetime!(2026-09-11 12:00 UTC);
        let providers = vec![
            provider(CodingProvider::Codex, 200),
            provider(CodingProvider::Claude, 1000),
        ];
        let enabled = BTreeSet::from([CodingProvider::Codex]);
        let daily = vec![ProviderDailyUsage {
            provider: CodingProvider::Codex,
            day: now.date() - Duration::days(1),
            total: total(300),
            correction: None,
        }];
        let models = vec![
            ModelDay {
                provider: CodingProvider::Codex,
                day: now.date(),
                key: "new".into(),
                display: Some("New model".into()),
                tokens: 200,
            },
            ModelDay {
                provider: CodingProvider::Codex,
                day: now.date() - Duration::days(1),
                key: "old".into(),
                display: Some("Old model".into()),
                tokens: 300,
            },
            ModelDay {
                provider: CodingProvider::Claude,
                day: now.date(),
                key: "private unrecognized model".into(),
                display: None,
                tokens: 1000,
            },
        ];
        let history = project(now.date(), &providers, &enabled, &daily, &models);
        assert_eq!(history.scopes.len(), 2);
        let combined = &history.scopes[0];
        assert_eq!(
            combined.top_models.today.as_ref().unwrap().model.as_deref(),
            Some("New model")
        );
        assert_eq!(
            combined
                .top_models
                .seven_days
                .as_ref()
                .unwrap()
                .model
                .as_deref(),
            Some("Old model")
        );
        assert_eq!(observed_tokens(&combined.days[29].total), Some(200));
        assert!(!serde_json::to_string(&history).unwrap().contains("private"));
    }
    #[test]
    fn hourly_projection_excludes_future_records_and_unknown_models_stay_unknown() {
        let now = datetime!(2026-09-11 12:00 UTC);
        let providers = vec![provider(CodingProvider::Codex, 200)];
        let mut history = project(
            now.date(),
            &providers,
            &BTreeSet::from([CodingProvider::Codex]),
            &[],
            &[],
        );
        let mut unknown = part(CodingProvider::Codex, 11, 100);
        unknown.display = None;
        unknown.key = "private model".into();
        unknown.cost = None;
        unknown.priced_tokens = 0;
        unknown.basis = None;
        project_hours(
            &mut history,
            now,
            &providers,
            &[unknown, part(CodingProvider::Codex, 13, 100)],
        );
        assert_eq!(
            history.scopes[0].hours[11]
                .top_model_usage
                .as_ref()
                .unwrap()
                .model,
            None
        );
        assert_eq!(history.scopes[0].hours.len(), 13);
        assert!(!history.scopes[0].hourly_matches_total);
        validate(&history).unwrap();
    }
}
