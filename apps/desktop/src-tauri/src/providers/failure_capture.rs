//! Content-free failure evidence. A scan emits at most one report per day/reason.

use std::cell::RefCell;

use crate::diagnostics::{
    self, Failure, ParserCode, ParserContext, ParserReason, PricingCode, PricingContext,
    PricingReason, Provider, ReviewStatus,
};

const MAX_SCAN_FAILURES: usize = 64;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Default)]
struct ScanEvidence {
    failures: Vec<Failure>,
    capture_epoch: u64,
    database_stages: Vec<&'static str>,
    files_seen: u64,
    records_accepted: u64,
    records_rejected: u64,
    source_versions: Vec<String>,
    reviewed: bool,
    unreviewed: bool,
    provider_stopping: bool,
}

thread_local! {
    static SCAN: RefCell<Option<ScanEvidence>> = const { RefCell::new(None) };
}

pub(crate) struct ScanFailures(bool);

impl ScanFailures {
    pub(crate) fn begin() -> Self {
        Self(SCAN.with_borrow_mut(|scan| {
            if scan.is_some() {
                false
            } else {
                *scan = Some(ScanEvidence {
                    capture_epoch: diagnostics::capture_epoch(),
                    ..ScanEvidence::default()
                });
                true
            }
        }))
    }
}

impl Drop for ScanFailures {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let Some(scan) = SCAN.with_borrow_mut(Option::take) else {
            return;
        };
        for mut failure in scan.failures {
            if let Failure::Parser { context, .. } = &mut failure {
                context.files_seen = Some(scan.files_seen);
                context.records_accepted = Some(scan.records_accepted);
                context.records_rejected = Some(scan.records_rejected);
                context.source_versions = scan.source_versions.clone();
                context.review_status = match (scan.reviewed, scan.unreviewed) {
                    (true, true) => ReviewStatus::Mixed,
                    (true, false) => ReviewStatus::Reviewed,
                    (false, true) => ReviewStatus::Unreviewed,
                    _ => ReviewStatus::Unknown,
                };
            }
            diagnostics::capture_at_epoch(failure, scan.capture_epoch);
        }
    }
}

pub(crate) fn file_seen() {
    SCAN.with_borrow_mut(|scan| {
        if let Some(scan) = scan {
            scan.files_seen = scan.files_seen.saturating_add(1).min(MAX_SAFE_INTEGER);
        }
    });
}

pub(crate) fn record_accepted() {
    SCAN.with_borrow_mut(|scan| {
        if let Some(scan) = scan {
            scan.records_accepted = scan
                .records_accepted
                .saturating_add(1)
                .min(MAX_SAFE_INTEGER);
        }
    });
}

pub(crate) fn record_rejected() {
    SCAN.with_borrow_mut(|scan| {
        if let Some(scan) = scan {
            scan.records_rejected = scan
                .records_rejected
                .saturating_add(1)
                .min(MAX_SAFE_INTEGER);
        }
    });
}

fn safe_source_version(value: &str) -> bool {
    let (release, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(release, suffix)| (release, Some(suffix)));
    value.len() <= 32
        && (1..=4).contains(&release.split('.').count())
        && release
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && prerelease.is_none_or(|suffix| {
            suffix.split_once('.').is_some_and(|(channel, build)| {
                matches!(channel, "alpha" | "beta" | "rc")
                    && (1..=3).contains(&build.split('.').count())
                    && build
                        .split('.')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            })
        })
}

pub(crate) fn source_version(version: &str, reviewed: bool) {
    SCAN.with_borrow_mut(|scan| {
        if let Some(scan) = scan {
            scan.reviewed |= reviewed;
            scan.unreviewed |= !reviewed;
            if scan.source_versions.len() < 8
                && safe_source_version(version)
                && !scan.source_versions.iter().any(|value| value == version)
            {
                scan.source_versions.push(version.to_owned());
                scan.source_versions.sort();
            }
        }
    });
}

fn same_reason(left: &Failure, right: &Failure) -> bool {
    match (left, right) {
        (
            Failure::Parser {
                provider: a,
                context: x,
                code: c,
            },
            Failure::Parser {
                provider: b,
                context: y,
                code: d,
            },
        ) => a == b && c == d && x.reason == y.reason,
        (
            Failure::Pricing {
                provider: a,
                context: x,
                code: c,
            },
            Failure::Pricing {
                provider: b,
                context: y,
                code: d,
            },
        ) => a == b && c == d && x.reason == y.reason && x.ranking_day == y.ranking_day,
        _ => left == right,
    }
}

pub(crate) fn capture_epoch() -> u64 {
    SCAN.with_borrow(|scan| {
        scan.as_ref()
            .map_or_else(diagnostics::capture_epoch, |scan| scan.capture_epoch)
    })
}

pub(crate) fn capture(failure: Failure) {
    let immediate = SCAN.with_borrow_mut(|scan| {
        let Some(scan) = scan else {
            return Some(failure);
        };
        if scan.failures.len() < MAX_SCAN_FAILURES
            && !scan
                .failures
                .iter()
                .any(|value| same_reason(value, &failure))
        {
            scan.failures.push(failure);
        }
        None
    });
    if let Some(failure) = immediate {
        diagnostics::capture(failure);
    }
}

pub(crate) fn parser(provider: Provider, parser_version: i64, reason: ParserReason) {
    capture(Failure::Parser {
        code: if reason == ParserReason::ReadFailed {
            ParserCode::ParserScanFailed
        } else {
            ParserCode::ParserRecordInvalid
        },
        provider,
        context: ParserContext {
            parser_version: u64::try_from(parser_version).ok(),
            source_versions: Vec::new(),
            review_status: ReviewStatus::Unknown,
            files_seen: None,
            records_accepted: None,
            records_rejected: None,
            reason,
        },
    });
}

pub(crate) fn pricing(
    provider: Provider,
    day: Option<time::Date>,
    reason: PricingReason,
    catalog_version: Option<&str>,
) {
    capture(Failure::Pricing {
        code: PricingCode::PricingCalculationFailed,
        provider,
        context: PricingContext {
            ranking_day: day.map(|day| day.to_string()),
            revision: None,
            // Repricing can read records from an older parser. Do not assign the running parser to that evidence.
            parser_version: None,
            catalog_version: catalog_version.map(str::to_owned),
            reason,
            // Lookup inputs can be individual messages. Only daily aggregates may export token totals.
            observed_tokens: None,
            priced_tokens: None,
            local_cost_micros: None,
            outgoing_cost_micros: None,
        },
    });
}

pub(crate) fn database_failure(path: &std::path::Path, stage: &'static str) {
    let capture_now = SCAN.with_borrow_mut(|scan| {
        let Some(scan) = scan else {
            return true;
        };
        if scan.database_stages.contains(&stage) || scan.failures.len() >= MAX_SCAN_FAILURES {
            return false;
        }
        scan.database_stages.push(stage);
        true
    });
    if capture_now {
        capture(crate::database::operation_failure(path, stage));
    }
}

pub(crate) fn database_connection_failure(connection: &rusqlite::Connection, stage: &'static str) {
    database_failure(
        std::path::Path::new(connection.path().unwrap_or_default()),
        stage,
    );
}

// The process layer still returns its existing unavailable outcome during shutdown.
// Keep that expected stop separate from an attempted provider request failure.
pub(crate) fn provider_stopping() {
    SCAN.with_borrow_mut(|scan| {
        if let Some(scan) = scan {
            scan.provider_stopping = true;
        }
    });
}

pub(crate) fn access(
    provider: Provider,
    operation: diagnostics::AccessOperation,
    reason: diagnostics::AccessReason,
) {
    if SCAN.with_borrow(|scan| scan.as_ref().is_some_and(|scan| scan.provider_stopping)) {
        return;
    }
    capture(Failure::ProviderAccess {
        code: diagnostics::ProviderAccessCode::ProviderAccessFailed,
        provider,
        context: diagnostics::ProviderAccessContext {
            operation,
            reason,
            status_code: None,
            retry_count: 0,
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_coalesces_repeated_failures_and_keeps_only_safe_versions() {
        let failures = diagnostics::collect_failures_for_test(|| {
            let _scan = ScanFailures::begin();
            file_seen();
            record_accepted();
            source_version("2.1.263", true);
            source_version("private-project-name", false);
            for _ in 0..100 {
                parser(Provider::Claude, 11, ParserReason::InvalidUsageShape);
                record_rejected();
            }
        });
        assert_eq!(failures.len(), 1);
        let Failure::Parser { context, .. } = &failures[0] else {
            panic!("parser failure");
        };
        assert_eq!(context.source_versions, ["2.1.263"]);
        assert_eq!(context.review_status, ReviewStatus::Mixed);
        assert_eq!(context.files_seen, Some(1));
        assert_eq!(context.records_accepted, Some(1));
        assert_eq!(context.records_rejected, Some(100));
        assert!(
            !serde_json::to_string(&failures)
                .unwrap()
                .contains("private-project")
        );
    }

    #[test]
    fn unreviewed_version_without_failure_emits_nothing() {
        let failures = diagnostics::collect_failures_for_test(|| {
            let _scan = ScanFailures::begin();
            source_version("2.1.999", false);
            record_accepted();
        });
        assert!(failures.is_empty());
    }
    #[test]
    fn diagnostic_counts_do_not_truncate_normal_large_scans() {
        let failures = diagnostics::collect_failures_for_test(|| {
            let _scan = ScanFailures::begin();
            for _ in 0..10_001 {
                record_accepted();
            }
            parser(Provider::Codex, 20, ParserReason::InvalidUsageShape);
            parser(Provider::Codex, 20, ParserReason::InvalidCounter);
            record_rejected();
        });
        assert_eq!(failures.len(), 2);
        for failure in failures {
            let Failure::Parser { context, .. } = failure else {
                panic!("parser failure");
            };
            assert_eq!(context.records_accepted, Some(10_001));
            assert_eq!(context.records_rejected, Some(1));
        }
        assert!(safe_source_version("0.148.0-alpha.21"));
        assert!(!safe_source_version("0.148.0-private-project.21"));
    }
}
