//! Claude quota observation through the installed Claude Code CLI.
//!
//! The provider runs `/usage` in a private terminal, reduces the response to
//! the two supported quota lanes, and discards the terminal output.

mod cli_probe;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration as StdDuration,
};

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::registry::resolve_provider_executable;
use super::{ProviderObservation, ProviderObservationAdapter};
use crate::sanitized::{
    Clock, CodingProvider, ProviderPresentation, ProviderSnapshot, QuotaLane, RefreshAttempt,
    RefreshFailure,
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
    probe_directory: Option<PathBuf>,
    #[cfg(test)]
    fixture: Option<ClaudeQuotaObservation>,
}

impl ClaudeProviderObservationAdapter {
    pub(crate) fn production(clock: Arc<dyn Clock>, database_path: Option<PathBuf>) -> Self {
        Self {
            clock,
            probe_directory: database_path
                .as_deref()
                .and_then(Path::parent)
                .map(|parent| parent.join("claude-quota-probe")),
            #[cfg(test)]
            fixture: None,
        }
    }

    #[cfg(test)]
    pub(super) fn fixture(clock: Arc<dyn Clock>, observation: ClaudeQuotaObservation) -> Self {
        Self {
            clock,
            probe_directory: None,
            fixture: Some(observation),
        }
    }

    fn observe(
        &self,
        now: OffsetDateTime,
        timeout: StdDuration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ClaudeQuotaObservation, RefreshFailure> {
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
        cli_probe::probe_usage(&executable, probe_directory, now, timeout, cancelled).map_err(
            |failure| match failure {
                cli_probe::ProbeFailure::Cancelled => RefreshFailure::Cancelled,
                cli_probe::ProbeFailure::Unavailable => {
                    debug_event("cli_probe_unavailable reason=probe_failed");
                    RefreshFailure::SourceUnavailable
                }
            },
        )
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
        if attempt.should_skip_claude_quota_probe() {
            debug_event("refresh_skipped reason=claude_irrelevant_sources");
            return Ok(None);
        }
        let timeout = attempt.remaining()?;
        let now = self.clock.now();
        debug_event("cli_probe_started");
        let observation = self.observe(now, timeout, &|| attempt.is_cancelled())?;
        let quota = observation.sanitized_snapshot(now).map_err(|()| {
            debug_event("cli_probe_unavailable reason=invalid_quota");
            RefreshFailure::SourceUnavailable
        })?;
        if cached.quota == quota {
            debug_event("cli_probe_unchanged");
            return Ok(None);
        }
        debug_event("cli_probe_loaded lane_count=2");
        Ok(Some(ProviderObservation {
            quota,
            usage: cached.usage.clone(),
        }))
    }
}

pub(super) fn debug_live_quota_report(
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    let executable = resolve_provider_executable(CodingProvider::Claude).ok_or(())?;
    let observation = cli_probe::probe_usage(
        &executable,
        probe_directory,
        now,
        StdDuration::from_secs(30),
        &|| false,
    )
    .map_err(|_| ())?;
    format_debug_quota_report(&observation, now)
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

    struct FixedClock(OffsetDateTime);

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    #[test]
    fn codex_provider_notification_does_not_refresh_claude() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let adapter = ClaudeProviderObservationAdapter::fixture(
            Arc::new(FixedClock(now)),
            fixture_observation(now),
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
    fn joined_codex_notification_and_local_usage_do_not_refresh_claude() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let adapter = ClaudeProviderObservationAdapter::fixture(
            Arc::new(FixedClock(now)),
            fixture_observation(now),
        );
        let cached = ProviderPresentation::unavailable(CodingProvider::Claude);

        assert_eq!(
            adapter
                .refresh(
                    &cached,
                    &RefreshAttempt::test_provider_notification_with_local_usage(),
                )
                .unwrap(),
            None
        );
    }
}
