//! Claude quota observation through the installed Claude Code CLI.
//!
//! The provider runs `/usage` in a private terminal, reduces the response to
//! the two supported quota lanes, and discards the terminal output.

mod cli_probe;
mod pricing;
mod usage;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration as StdDuration,
};

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::registry::resolve_provider_executable;
use super::{ProviderObservation, ProviderObservationAdapter};
use crate::daily_usage_aggregate::preserve_best_known_costs;
use crate::providers::process::ProviderProcessSupervisor;
use crate::sanitized::{
    Clock, CodingProvider, ProviderPresentation, ProviderSnapshot, QuotaLane, RefreshAttempt,
    RefreshFailure, TopModelUsage, UsagePeriods,
};

#[cfg(debug_assertions)]
fn debug_event(event: &str) {
    eprintln!("[TouchGrassBar][claude-quota] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_event(_event: &str) {}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClaudeRateLimitWindow {
    resets_at: i64,
    used_percentage: f64,
}

impl ClaudeRateLimitWindow {
    fn validate(self, now: OffsetDateTime) -> Result<Self, ()> {
        let reset_at = OffsetDateTime::from_unix_timestamp(self.resets_at).map_err(|_| ())?;
        (self.used_percentage.is_finite()
            && (0.0..=100.0).contains(&self.used_percentage)
            && reset_at > now)
            .then_some(self)
            .ok_or(())
    }

    fn sanitized_lane(self, label: &str) -> Result<QuotaLane, ()> {
        let reset_at = OffsetDateTime::from_unix_timestamp(self.resets_at).map_err(|_| ())?;
        Ok(QuotaLane {
            label: label.to_owned(),
            unit: "percent".to_owned(),
            allowance: Some(100.0),
            remaining: Some(100.0 - self.used_percentage),
            reset_at: Some(reset_at.format(&Rfc3339).map_err(|_| ())?),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ClaudeQuotaObservation {
    observed_at: OffsetDateTime,
    five_hour: ClaudeRateLimitWindow,
    seven_day: ClaudeRateLimitWindow,
}

impl ClaudeQuotaObservation {
    fn sanitized_snapshot(&self, now: OffsetDateTime) -> Result<ProviderSnapshot, ()> {
        if self.observed_at > now {
            return Err(());
        }
        let five_hour = self.five_hour.validate(now)?;
        let seven_day = self.seven_day.validate(now)?;
        Ok(ProviderSnapshot::Current {
            provider: CodingProvider::Claude,
            observed_at: self.observed_at.format(&Rfc3339).map_err(|_| ())?,
            quota_lanes: vec![
                five_hour.sanitized_lane("5-hour limit")?,
                seven_day.sanitized_lane("Weekly limit")?,
            ],
        })
    }
}

pub(crate) struct ClaudeProviderObservationAdapter {
    clock: Arc<dyn Clock>,
    database_path: Option<PathBuf>,
    probe_directory: Option<PathBuf>,
    processes: ProviderProcessSupervisor,
    #[cfg(test)]
    fixture: Option<ClaudeQuotaObservation>,
    #[cfg(test)]
    fixture_failure: Option<RefreshFailure>,
    #[cfg(test)]
    fixture_usage: Option<crate::sanitized::UsagePeriods>,
}

impl ClaudeProviderObservationAdapter {
    pub(crate) fn production(
        clock: Arc<dyn Clock>,
        database_path: Option<PathBuf>,
        processes: ProviderProcessSupervisor,
    ) -> Self {
        Self {
            clock,
            database_path: database_path.clone(),
            probe_directory: database_path
                .as_deref()
                .and_then(Path::parent)
                .map(|parent| parent.join("claude-quota-probe")),
            processes,
            #[cfg(test)]
            fixture: None,
            #[cfg(test)]
            fixture_failure: None,
            #[cfg(test)]
            fixture_usage: None,
        }
    }

    #[cfg(test)]
    pub(super) fn fixture(
        clock: Arc<dyn Clock>,
        observation: ClaudeQuotaObservation,
        processes: ProviderProcessSupervisor,
    ) -> Self {
        Self {
            clock,
            database_path: None,
            probe_directory: None,
            processes,
            fixture: Some(observation),
            fixture_failure: None,
            fixture_usage: None,
        }
    }

    #[cfg(test)]
    fn fixture_with_usage(
        clock: Arc<dyn Clock>,
        observation: Result<ClaudeQuotaObservation, RefreshFailure>,
        fixture_usage: crate::sanitized::UsagePeriods,
    ) -> Self {
        let (fixture, fixture_failure) = match observation {
            Ok(observation) => (Some(observation), None),
            Err(failure) => (None, Some(failure)),
        };
        Self {
            clock,
            database_path: None,
            probe_directory: None,
            processes: ProviderProcessSupervisor::default(),
            fixture,
            fixture_failure,
            fixture_usage: Some(fixture_usage),
        }
    }

    fn observe(
        &self,
        now: OffsetDateTime,
        timeout: StdDuration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ClaudeQuotaObservation, RefreshFailure> {
        #[cfg(test)]
        if let Some(failure) = self.fixture_failure {
            return Err(failure);
        }

        #[cfg(test)]
        if let Some(observation) = &self.fixture {
            return Ok(observation.clone());
        }

        let probe_directory = self.probe_directory.as_deref().ok_or_else(|| {
            debug_event("cli_probe_unavailable reason=storage_unavailable");
            RefreshFailure::SourceUnavailable
        })?;
        let executable = resolve_provider_executable(CodingProvider::Claude).ok_or_else(|| {
            debug_event("cli_probe_unavailable reason=executable_unavailable");
            RefreshFailure::SourceUnavailable
        })?;
        cli_probe::probe_usage(
            &self.processes,
            &executable,
            probe_directory,
            now,
            timeout,
            cancelled,
        )
        .map_err(|failure| match failure {
            cli_probe::ProbeFailure::Cancelled => RefreshFailure::Cancelled,
            cli_probe::ProbeFailure::Unavailable => {
                debug_event("cli_probe_unavailable reason=probe_failed");
                RefreshFailure::SourceUnavailable
            }
        })
    }

    fn observe_usage(&self, now: OffsetDateTime) -> Option<(UsagePeriods, Option<TopModelUsage>)> {
        #[cfg(test)]
        if let Some(usage) = &self.fixture_usage {
            return Some((usage.clone(), None));
        }

        usage::scan_local_usage(
            self.database_path.as_deref(),
            self.probe_directory.as_deref(),
            now,
        )
        .as_ref()
        .map(|local| {
            (
                usage::project_usage_periods(Some(local), now),
                local.top_model_usage.clone(),
            )
        })
    }
}

impl ProviderObservationAdapter for ClaudeProviderObservationAdapter {
    fn provider(&self) -> CodingProvider {
        CodingProvider::Claude
    }

    fn refresh(
        &self,
        cached: &ProviderPresentation,
        attempt: &RefreshAttempt,
    ) -> Result<Option<ProviderObservation>, RefreshFailure> {
        let skip_quota = attempt.should_skip_claude_quota_probe();
        let scan_usage = !skip_quota || attempt.includes_local_usage_catch_up();
        if skip_quota && !scan_usage {
            debug_event("refresh_skipped reason=claude_irrelevant_sources");
            return Ok(None);
        }
        let now = self.clock.now();
        let projected_usage = if scan_usage {
            attempt.remaining()?;
            self.observe_usage(now)
        } else {
            None
        };
        let (usage, top_model_usage) = projected_usage
            .map(|(usage, top_model_usage)| {
                (
                    preserve_best_known_costs(usage, &cached.usage),
                    top_model_usage,
                )
            })
            .unwrap_or_else(|| (cached.usage.clone(), cached.top_model_usage.clone()));
        let mut quota_failed = false;
        let quota = if skip_quota {
            cached.quota.clone()
        } else {
            let timeout = attempt.remaining()?;
            debug_event("cli_probe_started");
            match self.observe(now, timeout, &|| attempt.is_cancelled()) {
                Ok(observation) => match observation.sanitized_snapshot(now) {
                    Ok(snapshot) => snapshot,
                    Err(()) => {
                        debug_event("cli_probe_unavailable reason=invalid_quota");
                        quota_failed = true;
                        cached.quota.clone()
                    }
                },
                Err(RefreshFailure::SourceUnavailable) => {
                    quota_failed = true;
                    cached.quota.clone()
                }
                Err(failure) => return Err(failure),
            }
        };
        if cached.quota == quota
            && cached.usage == usage
            && cached.top_model_usage == top_model_usage
        {
            if quota_failed {
                return Err(RefreshFailure::SourceUnavailable);
            }
            debug_event("refresh_unchanged");
            return Ok(None);
        }
        debug_event("refresh_loaded");
        Ok(Some(ProviderObservation {
            quota,
            usage,
            top_model_usage,
        }))
    }
}

pub(super) fn debug_live_quota_report(
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    let executable = resolve_provider_executable(CodingProvider::Claude).ok_or(())?;
    let processes = ProviderProcessSupervisor::default();
    let observation = cli_probe::probe_usage(
        &processes,
        &executable,
        probe_directory,
        now,
        StdDuration::from_secs(30),
        &|| false,
    )
    .map_err(|_| ())?;
    format_debug_quota_report(&observation, now)
}

pub(super) fn debug_usage_report(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    usage::debug_usage_report(database_path, config_root, probe_directory, now)
}

fn format_debug_quota_report(
    observation: &ClaudeQuotaObservation,
    now: OffsetDateTime,
) -> Result<String, ()> {
    let ProviderSnapshot::Current { quota_lanes, .. } = observation.sanitized_snapshot(now)? else {
        return Err(());
    };
    let mut report = format!(
        "[TouchGrassBar][claude-quota-report] availability=current observed_age_seconds={} lane_count={}",
        (now - observation.observed_at).whole_seconds().max(0),
        quota_lanes.len()
    );
    for (index, lane) in quota_lanes.iter().enumerate() {
        let lane_name = match index {
            0 => "five_hour",
            1 => "seven_day",
            _ => return Err(()),
        };
        let remaining = lane.remaining.ok_or(())?;
        let reset_at = lane.reset_at.as_deref().ok_or(())?;
        let reset_at = OffsetDateTime::parse(reset_at, &Rfc3339).map_err(|_| ())?;
        report.push_str(&format!(
            "\n[TouchGrassBar][claude-quota-report] lane={lane_name} remaining_percent={remaining:.2} reset_in_seconds={}",
            (reset_at - now).whole_seconds().max(0)
        ));
    }
    Ok(report)
}

#[cfg(test)]
pub(super) fn fixture_observation(now: OffsetDateTime) -> ClaudeQuotaObservation {
    ClaudeQuotaObservation {
        observed_at: now,
        five_hour: ClaudeRateLimitWindow {
            resets_at: (now + time::Duration::hours(2)).unix_timestamp(),
            used_percentage: 23.5,
        },
        seven_day: ClaudeRateLimitWindow {
            resets_at: (now + time::Duration::days(3)).unix_timestamp(),
            used_percentage: 41.25,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitized::{
        ApiEquivalentCostQuality, UsageCoverage, UsageEvidenceBasis, UsagePeriods, UsageScanStatus,
        UsageTotal,
    };

    struct FixedClock(OffsetDateTime);

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    fn fixture_usage(now: OffsetDateTime, observed_tokens: u64) -> UsagePeriods {
        let total = UsageTotal::Current {
            evidence_basis: UsageEvidenceBasis::LocallyDerived,
            coverage: UsageCoverage::Partial,
            observed_at: now.format(&Rfc3339).unwrap(),
            observed_tokens,
            api_equivalent_cost_usd: Some(1.25),
            trend_percent: None,
            trend_previous_tokens: None,
            api_equivalent_cost_basis: Some("anthropic-standard-test".to_owned()),
            api_equivalent_cost_quality: Some(ApiEquivalentCostQuality::LocalOnly),
            api_equivalent_cost_coverage_percent: None,
        };
        UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: total.clone(),
            seven_days: total.clone(),
            thirty_days: total,
        }
    }

    #[test]
    fn codex_provider_notification_does_not_refresh_claude() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let adapter = ClaudeProviderObservationAdapter::fixture(
            Arc::new(FixedClock(now)),
            fixture_observation(now),
            ProviderProcessSupervisor::default(),
        );
        let cached = ProviderPresentation::unavailable(CodingProvider::Claude);

        assert_eq!(
            adapter
                .refresh(&cached, &RefreshAttempt::test_provider_notification())
                .unwrap(),
            None
        );
    }

    #[test]
    fn joined_codex_notification_and_local_usage_refresh_only_claude_usage() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let expected_usage = fixture_usage(now, 321);
        let adapter = ClaudeProviderObservationAdapter::fixture_with_usage(
            Arc::new(FixedClock(now)),
            Ok(fixture_observation(now)),
            expected_usage.clone(),
        );
        let cached = ProviderPresentation::unavailable(CodingProvider::Claude);

        let observation = adapter
            .refresh(
                &cached,
                &RefreshAttempt::test_provider_notification_with_local_usage(),
            )
            .unwrap()
            .expect("local usage catch-up must publish Claude usage");

        assert_eq!(observation.quota, cached.quota);
        assert_eq!(observation.usage, expected_usage);
    }

    #[test]
    fn incomplete_usage_refresh_keeps_the_last_valid_cost() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let mut cached = ProviderPresentation::unavailable(CodingProvider::Claude);
        cached.usage = fixture_usage(now, 321);
        let mut incomplete = fixture_usage(now, 321);
        incomplete.scan_status = UsageScanStatus::Indexing;
        incomplete.today_scan_status = UsageScanStatus::Indexing;
        incomplete.seven_day_scan_status = UsageScanStatus::Indexing;
        incomplete.thirty_day_scan_status = UsageScanStatus::Indexing;
        for total in [
            &mut incomplete.today,
            &mut incomplete.seven_days,
            &mut incomplete.thirty_days,
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = total
            else {
                panic!("fixture usage must be current");
            };
            *api_equivalent_cost_usd = None;
            *api_equivalent_cost_basis = None;
            *api_equivalent_cost_quality = None;
            *api_equivalent_cost_coverage_percent = None;
        }
        let adapter = ClaudeProviderObservationAdapter::fixture_with_usage(
            Arc::new(FixedClock(now)),
            Ok(fixture_observation(now)),
            incomplete,
        );

        let observation = adapter
            .refresh(&cached, &RefreshAttempt::test())
            .unwrap()
            .expect("incomplete Claude evidence must publish");

        for total in [
            observation.usage.today,
            observation.usage.seven_days,
            observation.usage.thirty_days,
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                ..
            } = total
            else {
                panic!("fixture usage must remain current");
            };
            assert_eq!(api_equivalent_cost_usd, Some(1.25));
        }
    }

    #[test]
    fn incomplete_usage_refresh_models_a_carried_cost_when_tokens_change() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let mut cached = ProviderPresentation::unavailable(CodingProvider::Claude);
        cached.usage = fixture_usage(now, 321);
        let mut incomplete = fixture_usage(now, 642);
        incomplete.scan_status = UsageScanStatus::Indexing;
        incomplete.today_scan_status = UsageScanStatus::Indexing;
        incomplete.seven_day_scan_status = UsageScanStatus::Indexing;
        incomplete.thirty_day_scan_status = UsageScanStatus::Indexing;
        for total in [
            &mut incomplete.today,
            &mut incomplete.seven_days,
            &mut incomplete.thirty_days,
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_basis,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = total
            else {
                panic!("fixture usage must be current");
            };
            *api_equivalent_cost_usd = None;
            *api_equivalent_cost_basis = None;
            *api_equivalent_cost_quality = None;
            *api_equivalent_cost_coverage_percent = None;
        }
        let adapter = ClaudeProviderObservationAdapter::fixture_with_usage(
            Arc::new(FixedClock(now)),
            Ok(fixture_observation(now)),
            incomplete,
        );

        let observation = adapter
            .refresh(&cached, &RefreshAttempt::test())
            .unwrap()
            .expect("incomplete Claude evidence must publish");

        for total in [
            observation.usage.today,
            observation.usage.seven_days,
            observation.usage.thirty_days,
        ] {
            let UsageTotal::Current {
                api_equivalent_cost_usd,
                api_equivalent_cost_quality,
                api_equivalent_cost_coverage_percent,
                ..
            } = total
            else {
                panic!("fixture usage must remain current");
            };
            assert_eq!(api_equivalent_cost_usd, Some(2.5));
            assert_eq!(
                api_equivalent_cost_quality,
                Some(ApiEquivalentCostQuality::Modeled)
            );
            assert_eq!(api_equivalent_cost_coverage_percent, Some(50.0));
        }
    }

    #[test]
    fn quota_failure_does_not_block_a_claude_usage_update() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let expected_usage = fixture_usage(now, 654);
        let adapter = ClaudeProviderObservationAdapter::fixture_with_usage(
            Arc::new(FixedClock(now)),
            Err(RefreshFailure::SourceUnavailable),
            expected_usage.clone(),
        );
        let cached = ProviderPresentation::unavailable(CodingProvider::Claude);

        let observation = adapter
            .refresh(&cached, &RefreshAttempt::test())
            .unwrap()
            .expect("usage must publish when the independent quota probe fails");

        assert_eq!(observation.quota, cached.quota);
        assert_eq!(observation.usage, expected_usage);
    }

    #[test]
    fn invalid_quota_does_not_block_a_claude_usage_update() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let expected_usage = fixture_usage(now, 777);
        let mut invalid_quota = fixture_observation(now);
        invalid_quota.five_hour.used_percentage = 101.0;
        let adapter = ClaudeProviderObservationAdapter::fixture_with_usage(
            Arc::new(FixedClock(now)),
            Ok(invalid_quota),
            expected_usage.clone(),
        );
        let cached = ProviderPresentation::unavailable(CodingProvider::Claude);

        let observation = adapter
            .refresh(&cached, &RefreshAttempt::test())
            .unwrap()
            .expect("usage must publish when the independent quota result is invalid");

        assert_eq!(observation.quota, cached.quota);
        assert_eq!(observation.usage, expected_usage);
    }
}
