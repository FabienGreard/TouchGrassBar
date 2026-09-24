//! Bounded scan evidence. Read only fixed columns; never return source material.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use time::{Date, Duration};

const MAX_DAYS: i64 = 30;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ScanStatus {
    Complete,
    Indexing,
    Unavailable,
    /// The scan outcome could not be established.
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScanFiles {
    pub complete: u64,
    pub indexing: u64,
    pub error: u64,
    pub missing: u64,
    pub older_parser: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScanDay {
    pub ranking_day: String,
    pub observed_tokens: u64,
    pub priced_tokens: u64,
    pub cost_micros: Option<u64>,
    pub pricing_basis: Option<String>,
    pub revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UsageScanContext {
    pub status: ScanStatus,
    pub parser_version: u64,
    pub aggregate_parser_version: Option<u64>,
    pub catalog_version: Option<String>,
    pub files: ScanFiles,
    pub days: Vec<ScanDay>,
}

impl UsageScanContext {
    pub(crate) fn valid(&self) -> bool {
        self.parser_version <= 2_147_483_647
            && self
                .aggregate_parser_version
                .is_none_or(|version| version <= 2_147_483_647)
            && self
                .catalog_version
                .as_deref()
                .is_none_or(super::validation::catalog_version)
            && [
                self.files.complete,
                self.files.indexing,
                self.files.error,
                self.files.missing,
                self.files.older_parser,
                self.files.deferred.unwrap_or(0),
                self.files.excluded.unwrap_or(0),
            ]
            .iter()
            .all(|n| *n <= MAX_SAFE_INTEGER)
            && self.days.len() <= MAX_DAYS as usize
            && self.days.iter().all(|item| {
                super::types::day(&item.ranking_day)
                    && item.observed_tokens <= MAX_SAFE_INTEGER
                    && item.priced_tokens <= item.observed_tokens
                    && item
                        .revision
                        .is_none_or(|n| (1..=MAX_SAFE_INTEGER).contains(&n))
                    && item.cost_micros.is_none_or(|n| n <= MAX_SAFE_INTEGER)
                    && item
                        .pricing_basis
                        .as_deref()
                        .is_none_or(super::validation::catalog_version)
            })
            && self
                .days
                .windows(2)
                .all(|pair| pair[0].ranking_day > pair[1].ranking_day)
    }
}

/// Read fixed scan evidence for an automatic failure report.
pub(crate) fn read(
    provider: crate::providers::CodingProvider,
    connection: &Connection,
    today: Date,
    parser_version: u64,
    catalog_version: Option<&str>,
    status: ScanStatus,
) -> Result<UsageScanContext, ()> {
    struct Deadline<'a>(&'a Connection);
    impl Drop for Deadline<'_> {
        fn drop(&mut self) {
            self.0.progress_handler(0, None::<fn() -> bool>);
        }
    }
    let started = std::time::Instant::now();
    connection.progress_handler(1000, Some(move || started.elapsed().as_millis() >= 100));
    let _deadline = Deadline(connection);
    use crate::providers::CodingProvider;
    let files_sql = match provider {
        CodingProvider::Claude => {
            "SELECT COALESCE(SUM(completion_state = 'complete'), 0), COALESCE(SUM(completion_state = 'indexing'), 0), COALESCE(SUM(completion_state = 'error'), 0), COALESCE(SUM(completion_state = 'missing'), 0), COALESCE(SUM(parser_version < ?1), 0), NULL, NULL FROM claude_usage_files"
        }
        CodingProvider::Codex => {
            "SELECT COALESCE(SUM(completion_state = 'complete'), 0), COALESCE(SUM(completion_state NOT IN ('complete', 'error', 'deferred', 'deferred-error', 'missing')), 0), COALESCE(SUM(completion_state IN ('error', 'deferred-error')), 0), COALESCE(SUM(completion_state = 'missing'), 0), COALESCE(SUM(parser_version < ?1), 0), COALESCE(SUM(completion_state IN ('deferred', 'deferred-error')), 0), COALESCE(SUM(usage_excluded = 1), 0) FROM codex_usage_files"
        }
    };
    let files = connection
        .query_row(files_sql, [parser_version], |row| {
            Ok(ScanFiles {
                complete: row.get(0)?,
                indexing: row.get(1)?,
                error: row.get(2)?,
                missing: row.get(3)?,
                older_parser: row.get(4)?,
                deferred: row.get(5)?,
                excluded: row.get(6)?,
            })
        })
        .map_err(|_| ())?;
    let aggregate_parser_version = if provider == CodingProvider::Claude {
        connection.query_row(
        "SELECT value FROM claude_usage_index_meta WHERE key = 'usage_aggregate_parser_version'",
        [], |row| row.get::<_, String>(0),
    ).optional().map_err(|_| ())?.map(|value| value.parse::<u64>().map_err(|_| ())).transpose()?
    } else {
        None
    };
    let mut statement = connection
        .prepare(
            match provider {
              CodingProvider::Claude => "SELECT day, observed_tokens, priced_tokens, cost_usd, pricing_basis, revision FROM claude_usage_daily WHERE ?3 >= 0 AND day >= ?1 AND day <= ?2 ORDER BY day DESC LIMIT 30",
              CodingProvider::Codex => "WITH basis AS (
                SELECT d.day, CASE WHEN COUNT(*) = COUNT(d.pricing_basis) AND COUNT(DISTINCT d.pricing_basis) = 1 THEN MIN(d.pricing_basis) END AS pricing_basis
                FROM codex_usage_file_model_days d JOIN codex_usage_files f ON f.path = d.path
                WHERE f.parser_version = ?3 AND f.accounting_ready = 1 AND f.usage_excluded = 0 AND d.day >= ?1 AND d.day <= ?2 AND d.complete = 1 AND d.cost_usd IS NOT NULL GROUP BY d.day
              ) SELECT d.day, SUM(d.observed_tokens),
                CASE WHEN b.pricing_basis IS NOT NULL THEN SUM(d.priced_tokens) ELSE 0 END,
                CASE WHEN b.pricing_basis IS NOT NULL AND SUM(d.priced_tokens) > 0 THEN SUM(d.cost_usd) END,
                b.pricing_basis, NULL
                FROM codex_usage_file_days d JOIN codex_usage_files f ON f.path = d.path LEFT JOIN basis b ON b.day = d.day
                WHERE f.parser_version = ?3 AND f.accounting_ready = 1 AND f.usage_excluded = 0 AND d.day >= ?1 AND d.day <= ?2
                GROUP BY d.day ORDER BY d.day DESC LIMIT 30",
            }
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                (today - Duration::days(MAX_DAYS - 1)).to_string(),
                today.to_string(),
                parser_version
            ],
            |row| {
                let usd: Option<f64> = row.get(3)?;
                let cost_micros = usd
                    .map(|usd| {
                        let micros = (usd * 1_000_000.0).round();
                        if !micros.is_finite() || micros < 0.0 || micros > MAX_SAFE_INTEGER as f64 {
                            return Err(rusqlite::Error::InvalidQuery);
                        }
                        Ok(micros as u64)
                    })
                    .transpose()?;
                Ok(ScanDay {
                    ranking_day: row.get(0)?,
                    observed_tokens: row.get(1)?,
                    priced_tokens: row.get(2)?,
                    cost_micros,
                    pricing_basis: row
                        .get::<_, Option<String>>(4)?
                        .map(|basis| super::safe_catalog_reference(&basis)),
                    revision: row.get(5)?,
                })
            },
        )
        .map_err(|_| ())?;
    let context = UsageScanContext {
        status,
        parser_version,
        aggregate_parser_version,
        catalog_version: catalog_version.map(str::to_owned),
        files,
        days: rows.collect::<Result<Vec<_>, _>>().map_err(|_| ())?,
    };
    context.valid().then_some(context).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::CodingProvider;
    use rusqlite::OpenFlags;
    #[test]
    fn both_provider_reads_are_bounded_read_only_and_exclude_private_source_material() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.sqlite3");
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

        let today = time::macros::date!(2026 - 09 - 24);
        for offset in 0..40 {
            connection
                .execute(
                    "INSERT INTO claude_usage_daily VALUES(?1, 100, 0, NULL, NULL, 1)",
                    [(today - Duration::days(offset)).to_string()],
                )
                .unwrap();
        }
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let connection =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        let codex = read(
            CodingProvider::Codex,
            &transaction,
            today,
            25,
            None,
            ScanStatus::Unavailable,
        )
        .unwrap();
        let claude = read(
            CodingProvider::Claude,
            &transaction,
            today,
            15,
            None,
            ScanStatus::Unavailable,
        )
        .unwrap();
        assert_eq!(codex.files.error, 1);
        assert_eq!(codex.files.deferred, Some(1));
        assert_eq!(codex.files.excluded, Some(1));
        assert_eq!(codex.days[0].observed_tokens, 1000);
        assert_eq!(codex.days[0].priced_tokens, 500);
        assert_eq!(codex.days[0].cost_micros, Some(250000));
        assert_eq!(codex.days[0].revision, None);
        assert_eq!(claude.aggregate_parser_version, Some(14));
        assert_eq!(claude.files.error, 1);
        assert_eq!(claude.files.older_parser, 1);
        assert_eq!(claude.days.len(), 30);
        assert_eq!(claude.days[0].cost_micros, None);
        let text = serde_json::to_string(&(codex, claude)).unwrap();
        assert!(!text.contains("private"));
        assert!(!text.contains("dedupe"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(transaction);
        drop(connection);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("DROP TABLE codex_usage_file_model_days", [])
            .unwrap();
        assert!(
            read(
                CodingProvider::Codex,
                &connection,
                today,
                25,
                None,
                ScanStatus::Unavailable
            )
            .is_err()
        );
        assert!(
            read(
                CodingProvider::Claude,
                &connection,
                today,
                15,
                None,
                ScanStatus::Unavailable
            )
            .is_ok()
        );
    }
}
