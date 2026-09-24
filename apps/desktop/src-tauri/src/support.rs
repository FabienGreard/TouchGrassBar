//! A user-requested report from fixed, read-only queries.

use crate::diagnostics::scan::{ScanStatus, UsageScanContext};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::{path::PathBuf, time::Duration};
use time::OffsetDateTime;

#[derive(Clone)]
pub(crate) struct SupportReports(pub Option<PathBuf>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportReport<'a> {
    schema_version: u8,
    app_version: &'a str,
    captured_at: u64,
    providers: Vec<ProviderReport>,
}

#[derive(Serialize)]
struct ProviderReport {
    provider: crate::providers::CodingProvider,
    scan: Option<UsageScanContext>,
}

impl SupportReports {
    pub(crate) fn read(&self, app_version: &str, now: OffsetDateTime) -> Result<String, ()> {
        let path = self.0.as_deref().ok_or(())?;
        let connection =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| ())?;
        connection
            .busy_timeout(Duration::from_millis(500))
            .map_err(|_| ())?;
        let transaction = connection.unchecked_transaction().map_err(|_| ())?;
        let providers = crate::providers::PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| ProviderReport {
                provider: descriptor.provider,
                scan: crate::providers::read_scan_context(
                    descriptor.provider,
                    &transaction,
                    now.date(),
                    ScanStatus::Unknown,
                )
                .ok(),
            })
            .collect();
        let report = SupportReport {
            schema_version: 1,
            app_version,
            captured_at: u64::try_from(now.unix_timestamp_nanos() / 1_000_000).map_err(|_| ())?,
            providers,
        };
        let text = serde_json::to_string_pretty(&report).map_err(|_| ())?;
        (text.len() <= 32 * 1024).then_some(text).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_reads_bounded_evidence_without_source_material_or_database_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE claude_usage_files(path TEXT, completion_state TEXT, parser_version INTEGER);
             INSERT INTO claude_usage_files VALUES('/private/customer/project', 'error', 14);
             INSERT INTO claude_usage_files VALUES('/private/session', 'complete', 15);
             CREATE TABLE claude_usage_index_meta(key TEXT, value TEXT);
             INSERT INTO claude_usage_index_meta VALUES('usage_aggregate_parser_version', '14');
             INSERT INTO claude_usage_index_meta VALUES('dedupe_salt', 'private-secret');
             CREATE TABLE claude_usage_daily(day TEXT, observed_tokens INTEGER, priced_tokens INTEGER,
               cost_usd REAL, pricing_basis TEXT, revision INTEGER);
             CREATE TABLE codex_usage_files(path TEXT, completion_state TEXT, parser_version INTEGER, usage_excluded INTEGER, accounting_ready INTEGER);
             INSERT INTO codex_usage_files VALUES('/private/codex-session', 'deferred-error', 25, 0, 1);
             INSERT INTO codex_usage_files VALUES('/private/excluded-session', 'complete', 25, 1, 1);
             CREATE TABLE codex_usage_file_days(path TEXT, day TEXT, observed_tokens INTEGER, priced_tokens INTEGER, cost_usd REAL);
             INSERT INTO codex_usage_file_days VALUES('/private/codex-session', '2026-09-24', 1000, 500, 0.25);
             INSERT INTO codex_usage_file_days VALUES('/private/excluded-session', '2026-09-24', 9000, 9000, 99.0);
             CREATE TABLE codex_usage_file_model_days(path TEXT, day TEXT, complete INTEGER, cost_usd REAL, pricing_basis TEXT);
             INSERT INTO codex_usage_file_model_days VALUES('/private/codex-session', '2026-09-24', 1, 0.25, 'openai-standard-2026-08-26-v2');
             CREATE TABLE credentials(secret TEXT);
             INSERT INTO credentials VALUES('private-secret');"
        ).unwrap();
        let now = OffsetDateTime::parse(
            "2026-09-24T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        for offset in 0..40 {
            connection
                .execute(
                    "INSERT INTO claude_usage_daily VALUES(?1, 100, 0, NULL, NULL, 1)",
                    [(now.date() - time::Duration::days(offset)).to_string()],
                )
                .unwrap();
        }
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let text = SupportReports(Some(path.clone()))
            .read("0.0.52", now)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["providers"][0]["provider"], "codex");
        assert_eq!(value["providers"][0]["scan"]["files"]["error"], 1);
        assert_eq!(value["providers"][0]["scan"]["files"]["deferred"], 1);
        assert_eq!(value["providers"][0]["scan"]["files"]["excluded"], 1);
        assert_eq!(
            value["providers"][0]["scan"]["days"][0]["observedTokens"],
            1000
        );
        assert_eq!(
            value["providers"][0]["scan"]["days"][0]["pricedTokens"],
            500
        );
        assert_eq!(
            value["providers"][0]["scan"]["days"][0]["costMicros"],
            250000
        );
        assert!(value["providers"][0]["scan"]["days"][0]["revision"].is_null());
        assert_eq!(value["providers"][1]["provider"], "claude");
        assert_eq!(value["providers"][1]["scan"]["aggregateParserVersion"], 14);
        assert_eq!(value["providers"][1]["scan"]["status"], "unknown");
        assert_eq!(value["providers"][1]["scan"]["files"]["error"], 1);
        assert_eq!(value["providers"][1]["scan"]["files"]["olderParser"], 1);
        assert_eq!(
            value["providers"][1]["scan"]["days"]
                .as_array()
                .unwrap()
                .len(),
            30
        );
        assert!(!text.contains("private"));
        assert!(!text.contains("dedupe"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("DROP TABLE codex_usage_file_model_days", [])
            .unwrap();
        drop(connection);
        let text = SupportReports(Some(path)).read("0.0.52", now).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value["providers"][0]["scan"].is_null());
        assert!(value["providers"][1]["scan"].is_object());
        let missing = dir.path().join("absent.sqlite3");
        assert!(
            SupportReports(Some(missing.clone()))
                .read("0.0.52", now)
                .is_err()
        );
        assert!(!missing.exists());
    }
}
