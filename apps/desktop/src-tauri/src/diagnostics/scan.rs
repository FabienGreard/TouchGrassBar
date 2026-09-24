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
    /// A manual database read cannot prove that source traversal completed.
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
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScanDay {
    pub ranking_day: String,
    pub observed_tokens: u64,
    pub priced_tokens: u64,
    pub cost_micros: Option<u64>,
    pub pricing_basis: Option<String>,
    pub revision: u64,
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
            ]
            .iter()
            .all(|n| *n <= MAX_SAFE_INTEGER)
            && self.days.len() <= MAX_DAYS as usize
            && self.days.iter().all(|item| {
                super::types::day(&item.ranking_day)
                    && item.observed_tokens <= MAX_SAFE_INTEGER
                    && item.priced_tokens <= item.observed_tokens
                    && (1..=MAX_SAFE_INTEGER).contains(&item.revision)
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

/// The caller supplies the scan outcome. A manual read uses Unknown.
pub(crate) fn read_claude(
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
    let files = connection
        .query_row(
            "SELECT
           COALESCE(SUM(completion_state = 'complete'), 0),
           COALESCE(SUM(completion_state = 'indexing'), 0),
           COALESCE(SUM(completion_state = 'error'), 0),
           COALESCE(SUM(completion_state = 'missing'), 0),
           COALESCE(SUM(parser_version < ?1), 0)
         FROM claude_usage_files",
            [parser_version],
            |row| {
                Ok(ScanFiles {
                    complete: row.get(0)?,
                    indexing: row.get(1)?,
                    error: row.get(2)?,
                    missing: row.get(3)?,
                    older_parser: row.get(4)?,
                })
            },
        )
        .map_err(|_| ())?;
    let aggregate_parser_version = connection.query_row(
        "SELECT value FROM claude_usage_index_meta WHERE key = 'usage_aggregate_parser_version'",
        [], |row| row.get::<_, String>(0),
    ).optional().map_err(|_| ())?.map(|value| value.parse::<u64>().map_err(|_| ())).transpose()?;
    let mut statement = connection
        .prepare(
            "SELECT day, observed_tokens, priced_tokens, cost_usd, pricing_basis, revision
         FROM claude_usage_daily WHERE day >= ?1 AND day <= ?2 ORDER BY day DESC LIMIT 30",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                (today - Duration::days(MAX_DAYS - 1)).to_string(),
                today.to_string()
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
