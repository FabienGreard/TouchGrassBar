use crate::sanitized::ProviderSnapshot;
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeadroomFreshness {
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeadroomCompleteness {
    Complete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OverallQuotaHeadroom {
    Unavailable,
    Calculated {
        remaining_percent: f64,
        freshness: HeadroomFreshness,
        completeness: HeadroomCompleteness,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RevisionedOverallQuotaHeadroom {
    pub(crate) revision: u64,
    pub(crate) headroom: OverallQuotaHeadroom,
}

struct ProviderQuotaHeadroom {
    remaining_percent: f64,
    freshness: HeadroomFreshness,
}

pub(crate) fn overall_quota_headroom<'a>(
    enabled_provider_snapshots: impl IntoIterator<Item = &'a ProviderSnapshot>,
    now: OffsetDateTime,
) -> OverallQuotaHeadroom {
    let mut calculated_sum = 0.0;
    let mut calculated_count = 0_u32;
    let mut stale = false;
    let mut incomplete = false;

    for snapshot in enabled_provider_snapshots {
        let Some(provider_headroom) = provider_headroom(snapshot, now) else {
            incomplete = true;
            continue;
        };
        calculated_sum += provider_headroom.remaining_percent;
        calculated_count += 1;
        stale |= provider_headroom.freshness == HeadroomFreshness::Stale;
    }

    if calculated_count == 0 {
        return OverallQuotaHeadroom::Unavailable;
    }

    OverallQuotaHeadroom::Calculated {
        remaining_percent: calculated_sum / f64::from(calculated_count),
        freshness: if stale {
            HeadroomFreshness::Stale
        } else {
            HeadroomFreshness::Current
        },
        completeness: if incomplete {
            HeadroomCompleteness::Incomplete
        } else {
            HeadroomCompleteness::Complete
        },
    }
}

fn provider_headroom(
    snapshot: &ProviderSnapshot,
    now: OffsetDateTime,
) -> Option<ProviderQuotaHeadroom> {
    let (quota_lanes, freshness) = match snapshot {
        ProviderSnapshot::Unavailable { .. } => return None,
        ProviderSnapshot::Current { quota_lanes, .. } => (quota_lanes, HeadroomFreshness::Current),
        ProviderSnapshot::Stale { quota_lanes, .. } => (quota_lanes, HeadroomFreshness::Stale),
    };
    if quota_lanes.is_empty() {
        return None;
    }

    let mut lowest = 100.0_f64;
    let mut has_active_lane = false;
    for lane in quota_lanes.iter().filter(|lane| lane.is_active_at(now)) {
        has_active_lane = true;
        let allowance = lane
            .allowance
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let remaining = lane.remaining.filter(|value| value.is_finite())?;
        let percentage = ((remaining / allowance) * 100.0).clamp(0.0, 100.0);
        lowest = lowest.min(percentage);
    }

    has_active_lane.then_some(ProviderQuotaHeadroom {
        remaining_percent: if lowest == 0.0 { 0.0 } else { lowest },
        freshness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::CodingProvider,
        sanitized::{ProviderSnapshot, QuotaLane},
    };

    fn lane(allowance: Option<f64>, remaining: Option<f64>) -> QuotaLane {
        QuotaLane {
            label: "Fixture limit".to_owned(),
            unit: "percent".to_owned(),
            allowance,
            remaining,
            reset_at: None,
        }
    }

    fn current(provider: CodingProvider, lanes: Vec<QuotaLane>) -> ProviderSnapshot {
        ProviderSnapshot::Current {
            provider,
            observed_at: "2026-08-08T12:00:00Z".to_owned(),
            quota_lanes: lanes,
        }
    }

    fn stale(provider: CodingProvider, lanes: Vec<QuotaLane>) -> ProviderSnapshot {
        ProviderSnapshot::Stale {
            provider,
            observed_at: "2026-08-08T11:50:00Z".to_owned(),
            quota_lanes: lanes,
        }
    }

    fn unavailable(provider: CodingProvider) -> ProviderSnapshot {
        ProviderSnapshot::Unavailable {
            provider,
            quota_lanes: [],
        }
    }

    fn calculated(
        headroom: OverallQuotaHeadroom,
    ) -> (f64, HeadroomFreshness, HeadroomCompleteness) {
        let OverallQuotaHeadroom::Calculated {
            remaining_percent,
            freshness,
            completeness,
        } = headroom
        else {
            panic!("headroom must be calculable");
        };
        (remaining_percent, freshness, completeness)
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    #[test]
    fn codex_eight_and_claude_sixty_produce_thirty_four_percent() {
        let codex = current(CodingProvider::Codex, vec![lane(Some(100.0), Some(8.0))]);
        let claude = current(CodingProvider::Claude, vec![lane(Some(100.0), Some(60.0))]);

        assert_eq!(
            calculated(overall_quota_headroom([&codex, &claude], now())),
            (
                34.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            )
        );
    }

    #[test]
    fn one_unavailable_provider_is_excluded_and_makes_the_result_incomplete() {
        let codex = current(CodingProvider::Codex, vec![lane(Some(100.0), Some(8.0))]);
        let claude = unavailable(CodingProvider::Claude);

        assert_eq!(
            calculated(overall_quota_headroom([&codex, &claude], now())),
            (
                8.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Incomplete,
            )
        );
    }

    #[test]
    fn no_calculable_provider_is_unavailable() {
        let codex = unavailable(CodingProvider::Codex);
        let claude = unavailable(CodingProvider::Claude);

        assert_eq!(
            overall_quota_headroom([&codex, &claude], now()),
            OverallQuotaHeadroom::Unavailable
        );
        assert_eq!(
            overall_quota_headroom(std::iter::empty(), now()),
            OverallQuotaHeadroom::Unavailable
        );
    }

    #[test]
    fn calculated_zero_is_not_unavailable() {
        let codex = current(CodingProvider::Codex, vec![lane(Some(100.0), Some(0.0))]);

        assert_eq!(
            calculated(overall_quota_headroom([&codex], now())),
            (
                0.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            )
        );
    }

    #[test]
    fn a_still_valid_stale_contributor_remains_in_the_mean() {
        let codex = stale(CodingProvider::Codex, vec![lane(Some(100.0), Some(8.0))]);
        let claude = current(CodingProvider::Claude, vec![lane(Some(100.0), Some(60.0))]);

        assert_eq!(
            calculated(overall_quota_headroom([&codex, &claude], now())),
            (
                34.0,
                HeadroomFreshness::Stale,
                HeadroomCompleteness::Complete,
            )
        );
    }

    #[test]
    fn absent_lanes_are_ignored_and_the_lowest_present_lane_wins() {
        let codex = current(
            CodingProvider::Codex,
            vec![
                lane(Some(100.0), Some(81.0)),
                lane(Some(100.0), Some(8.125)),
            ],
        );
        let claude = current(CodingProvider::Claude, vec![lane(Some(100.0), Some(60.25))]);

        assert_eq!(
            calculated(overall_quota_headroom([&codex, &claude], now())).0,
            34.1875
        );
    }

    #[test]
    fn one_active_unknown_lane_makes_its_provider_unavailable() {
        for unknown in [
            lane(None, Some(20.0)),
            lane(Some(100.0), None),
            lane(Some(0.0), Some(0.0)),
            lane(Some(f64::INFINITY), Some(20.0)),
            lane(Some(100.0), Some(f64::NAN)),
        ] {
            let codex = current(
                CodingProvider::Codex,
                vec![lane(Some(100.0), Some(80.0)), unknown],
            );

            assert_eq!(
                overall_quota_headroom([&codex], now()),
                OverallQuotaHeadroom::Unavailable
            );
        }
    }

    #[test]
    fn lane_percentages_are_clamped_before_provider_and_overall_reduction() {
        let codex = current(CodingProvider::Codex, vec![lane(Some(100.0), Some(-10.0))]);
        let claude = current(CodingProvider::Claude, vec![lane(Some(100.0), Some(140.0))]);

        assert_eq!(
            calculated(overall_quota_headroom([&codex, &claude], now())).0,
            50.0
        );
    }
}
