//! Pending Usage Snapshot synchronization.
//!
//! This deep module owns the local cumulative Usage Snapshot and outbox
//! contract, the bounded delivery runtime, and the protected Convex Adapter.
//! Callers only request work or pause it for an update.

mod runtime;
mod transport;

pub(crate) use runtime::{
    PendingUsageSynchronization, SynchronizationEnvironment, UsageSyncAttemptResult,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::sanitized::{
    ApiEquivalentCostQuality, CodingProvider, SanitizedDesktopStateV3, UsageCoverage,
    UsageEvidenceBasis, UsageTotal,
};

pub(crate) const MAX_USAGE_SYNC_BATCH: usize = 62;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_PRICING_BASIS_BYTES: usize = 256;
const MAX_LOCAL_VALUE_BYTES: usize = 4_096;
const MAX_ACKNOWLEDGEMENT_BYTES: usize = 64 * 1_024;
const INSTALLATION_CREDENTIAL_BYTES: usize = 52;
const FUTURE_OBSERVATION_TOLERANCE_MILLIS: u64 = 5 * 60 * 1_000;

const GENERATION_ACTIVE: &str = "active";
const GENERATION_BLOCKED: &str = "blocked";
const GENERATION_ABANDONED: &str = "abandoned";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UsageSyncError(&'static str);

impl UsageSyncError {
    const INVALID_VALUE: Self = Self("usage sync cannot use this value");
    const STORAGE_UNAVAILABLE: Self = Self("usage sync cannot use this database");
    const INVALID_RESPONSE: Self = Self("usage sync cannot use this response");
    const ABANDONED_GENERATION: Self = Self("usage sync cannot use an abandoned generation");
}

impl fmt::Display for UsageSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for UsageSyncError {}

impl From<rusqlite::Error> for UsageSyncError {
    fn from(_: rusqlite::Error) -> Self {
        Self::STORAGE_UNAVAILABLE
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SyncEvidenceBasis {
    ProviderReported,
    LocallyDerived,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SyncCoverage {
    Complete,
    Partial,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SyncCostQuality {
    Reconciled,
    Modeled,
    LocalOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CorrectionReason {
    ProviderReplacement,
    ParserCorrection,
}

/// Content-free correction proof for one provider refresh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct UsageSyncCorrections(BTreeMap<CodingProvider, UsageSyncCorrection>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UsageSyncCorrection {
    reason: CorrectionReason,
    source_revision: u64,
}

impl UsageSyncCorrections {
    pub(crate) fn record_parser_correction(
        &mut self,
        provider: CodingProvider,
        source_revision: u64,
    ) -> Result<(), UsageSyncError> {
        validate_revision(source_revision)?;
        self.0.insert(
            provider,
            UsageSyncCorrection {
                reason: CorrectionReason::ParserCorrection,
                source_revision,
            },
        );
        Ok(())
    }

    fn reason_for(&self, provider: CodingProvider) -> Option<CorrectionReason> {
        self.0.get(&provider).map(|correction| correction.reason)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SyncApiEquivalentCost {
    pub(crate) micros: u64,
    pub(crate) pricing_basis: String,
    pub(crate) quality: SyncCostQuality,
    pub(crate) coverage_percent: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct DailyUsageAggregate {
    pub(crate) provider: CodingProvider,
    pub(crate) ranking_day: String,
    pub(crate) evidence_basis: SyncEvidenceBasis,
    pub(crate) coverage: SyncCoverage,
    /// UTC Unix time in milliseconds.
    pub(crate) observed_at: u64,
    pub(crate) observed_tokens: u64,
    pub(crate) api_equivalent_cost: Option<SyncApiEquivalentCost>,
    pub(crate) correction_reason: Option<CorrectionReason>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UsageSyncSnapshot {
    pub(crate) provider: CodingProvider,
    pub(crate) ranking_day: String,
    pub(crate) revision: u64,
    pub(crate) evidence_basis: SyncEvidenceBasis,
    pub(crate) coverage: SyncCoverage,
    /// UTC Unix time in milliseconds.
    pub(crate) observed_at: u64,
    pub(crate) observed_tokens: u64,
    pub(crate) api_equivalent_cost: Option<SyncApiEquivalentCost>,
    pub(crate) correction_reason: Option<CorrectionReason>,
    pub(crate) correction_revision: Option<u64>,
}

impl UsageSyncSnapshot {
    fn from_aggregate(
        aggregate: DailyUsageAggregate,
        revision: u64,
        correction_revision: Option<u64>,
    ) -> Self {
        Self {
            provider: aggregate.provider,
            ranking_day: aggregate.ranking_day,
            revision,
            evidence_basis: aggregate.evidence_basis,
            coverage: aggregate.coverage,
            observed_at: aggregate.observed_at,
            observed_tokens: aggregate.observed_tokens,
            api_equivalent_cost: aggregate.api_equivalent_cost,
            correction_reason: aggregate.correction_reason,
            correction_revision,
        }
    }

    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_revision(self.revision)?;
        self.as_aggregate().validate()?;
        match (self.correction_reason, self.correction_revision) {
            (None, None) => Ok(()),
            (Some(_), Some(correction_revision)) => {
                validate_revision(correction_revision)?;
                if correction_revision > self.revision {
                    return Err(UsageSyncError::INVALID_VALUE);
                }
                Ok(())
            }
            _ => Err(UsageSyncError::INVALID_VALUE),
        }
    }

    fn correction(&self) -> Option<SnapshotCorrection> {
        match (self.correction_reason, self.correction_revision) {
            (Some(reason), Some(revision)) => Some(SnapshotCorrection { reason, revision }),
            _ => None,
        }
    }

    fn as_aggregate(&self) -> DailyUsageAggregate {
        DailyUsageAggregate {
            provider: self.provider,
            ranking_day: self.ranking_day.clone(),
            evidence_basis: self.evidence_basis,
            coverage: self.coverage,
            observed_at: self.observed_at,
            observed_tokens: self.observed_tokens,
            api_equivalent_cost: self.api_equivalent_cost.clone(),
            correction_reason: self.correction_reason,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SnapshotCorrection {
    reason: CorrectionReason,
    revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CorrectionContinuation {
    Stable(SnapshotCorrection),
    NewTransition(CorrectionReason),
}

impl DailyUsageAggregate {
    #[cfg(test)]
    pub(crate) fn with_correction(mut self, reason: CorrectionReason) -> Self {
        self.correction_reason = Some(reason);
        self
    }

    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_ranking_day(&self.ranking_day)?;
        validate_safe_integer(self.observed_at)?;
        validate_safe_integer(self.observed_tokens)?;
        if let Some(cost) = &self.api_equivalent_cost {
            cost.validate()?;
            if !approved_pricing_basis(self.provider, &cost.pricing_basis) {
                return Err(UsageSyncError::INVALID_VALUE);
            }
            if matches!(
                (self.evidence_basis, cost.quality),
                (
                    SyncEvidenceBasis::ProviderReported,
                    SyncCostQuality::LocalOnly
                ) | (
                    SyncEvidenceBasis::LocallyDerived,
                    SyncCostQuality::Reconciled
                )
            ) {
                return Err(UsageSyncError::INVALID_VALUE);
            }
        }
        if matches!(
            (self.evidence_basis, self.correction_reason),
            (
                SyncEvidenceBasis::ProviderReported,
                Some(CorrectionReason::ParserCorrection)
            ) | (
                SyncEvidenceBasis::LocallyDerived,
                Some(CorrectionReason::ProviderReplacement)
            )
        ) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        Ok(())
    }

    fn same_measurement(&self, candidate: &Self) -> bool {
        self.provider == candidate.provider
            && self.ranking_day == candidate.ranking_day
            && self.evidence_basis == candidate.evidence_basis
            && self.coverage == candidate.coverage
            && self.observed_at == candidate.observed_at
            && self.observed_tokens == candidate.observed_tokens
            && self.api_equivalent_cost == candidate.api_equivalent_cost
    }

    fn proves_token_decrease_from(&self, previous: &Self) -> bool {
        matches!(
            (
                previous.evidence_basis,
                self.evidence_basis,
                self.correction_reason
            ),
            (
                SyncEvidenceBasis::LocallyDerived,
                SyncEvidenceBasis::ProviderReported,
                Some(CorrectionReason::ProviderReplacement)
            ) | (
                SyncEvidenceBasis::LocallyDerived,
                SyncEvidenceBasis::LocallyDerived,
                Some(CorrectionReason::ParserCorrection)
            )
        )
    }
}

impl SyncApiEquivalentCost {
    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_safe_integer(self.micros)?;
        if !valid_pricing_basis(&self.pricing_basis) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        match (self.quality, self.coverage_percent) {
            (SyncCostQuality::Modeled, Some(percent))
                if percent.is_finite() && (0.0..=100.0).contains(&percent) =>
            {
                Ok(())
            }
            (SyncCostQuality::Reconciled | SyncCostQuality::LocalOnly, None) => Ok(()),
            _ => Err(UsageSyncError::INVALID_VALUE),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AcknowledgementOutcome {
    Committed,
    Idempotent,
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UsageSyncAcknowledgement {
    pub(crate) provider: CodingProvider,
    pub(crate) ranking_day: String,
    /// This is the committed server revision for a stale outcome.
    pub(crate) revision: u64,
    pub(crate) outcome: AcknowledgementOutcome,
}

impl UsageSyncAcknowledgement {
    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_ranking_day(&self.ranking_day).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
        validate_revision(self.revision).map_err(|_| UsageSyncError::INVALID_RESPONSE)
    }
}

/// A sanitized batch that is safe to give to the protected transport adapter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingUsageBatch {
    active_mac_generation: u64,
    snapshots: Vec<UsageSyncSnapshot>,
}

impl PendingUsageBatch {
    pub(crate) fn active_mac_generation(&self) -> u64 {
        self.active_mac_generation
    }

    #[cfg(test)]
    pub(crate) fn snapshots(&self) -> &[UsageSyncSnapshot] {
        &self.snapshots
    }

    pub(crate) fn is_for_current_utc_day(&self, now: OffsetDateTime) -> bool {
        let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
        !self.snapshots.is_empty()
            && self
                .snapshots
                .iter()
                .all(|snapshot| snapshot.ranking_day == ranking_day)
    }

    /// Add the Keychain value only at the transport boundary.
    pub(crate) fn mutation_args<'a>(
        &'a self,
        installation_credential: &'a str,
        now: OffsetDateTime,
    ) -> Result<UsageSyncMutationArgs<'a>, UsageSyncError> {
        if !valid_installation_credential(installation_credential) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        validate_generation(self.active_mac_generation)?;
        validate_current_day_batch(&self.snapshots, now)?;
        Ok(UsageSyncMutationArgs {
            installation_credential,
            active_mac_generation: self.active_mac_generation,
            snapshots: &self.snapshots,
        })
    }
}

/// Exact arguments for the `sync:dailyUsage` Convex mutation.
///
/// This type does not implement `Debug` and it borrows the installation
/// credential. The module never stores that credential in SQLite.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageSyncMutationArgs<'a> {
    installation_credential: &'a str,
    active_mac_generation: u64,
    snapshots: &'a [UsageSyncSnapshot],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueState {
    Pending,
    Blocked,
    Abandoned,
}

impl QueueState {
    fn as_database_value(self) -> &'static str {
        match self {
            Self::Pending => GENERATION_ACTIVE,
            Self::Blocked => GENERATION_BLOCKED,
            Self::Abandoned => GENERATION_ABANDONED,
        }
    }

    fn from_database_value(value: &str) -> Result<Self, UsageSyncError> {
        match value {
            GENERATION_ACTIVE => Ok(Self::Pending),
            GENERATION_BLOCKED => Ok(Self::Blocked),
            GENERATION_ABANDONED => Ok(Self::Abandoned),
            _ => Err(UsageSyncError::STORAGE_UNAVAILABLE),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueUpdate {
    Stored {
        provider: CodingProvider,
        revision: u64,
        state: QueueState,
    },
    Unchanged {
        provider: CodingProvider,
        revision: u64,
    },
    Stale {
        provider: CodingProvider,
        revision: u64,
    },
}

#[derive(Clone, Debug)]
struct StoredAggregate {
    revision: u64,
    aggregate: DailyUsageAggregate,
}

/// Create the private tables for the Daily Usage Aggregate and latest outbox.
pub(crate) fn install_usage_sync_schema(connection: &Connection) -> Result<(), UsageSyncError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS usage_sync_generations (
             active_generation INTEGER PRIMARY KEY,
             queue_state TEXT NOT NULL
                 CHECK(queue_state IN ('active', 'blocked', 'abandoned')),
             CHECK(active_generation >= 1 AND active_generation <= 9007199254740991)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS usage_sync_daily_aggregates (
             active_generation INTEGER NOT NULL,
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             revision INTEGER NOT NULL
                 CHECK(revision >= 1 AND revision <= 9007199254740991),
             aggregate_json TEXT NOT NULL CHECK(length(aggregate_json) <= 4096),
             PRIMARY KEY(active_generation, provider, ranking_day),
             FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS usage_sync_latest_outbox (
             active_generation INTEGER NOT NULL,
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             revision INTEGER NOT NULL
                 CHECK(revision >= 1 AND revision <= 9007199254740991),
             snapshot_json TEXT NOT NULL CHECK(length(snapshot_json) <= 4096),
             correction_reason TEXT
                 CHECK(correction_reason IS NULL OR correction_reason IN (
                     'provider-replacement', 'parser-correction'
                 )),
             correction_revision INTEGER,
             queue_state TEXT NOT NULL
                 CHECK(queue_state IN ('active', 'blocked', 'abandoned')),
             CHECK(
                 (correction_reason IS NULL AND correction_revision IS NULL)
                 OR (
                     correction_reason IS NOT NULL
                     AND correction_revision >= 1
                     AND correction_revision <= revision
                     AND correction_revision <= 9007199254740991
                 )
             ),
             PRIMARY KEY(active_generation, provider, ranking_day),
             FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
         ) STRICT;

         CREATE INDEX IF NOT EXISTS usage_sync_latest_outbox_pending
             ON usage_sync_latest_outbox(active_generation, queue_state, ranking_day, provider);

         CREATE TABLE IF NOT EXISTS usage_sync_correction_lineage (
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             source_revision INTEGER NOT NULL
                 CHECK(source_revision >= 1 AND source_revision <= 9007199254740991),
             reason TEXT NOT NULL CHECK(reason = 'parser-correction'),
             consumed_generation INTEGER
                 CHECK(consumed_generation IS NULL OR (
                     consumed_generation >= 1
                     AND consumed_generation <= 9007199254740991
                 )),
             PRIMARY KEY(provider, ranking_day)
         ) STRICT;",
    )?;
    migrate_usage_sync_outbox_correction_columns(connection)?;
    Ok(())
}

fn migrate_usage_sync_outbox_correction_columns(
    connection: &Connection,
) -> Result<(), UsageSyncError> {
    let columns = connection
        .prepare("PRAGMA table_info(usage_sync_latest_outbox)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    if !columns.contains("correction_reason") {
        connection.execute_batch(
            "ALTER TABLE usage_sync_latest_outbox
                 ADD COLUMN correction_reason TEXT
                 CHECK(correction_reason IS NULL OR correction_reason IN (
                     'provider-replacement', 'parser-correction'
                 ));",
        )?;
    }
    if !columns.contains("correction_revision") {
        connection.execute_batch(
            "ALTER TABLE usage_sync_latest_outbox
                 ADD COLUMN correction_revision INTEGER
                 CHECK(
                     (correction_reason IS NULL AND correction_revision IS NULL)
                     OR (
                         correction_reason IS NOT NULL
                         AND correction_revision >= 1
                         AND correction_revision <= revision
                         AND correction_revision <= 9007199254740991
                     )
                 );",
        )?;
    }
    connection.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS usage_sync_latest_outbox_correction_insert
         BEFORE INSERT ON usage_sync_latest_outbox
         WHEN (NEW.correction_reason IS NULL) != (NEW.correction_revision IS NULL)
              OR (
                  NEW.correction_reason IS NOT NULL
                  AND (
                      NEW.correction_reason NOT IN (
                          'provider-replacement', 'parser-correction'
                      )
                      OR NEW.correction_revision < 1
                      OR NEW.correction_revision > NEW.revision
                      OR NEW.correction_revision > 9007199254740991
                  )
              )
         BEGIN
             SELECT RAISE(ABORT, 'invalid usage sync correction provenance');
         END;

         CREATE TRIGGER IF NOT EXISTS usage_sync_latest_outbox_correction_update
         BEFORE UPDATE OF revision, correction_reason, correction_revision
         ON usage_sync_latest_outbox
         WHEN (NEW.correction_reason IS NULL) != (NEW.correction_revision IS NULL)
              OR (
                  NEW.correction_reason IS NOT NULL
                  AND (
                      NEW.correction_reason NOT IN (
                          'provider-replacement', 'parser-correction'
                      )
                      OR NEW.correction_revision < 1
                      OR NEW.correction_revision > NEW.revision
                      OR NEW.correction_revision > 9007199254740991
                  )
              )
         BEGIN
             SELECT RAISE(ABORT, 'invalid usage sync correction provenance');
         END;",
    )?;

    let rows = {
        let mut statement = connection.prepare(
            "SELECT active_generation, provider, ranking_day, revision, snapshot_json
             FROM usage_sync_latest_outbox",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (generation, provider, ranking_day, revision, snapshot_json) in rows {
        if snapshot_json.len() > MAX_LOCAL_VALUE_BYTES {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let mut snapshot: UsageSyncSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if snapshot.correction_reason.is_some() && snapshot.correction_revision.is_none() {
            // Legacy rows do not record the original correction revision. Do not
            // invent provenance that could authorize another decrease.
            snapshot.correction_reason = None;
        }
        snapshot
            .validate()
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        let expected_provider = provider_from_database_value(&provider)?;
        let expected_revision =
            u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if snapshot.provider != expected_provider
            || snapshot.ranking_day != ranking_day
            || snapshot.revision != expected_revision
        {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let migrated_json = encode_local_value(&snapshot)?;
        let correction_reason = snapshot
            .correction_reason
            .map(correction_reason_database_value);
        let correction_revision = snapshot
            .correction_revision
            .map(to_database_integer)
            .transpose()?;
        let updated = connection.execute(
            "UPDATE usage_sync_latest_outbox
             SET snapshot_json = ?1, correction_reason = ?2, correction_revision = ?3
             WHERE active_generation = ?4 AND provider = ?5 AND ranking_day = ?6
               AND revision = ?7",
            params![
                migrated_json,
                correction_reason,
                correction_revision,
                generation,
                provider,
                ranking_day,
                revision
            ],
        )?;
        if updated != 1 {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
    }
    Ok(())
}

/// Restore the one generation that can still send or remain blocked.
pub(crate) fn load_active_usage_sync_generation(
    connection: &Connection,
) -> Result<Option<u64>, UsageSyncError> {
    let mut statement = connection.prepare(
        "SELECT active_generation, queue_state
         FROM usage_sync_generations
         WHERE queue_state IN ('active', 'blocked')
         ORDER BY active_generation DESC
         LIMIT 2",
    )?;
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let generation =
        u64::try_from(row.get::<_, i64>(0)?).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    validate_generation(generation).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let state = QueueState::from_database_value(&row.get::<_, String>(1)?)?;
    if state == QueueState::Abandoned || rows.next()?.is_some() {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(Some(generation))
}

pub(crate) fn load_usage_sync_generation_state(
    connection: &Connection,
    active_mac_generation: u64,
) -> Result<Option<QueueState>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    connection
        .query_row(
            "SELECT queue_state FROM usage_sync_generations WHERE active_generation = ?1",
            [to_database_integer(active_mac_generation)?],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| QueueState::from_database_value(&value))
        .transpose()
}

/// Persist provider-private correction proof before an Active Mac exists.
pub(crate) fn stage_usage_sync_corrections(
    transaction: &Transaction<'_>,
    now: OffsetDateTime,
    corrections: &UsageSyncCorrections,
) -> Result<(), UsageSyncError> {
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    transaction.execute(
        "DELETE FROM usage_sync_correction_lineage WHERE ranking_day != ?1",
        [&ranking_day],
    )?;
    for (provider, correction) in &corrections.0 {
        validate_revision(correction.source_revision)?;
        if correction.reason != CorrectionReason::ParserCorrection {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        transaction.execute(
            "INSERT INTO usage_sync_correction_lineage(
                 provider, ranking_day, source_revision, reason, consumed_generation
             ) VALUES(?1, ?2, ?3, 'parser-correction', NULL)
             ON CONFLICT(provider, ranking_day) DO UPDATE SET
                 source_revision=excluded.source_revision,
                 reason=excluded.reason,
                 consumed_generation=CASE
                     WHEN excluded.source_revision > usage_sync_correction_lineage.source_revision
                     THEN NULL
                     ELSE usage_sync_correction_lineage.consumed_generation
                 END
             WHERE excluded.source_revision >= usage_sync_correction_lineage.source_revision",
            params![
                provider_database_value(*provider),
                ranking_day,
                to_database_integer(correction.source_revision)?
            ],
        )?;
    }
    Ok(())
}

fn load_staged_usage_sync_corrections(
    connection: &Connection,
    ranking_day: &str,
) -> Result<UsageSyncCorrections, UsageSyncError> {
    validate_ranking_day(ranking_day)?;
    let mut statement = connection.prepare(
        "SELECT provider, source_revision, reason
         FROM usage_sync_correction_lineage
         WHERE ranking_day = ?1 AND consumed_generation IS NULL
         ORDER BY provider",
    )?;
    let rows = statement.query_map([ranking_day], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut corrections = UsageSyncCorrections::default();
    for row in rows {
        let (provider, source_revision, reason) = row?;
        if reason != "parser-correction" {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let source_revision =
            u64::try_from(source_revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        corrections
            .record_parser_correction(provider_from_database_value(&provider)?, source_revision)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    }
    Ok(corrections)
}

fn consume_staged_usage_sync_correction(
    transaction: &Transaction<'_>,
    provider: CodingProvider,
    ranking_day: &str,
    correction: UsageSyncCorrection,
    active_mac_generation: u64,
) -> Result<(), UsageSyncError> {
    let updated = transaction.execute(
        "UPDATE usage_sync_correction_lineage
         SET consumed_generation = ?1
         WHERE provider = ?2 AND ranking_day = ?3 AND source_revision = ?4
           AND reason = 'parser-correction' AND consumed_generation IS NULL",
        params![
            to_database_integer(active_mac_generation)?,
            provider_database_value(provider),
            ranking_day,
            to_database_integer(correction.source_revision)?
        ],
    )?;
    if updated != 1 {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(())
}

/// Derive only provider rows for the current UTC Ranking Day.
///
/// The function does not read display names, quota data, model names, trends,
/// seven-day data, or thirty-day data.
#[cfg(test)]
pub(crate) fn current_utc_daily_aggregates(
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
) -> Result<Vec<DailyUsageAggregate>, UsageSyncError> {
    current_utc_daily_aggregates_with_corrections(state, now, &UsageSyncCorrections::default())
}

fn current_utc_daily_aggregates_with_corrections(
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    corrections: &UsageSyncCorrections,
) -> Result<Vec<DailyUsageAggregate>, UsageSyncError> {
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    let mut seen = BTreeSet::new();
    let mut aggregates = Vec::new();
    for presentation in &state.providers {
        if !seen.insert(presentation.provider) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        let Some(mut aggregate) = aggregate_from_total(
            presentation.provider,
            ranking_day.clone(),
            &presentation.usage.today,
        )?
        else {
            continue;
        };
        aggregate.correction_reason = corrections.reason_for(presentation.provider);
        aggregate.validate()?;
        aggregates.push(aggregate);
    }
    aggregates.sort_by_key(|aggregate| aggregate.provider);
    Ok(aggregates)
}

/// Store all current-day candidates in one caller-owned transaction.
pub(crate) fn queue_current_utc_day(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    queue_current_utc_day_with_corrections(
        transaction,
        active_mac_generation,
        state,
        now,
        &UsageSyncCorrections::default(),
    )
}

pub(crate) fn queue_current_utc_day_with_corrections(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    corrections: &UsageSyncCorrections,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    stage_usage_sync_corrections(transaction, now, corrections)?;
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    let staged = load_staged_usage_sync_corrections(transaction, &ranking_day)?;
    let aggregates = current_utc_daily_aggregates_with_corrections(state, now, &staged)?;
    let mut updates = Vec::with_capacity(aggregates.len());
    for aggregate in aggregates {
        let provider = aggregate.provider;
        let ranking_day = aggregate.ranking_day.clone();
        let update = queue_daily_aggregate(transaction, active_mac_generation, aggregate, now)?;
        if matches!(update, QueueUpdate::Stored { .. })
            && let Some(correction) = staged.0.get(&provider).copied()
        {
            consume_staged_usage_sync_correction(
                transaction,
                provider,
                &ranking_day,
                correction,
                active_mac_generation,
            )?;
        }
        updates.push(update);
    }
    Ok(updates)
}

/// Store one validated aggregate and its latest cumulative outbox revision.
///
/// Both writes use the supplied transaction. A blocked generation stays
/// blocked when a newer local aggregate replaces its outbox row.
pub(crate) fn queue_daily_aggregate(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    mut aggregate: DailyUsageAggregate,
    now: OffsetDateTime,
) -> Result<QueueUpdate, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    aggregate.validate()?;
    validate_current_day_aggregate(&aggregate, now)?;
    let queue_state = ensure_generation(transaction, active_mac_generation)?;
    if queue_state == QueueState::Abandoned {
        return Err(UsageSyncError::ABANDONED_GENERATION);
    }

    let existing = load_aggregate(
        transaction,
        active_mac_generation,
        aggregate.provider,
        &aggregate.ranking_day,
    )?;
    let outbox_lineage = load_outbox_snapshot(
        transaction,
        active_mac_generation,
        aggregate.provider,
        &aggregate.ranking_day,
    )?;
    let mut correction_revision = None;
    if let Some(existing) = &existing {
        if existing.aggregate.same_measurement(&aggregate)
            && (aggregate.correction_reason.is_none()
                || aggregate.correction_reason == existing.aggregate.correction_reason)
        {
            return Ok(QueueUpdate::Unchanged {
                provider: aggregate.provider,
                revision: existing.revision,
            });
        }
        if aggregate.correction_reason.is_none()
            && existing.aggregate.evidence_basis == SyncEvidenceBasis::LocallyDerived
            && aggregate.evidence_basis == SyncEvidenceBasis::ProviderReported
            && aggregate.observed_tokens < existing.aggregate.observed_tokens
        {
            aggregate.correction_reason = Some(CorrectionReason::ProviderReplacement);
        }
        aggregate.validate()?;
        if aggregate.observed_at < existing.aggregate.observed_at
            || (aggregate.observed_tokens < existing.aggregate.observed_tokens
                && !aggregate.proves_token_decrease_from(&existing.aggregate))
        {
            return Ok(QueueUpdate::Stale {
                provider: aggregate.provider,
                revision: existing.revision,
            });
        }
        if aggregate.correction_reason.is_none() {
            match carry_outbox_correction(
                outbox_lineage.as_ref(),
                &existing.aggregate,
                aggregate.evidence_basis,
            ) {
                Some(CorrectionContinuation::Stable(correction)) => {
                    aggregate.correction_reason = Some(correction.reason);
                    correction_revision = Some(correction.revision);
                }
                Some(CorrectionContinuation::NewTransition(reason)) => {
                    aggregate.correction_reason = Some(reason);
                }
                None => {}
            }
            aggregate.validate()?;
        }
    }

    let revision = existing.map_or(Ok(1), |stored| {
        stored
            .revision
            .checked_add(1)
            .ok_or(UsageSyncError::INVALID_VALUE)
    })?;
    validate_revision(revision)?;
    if aggregate.correction_reason.is_some() && correction_revision.is_none() {
        correction_revision = Some(revision);
    }
    let snapshot =
        UsageSyncSnapshot::from_aggregate(aggregate.clone(), revision, correction_revision);
    snapshot.validate()?;
    let aggregate_json = encode_local_value(&aggregate)?;
    let snapshot_json = encode_local_value(&snapshot)?;
    let correction_reason = snapshot
        .correction_reason
        .map(correction_reason_database_value);
    let correction_revision = snapshot
        .correction_revision
        .map(to_database_integer)
        .transpose()?;
    let provider = provider_database_value(aggregate.provider);
    let generation = to_database_integer(active_mac_generation)?;
    let revision = to_database_integer(revision)?;

    transaction.execute(
        "INSERT INTO usage_sync_daily_aggregates(
             active_generation, provider, ranking_day, revision, aggregate_json
         ) VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(active_generation, provider, ranking_day) DO UPDATE SET
             revision=excluded.revision,
             aggregate_json=excluded.aggregate_json",
        params![
            generation,
            provider,
            aggregate.ranking_day,
            revision,
            aggregate_json
        ],
    )?;
    transaction.execute(
        "INSERT INTO usage_sync_latest_outbox(
             active_generation, provider, ranking_day, revision, snapshot_json,
             correction_reason, correction_revision, queue_state
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(active_generation, provider, ranking_day) DO UPDATE SET
             revision=excluded.revision,
             snapshot_json=excluded.snapshot_json,
             correction_reason=excluded.correction_reason,
             correction_revision=excluded.correction_revision,
             queue_state=CASE
                 WHEN usage_sync_latest_outbox.queue_state = 'abandoned' THEN 'abandoned'
                 WHEN usage_sync_latest_outbox.queue_state = 'blocked' THEN 'blocked'
                 ELSE excluded.queue_state
             END",
        params![
            generation,
            provider,
            aggregate.ranking_day,
            revision,
            snapshot_json,
            correction_reason,
            correction_revision,
            queue_state.as_database_value()
        ],
    )?;

    Ok(QueueUpdate::Stored {
        provider: aggregate.provider,
        revision: u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?,
        state: queue_state,
    })
}

/// Load no more than 62 latest pending revisions for one generation.
#[cfg(test)]
pub(crate) fn load_pending_usage_batch(
    connection: &Connection,
    active_mac_generation: u64,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    load_pending_usage_batch_for_day(connection, active_mac_generation, None)
}

/// Load only the current UTC Ranking Day for issue #26 transport.
pub(crate) fn load_current_pending_usage_batch(
    connection: &Connection,
    active_mac_generation: u64,
    now: OffsetDateTime,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    load_pending_usage_batch_for_day(connection, active_mac_generation, Some(&ranking_day))
}

fn load_pending_usage_batch_for_day(
    connection: &Connection,
    active_mac_generation: u64,
    ranking_day: Option<&str>,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let mut statement = connection.prepare(
        "SELECT provider, ranking_day, revision, snapshot_json,
                correction_reason, correction_revision
         FROM usage_sync_latest_outbox
         WHERE active_generation = ?1
           AND queue_state = 'active'
           AND (?2 IS NULL OR ranking_day = ?2)
         ORDER BY ranking_day, provider
         LIMIT 62",
    )?;
    let rows = statement.query_map(
        params![to_database_integer(active_mac_generation)?, ranking_day],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        },
    )?;
    let mut snapshots = Vec::new();
    for row in rows {
        let (
            provider,
            ranking_day,
            revision,
            snapshot_json,
            correction_reason,
            correction_revision,
        ) = row?;
        if snapshot_json.len() > MAX_LOCAL_VALUE_BYTES {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let snapshot: UsageSyncSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        snapshot
            .validate()
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if snapshot.provider != provider_from_database_value(&provider)?
            || snapshot.ranking_day != ranking_day
            || snapshot.revision
                != u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?
            || snapshot
                .correction_reason
                .map(correction_reason_database_value)
                != correction_reason.as_deref()
            || snapshot
                .correction_revision
                .map(to_database_integer)
                .transpose()?
                != correction_revision
        {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        snapshots.push(snapshot);
    }
    if snapshots.is_empty() {
        return Ok(None);
    }
    validate_batch(&snapshots)?;
    Ok(Some(PendingUsageBatch {
        active_mac_generation,
        snapshots,
    }))
}

/// Parse the direct success value from `sync:dailyUsage`.
pub(crate) fn parse_usage_acknowledgements(
    bytes: &[u8],
) -> Result<Vec<UsageSyncAcknowledgement>, UsageSyncError> {
    if bytes.is_empty() || bytes.len() > MAX_ACKNOWLEDGEMENT_BYTES {
        return Err(UsageSyncError::INVALID_RESPONSE);
    }
    let acknowledgements: Vec<UsageSyncAcknowledgement> =
        serde_json::from_slice(bytes).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
    if acknowledgements.is_empty() || acknowledgements.len() > MAX_USAGE_SYNC_BATCH {
        return Err(UsageSyncError::INVALID_RESPONSE);
    }
    let mut keys = BTreeSet::new();
    for acknowledgement in &acknowledgements {
        acknowledgement.validate()?;
        if !keys.insert((
            acknowledgement.provider,
            acknowledgement.ranking_day.clone(),
        )) {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
    }
    Ok(acknowledgements)
}

/// Apply one complete success value to the exact submitted batch.
///
/// A committed or idempotent acknowledgement must name the submitted
/// revision. A stale acknowledgement must name a newer server revision. The
/// delete always uses the submitted revision. Therefore, a late response
/// cannot remove a newer local revision.
pub(crate) fn apply_usage_acknowledgements(
    transaction: &Transaction<'_>,
    batch: &PendingUsageBatch,
    acknowledgements: &[UsageSyncAcknowledgement],
) -> Result<usize, UsageSyncError> {
    validate_generation(batch.active_mac_generation)?;
    validate_batch(&batch.snapshots)?;
    if acknowledgements.len() != batch.snapshots.len() {
        return Err(UsageSyncError::INVALID_RESPONSE);
    }
    let submitted = batch
        .snapshots
        .iter()
        .map(|snapshot| {
            (
                (snapshot.provider, snapshot.ranking_day.as_str()),
                snapshot.revision,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for acknowledgement in acknowledgements {
        acknowledgement.validate()?;
        let key = (
            acknowledgement.provider,
            acknowledgement.ranking_day.as_str(),
        );
        if !seen.insert(key) {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
        let Some(submitted_revision) = submitted.get(&key).copied() else {
            return Err(UsageSyncError::INVALID_RESPONSE);
        };
        let revision_is_valid = match acknowledgement.outcome {
            AcknowledgementOutcome::Committed | AcknowledgementOutcome::Idempotent => {
                acknowledgement.revision == submitted_revision
            }
            AcknowledgementOutcome::Stale => acknowledgement.revision > submitted_revision,
        };
        if !revision_is_valid {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
    }

    let generation = to_database_integer(batch.active_mac_generation)?;
    let mut removed = 0;
    for snapshot in &batch.snapshots {
        let acknowledgement = acknowledgements
            .iter()
            .find(|acknowledgement| {
                acknowledgement.provider == snapshot.provider
                    && acknowledgement.ranking_day == snapshot.ranking_day
            })
            .ok_or(UsageSyncError::INVALID_RESPONSE)?;
        removed += transaction.execute(
            "DELETE FROM usage_sync_latest_outbox
             WHERE active_generation = ?1
               AND provider = ?2
               AND ranking_day = ?3
               AND revision = ?4
               AND queue_state = 'active'",
            params![
                generation,
                provider_database_value(snapshot.provider),
                snapshot.ranking_day,
                to_database_integer(snapshot.revision)?
            ],
        )?;
        if acknowledgement.outcome == AcknowledgementOutcome::Stale {
            advance_local_revision_floor(
                transaction,
                batch.active_mac_generation,
                snapshot,
                acknowledgement.revision,
            )?;
        }
    }
    Ok(removed)
}

fn advance_local_revision_floor(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    submitted_snapshot: &UsageSyncSnapshot,
    server_revision: u64,
) -> Result<(), UsageSyncError> {
    let provider = submitted_snapshot.provider;
    let ranking_day = &submitted_snapshot.ranking_day;
    let stored = load_aggregate(transaction, active_mac_generation, provider, ranking_day)?
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)?;
    if stored.revision > server_revision {
        return Ok(());
    }
    let current_outbox =
        load_outbox_snapshot(transaction, active_mac_generation, provider, ranking_day)?;
    let correction = match current_outbox.as_ref() {
        Some(snapshot) => snapshot.correction(),
        None => submitted_snapshot.correction(),
    };
    let correction_revision = match (stored.aggregate.correction_reason, correction) {
        (None, None) => None,
        (Some(reason), Some(correction)) if reason == correction.reason => {
            Some(correction.revision)
        }
        _ => return Err(UsageSyncError::STORAGE_UNAVAILABLE),
    };
    let revision = server_revision
        .checked_add(1)
        .ok_or(UsageSyncError::INVALID_RESPONSE)?;
    validate_revision(revision).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
    let snapshot =
        UsageSyncSnapshot::from_aggregate(stored.aggregate, revision, correction_revision);
    snapshot
        .validate()
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let snapshot_json = encode_local_value(&snapshot)?;
    let correction_reason = snapshot
        .correction_reason
        .map(correction_reason_database_value);
    let correction_revision = snapshot
        .correction_revision
        .map(to_database_integer)
        .transpose()?;
    let generation = to_database_integer(active_mac_generation)?;
    let provider_value = provider_database_value(provider);
    let revision_value = to_database_integer(revision)?;
    let previous_revision = to_database_integer(stored.revision)?;
    let updated = transaction.execute(
        "UPDATE usage_sync_daily_aggregates
         SET revision = ?1
         WHERE active_generation = ?2 AND provider = ?3 AND ranking_day = ?4
           AND revision = ?5",
        params![
            revision_value,
            generation,
            provider_value,
            ranking_day,
            previous_revision
        ],
    )?;
    if updated != 1 {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let queue_state = transaction
        .query_row(
            "SELECT queue_state FROM usage_sync_latest_outbox
             WHERE active_generation = ?1 AND provider = ?2 AND ranking_day = ?3",
            params![generation, provider_value, ranking_day],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| QueueState::from_database_value(&value))
        .transpose()?
        .unwrap_or(ensure_generation(transaction, active_mac_generation)?);
    transaction.execute(
        "INSERT INTO usage_sync_latest_outbox(
             active_generation, provider, ranking_day, revision, snapshot_json,
             correction_reason, correction_revision, queue_state
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(active_generation, provider, ranking_day) DO UPDATE SET
             revision=excluded.revision,
             snapshot_json=excluded.snapshot_json,
             correction_reason=excluded.correction_reason,
             correction_revision=excluded.correction_revision,
             queue_state=CASE
                 WHEN usage_sync_latest_outbox.queue_state = 'abandoned' THEN 'abandoned'
                 WHEN usage_sync_latest_outbox.queue_state = 'blocked' THEN 'blocked'
                 ELSE excluded.queue_state
             END",
        params![
            generation,
            provider_value,
            ranking_day,
            revision_value,
            snapshot_json,
            correction_reason,
            correction_revision,
            queue_state.as_database_value()
        ],
    )?;
    Ok(())
}

/// Stop one generation after a structured authority rejection.
pub(crate) fn mark_generation_authority_rejected(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
) -> Result<usize, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let generation = to_database_integer(active_mac_generation)?;
    let current = ensure_generation(transaction, active_mac_generation)?;
    if current == QueueState::Abandoned {
        return Ok(0);
    }
    transaction.execute(
        "UPDATE usage_sync_generations
         SET queue_state = 'blocked'
         WHERE active_generation = ?1 AND queue_state != 'abandoned'",
        [generation],
    )?;
    Ok(transaction.execute(
        "UPDATE usage_sync_latest_outbox
         SET queue_state = 'blocked'
         WHERE active_generation = ?1 AND queue_state != 'abandoned'",
        [generation],
    )?)
}

/// Activate one server-owned generation and permanently abandon older rows.
pub(crate) fn activate_generation(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
) -> Result<usize, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let generation = to_database_integer(active_mac_generation)?;
    transaction.execute(
        "INSERT INTO usage_sync_generations(active_generation, queue_state)
         VALUES(?1, 'active')
         ON CONFLICT(active_generation) DO NOTHING",
        [generation],
    )?;
    transaction.execute(
        "UPDATE usage_sync_generations
         SET queue_state = 'abandoned'
         WHERE active_generation < ?1",
        [generation],
    )?;
    Ok(transaction.execute(
        "UPDATE usage_sync_latest_outbox
         SET queue_state = 'abandoned'
         WHERE active_generation < ?1 AND queue_state != 'abandoned'",
        [generation],
    )?)
}

fn aggregate_from_total(
    provider: CodingProvider,
    ranking_day: String,
    total: &UsageTotal,
) -> Result<Option<DailyUsageAggregate>, UsageSyncError> {
    let (
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        api_equivalent_cost_usd,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    ) = match total {
        UsageTotal::Unavailable => return Ok(None),
        UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_at,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        }
        | UsageTotal::Stale {
            evidence_basis,
            coverage,
            observed_at,
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            *evidence_basis,
            *coverage,
            observed_at,
            *observed_tokens,
            *api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            *api_equivalent_cost_quality,
            *api_equivalent_cost_coverage_percent,
        ),
    };
    let observed_at = parse_observed_at(observed_at)?;
    if observed_at.to_offset(UtcOffset::UTC).date().to_string() != ranking_day {
        return Ok(None);
    }
    let observed_at = offset_date_time_millis(observed_at)?;
    let evidence_basis = match evidence_basis {
        UsageEvidenceBasis::ProviderReported => SyncEvidenceBasis::ProviderReported,
        UsageEvidenceBasis::LocallyDerived => SyncEvidenceBasis::LocallyDerived,
        UsageEvidenceBasis::Mixed => return Err(UsageSyncError::INVALID_VALUE),
    };
    let coverage = match coverage {
        UsageCoverage::Complete => SyncCoverage::Complete,
        UsageCoverage::Partial => SyncCoverage::Partial,
    };
    validate_safe_integer(observed_tokens)?;
    let api_equivalent_cost = convert_cost(
        api_equivalent_cost_usd,
        api_equivalent_cost_basis,
        api_equivalent_cost_quality,
        api_equivalent_cost_coverage_percent,
    )?;
    let aggregate = DailyUsageAggregate {
        provider,
        ranking_day,
        evidence_basis,
        coverage,
        observed_at,
        observed_tokens,
        api_equivalent_cost,
        correction_reason: None,
    };
    aggregate.validate()?;
    Ok(Some(aggregate))
}

fn convert_cost(
    cost_usd: Option<f64>,
    pricing_basis: &Option<String>,
    quality: Option<ApiEquivalentCostQuality>,
    coverage_percent: Option<f64>,
) -> Result<Option<SyncApiEquivalentCost>, UsageSyncError> {
    let (cost_usd, pricing_basis, quality) = match (cost_usd, pricing_basis, quality) {
        (None, None, None) if coverage_percent.is_none() => return Ok(None),
        (Some(cost_usd), Some(pricing_basis), Some(quality)) => {
            (cost_usd, pricing_basis.clone(), quality)
        }
        _ => return Err(UsageSyncError::INVALID_VALUE),
    };
    if !cost_usd.is_finite() || cost_usd < 0.0 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let micros = (cost_usd * 1_000_000.0).round();
    if !micros.is_finite() || micros < 0.0 || micros > MAX_SAFE_INTEGER as f64 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let (quality, coverage_percent) = match quality {
        ApiEquivalentCostQuality::Reconciled => (SyncCostQuality::Reconciled, coverage_percent),
        ApiEquivalentCostQuality::Modeled => (SyncCostQuality::Modeled, coverage_percent),
        ApiEquivalentCostQuality::LocalOnly => (SyncCostQuality::LocalOnly, coverage_percent),
    };
    let cost = SyncApiEquivalentCost {
        micros: micros as u64,
        pricing_basis,
        quality,
        coverage_percent,
    };
    cost.validate()?;
    Ok(Some(cost))
}

fn load_aggregate(
    connection: &Connection,
    active_mac_generation: u64,
    provider: CodingProvider,
    ranking_day: &str,
) -> Result<Option<StoredAggregate>, UsageSyncError> {
    let stored = connection
        .query_row(
            "SELECT revision, aggregate_json
             FROM usage_sync_daily_aggregates
             WHERE active_generation = ?1 AND provider = ?2 AND ranking_day = ?3",
            params![
                to_database_integer(active_mac_generation)?,
                provider_database_value(provider),
                ranking_day
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((revision, aggregate_json)) = stored else {
        return Ok(None);
    };
    if aggregate_json.len() > MAX_LOCAL_VALUE_BYTES {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let revision = u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    validate_revision(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let aggregate: DailyUsageAggregate =
        serde_json::from_str(&aggregate_json).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    aggregate
        .validate()
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if aggregate.provider != provider || aggregate.ranking_day != ranking_day {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(Some(StoredAggregate {
        revision,
        aggregate,
    }))
}

fn load_outbox_snapshot(
    connection: &Connection,
    active_mac_generation: u64,
    provider: CodingProvider,
    ranking_day: &str,
) -> Result<Option<UsageSyncSnapshot>, UsageSyncError> {
    let stored = connection
        .query_row(
            "SELECT revision, snapshot_json, correction_reason, correction_revision
             FROM usage_sync_latest_outbox
             WHERE active_generation = ?1 AND provider = ?2 AND ranking_day = ?3
               AND queue_state != 'abandoned'",
            params![
                to_database_integer(active_mac_generation)?,
                provider_database_value(provider),
                ranking_day
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((revision, snapshot_json, correction_reason, correction_revision)) = stored else {
        return Ok(None);
    };
    if snapshot_json.len() > MAX_LOCAL_VALUE_BYTES {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let revision = u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let snapshot: UsageSyncSnapshot =
        serde_json::from_str(&snapshot_json).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    snapshot
        .validate()
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if snapshot.provider != provider
        || snapshot.ranking_day != ranking_day
        || snapshot.revision != revision
        || snapshot
            .correction_reason
            .map(correction_reason_database_value)
            != correction_reason.as_deref()
        || snapshot
            .correction_revision
            .map(to_database_integer)
            .transpose()?
            != correction_revision
    {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(Some(snapshot))
}

fn carry_outbox_correction(
    outbox: Option<&UsageSyncSnapshot>,
    previous: &DailyUsageAggregate,
    current_evidence_basis: SyncEvidenceBasis,
) -> Option<CorrectionContinuation> {
    match (
        outbox.and_then(UsageSyncSnapshot::correction),
        previous.evidence_basis,
        current_evidence_basis,
    ) {
        (
            Some(
                correction @ SnapshotCorrection {
                    reason: CorrectionReason::ProviderReplacement,
                    ..
                },
            ),
            _,
            SyncEvidenceBasis::ProviderReported,
        ) => Some(CorrectionContinuation::Stable(correction)),
        (
            Some(SnapshotCorrection {
                reason: CorrectionReason::ParserCorrection,
                ..
            }),
            SyncEvidenceBasis::LocallyDerived,
            SyncEvidenceBasis::ProviderReported,
        ) => Some(CorrectionContinuation::NewTransition(
            CorrectionReason::ProviderReplacement,
        )),
        (
            Some(
                correction @ SnapshotCorrection {
                    reason: CorrectionReason::ParserCorrection,
                    ..
                },
            ),
            _,
            SyncEvidenceBasis::LocallyDerived,
        ) => Some(CorrectionContinuation::Stable(correction)),
        _ => None,
    }
}

fn ensure_generation(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
) -> Result<QueueState, UsageSyncError> {
    let generation = to_database_integer(active_mac_generation)?;
    transaction.execute(
        "INSERT INTO usage_sync_generations(active_generation, queue_state)
         VALUES(?1, 'active')
         ON CONFLICT(active_generation) DO NOTHING",
        [generation],
    )?;
    let state = transaction.query_row(
        "SELECT queue_state FROM usage_sync_generations WHERE active_generation = ?1",
        [generation],
        |row| row.get::<_, String>(0),
    )?;
    QueueState::from_database_value(&state)
}

fn validate_batch(snapshots: &[UsageSyncSnapshot]) -> Result<(), UsageSyncError> {
    if snapshots.is_empty() || snapshots.len() > MAX_USAGE_SYNC_BATCH {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let mut keys = BTreeSet::new();
    for snapshot in snapshots {
        snapshot.validate()?;
        if !keys.insert((snapshot.provider, snapshot.ranking_day.as_str())) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
    }
    Ok(())
}

fn validate_current_day_batch(
    snapshots: &[UsageSyncSnapshot],
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    validate_batch(snapshots)?;
    for snapshot in snapshots {
        validate_current_day_aggregate(&snapshot.as_aggregate(), now)?;
    }
    Ok(())
}

fn validate_current_day_aggregate(
    aggregate: &DailyUsageAggregate,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    let now_millis = offset_date_time_millis(now)?;
    let day = now.to_offset(UtcOffset::UTC).date();
    if aggregate.ranking_day != day.to_string() {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let day_start = day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let day_start_millis = offset_date_time_millis(day_start)?;
    let day_end_millis = day_start_millis
        .checked_add(24 * 60 * 60 * 1_000)
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    let latest_millis = now_millis
        .checked_add(FUTURE_OBSERVATION_TOLERANCE_MILLIS)
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    if aggregate.observed_at < day_start_millis
        || aggregate.observed_at >= day_end_millis
        || aggregate.observed_at > latest_millis
    {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_generation(value: u64) -> Result<(), UsageSyncError> {
    validate_revision(value)
}

fn validate_revision(value: u64) -> Result<(), UsageSyncError> {
    if value == 0 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    validate_safe_integer(value)
}

fn validate_safe_integer(value: u64) -> Result<(), UsageSyncError> {
    if value <= MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(UsageSyncError::INVALID_VALUE)
    }
}

fn to_database_integer(value: u64) -> Result<i64, UsageSyncError> {
    validate_safe_integer(value)?;
    i64::try_from(value).map_err(|_| UsageSyncError::INVALID_VALUE)
}

fn validate_ranking_day(value: &str) -> Result<(), UsageSyncError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let year = value[0..4]
        .parse::<i32>()
        .map_err(|_| UsageSyncError::INVALID_VALUE)?;
    let month = value[5..7]
        .parse::<u8>()
        .map_err(|_| UsageSyncError::INVALID_VALUE)?;
    let day = value[8..10]
        .parse::<u8>()
        .map_err(|_| UsageSyncError::INVALID_VALUE)?;
    let month = time::Month::try_from(month).map_err(|_| UsageSyncError::INVALID_VALUE)?;
    time::Date::from_calendar_date(year, month, day)
        .map(|_| ())
        .map_err(|_| UsageSyncError::INVALID_VALUE)
}

fn parse_observed_at(value: &str) -> Result<OffsetDateTime, UsageSyncError> {
    if value.is_empty() || value.len() > 64 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| UsageSyncError::INVALID_VALUE)
}

fn offset_date_time_millis(value: OffsetDateTime) -> Result<u64, UsageSyncError> {
    let milliseconds = value.unix_timestamp_nanos().div_euclid(1_000_000);
    let milliseconds = u64::try_from(milliseconds).map_err(|_| UsageSyncError::INVALID_VALUE)?;
    validate_safe_integer(milliseconds)?;
    Ok(milliseconds)
}

fn valid_pricing_basis(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PRICING_BASIS_BYTES
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b':' | b'+' | b'-')
        })
}

fn approved_pricing_basis(provider: CodingProvider, value: &str) -> bool {
    matches!(
        (provider, value),
        (CodingProvider::Codex, "openai-standard-2026-08-06-v1")
            | (CodingProvider::Claude, "anthropic-standard-2026-08-07-v1")
    )
}

fn valid_installation_credential(value: &str) -> bool {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    value.len() == INSTALLATION_CREDENTIAL_BYTES
        && value.bytes().all(|byte| ALPHABET.contains(&byte))
}

fn encode_local_value(value: &impl Serialize) -> Result<String, UsageSyncError> {
    let encoded = serde_json::to_string(value).map_err(|_| UsageSyncError::INVALID_VALUE)?;
    if encoded.len() > MAX_LOCAL_VALUE_BYTES {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(encoded)
}

fn provider_database_value(provider: CodingProvider) -> &'static str {
    match provider {
        CodingProvider::Codex => "codex",
        CodingProvider::Claude => "claude",
    }
}

fn correction_reason_database_value(reason: CorrectionReason) -> &'static str {
    match reason {
        CorrectionReason::ProviderReplacement => "provider-replacement",
        CorrectionReason::ParserCorrection => "parser-correction",
    }
}

fn provider_from_database_value(value: &str) -> Result<CodingProvider, UsageSyncError> {
    match value {
        "codex" => Ok(CodingProvider::Codex),
        "claude" => Ok(CodingProvider::Claude),
        _ => Err(UsageSyncError::STORAGE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use serde_json::{Value, json};
    use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::sanitized::{
        ProviderPresenceStatus, ProviderPresentation, ProviderSnapshot, SanitizedProfileOutcome,
        SyncState, SyncStatus, TopModelUsage, UsagePeriods, UsageScanStatus,
    };

    const NOW: &str = "2026-08-08T12:34:56Z";
    const DAY_START_MILLIS: u64 = 1_786_147_200_000;
    const INSTALLATION_CREDENTIAL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse(NOW, &Rfc3339).unwrap()
    }

    fn unavailable_periods() -> UsagePeriods {
        UsagePeriods {
            scan_status: UsageScanStatus::Complete,
            today_scan_status: UsageScanStatus::Complete,
            seven_day_scan_status: UsageScanStatus::Complete,
            thirty_day_scan_status: UsageScanStatus::Complete,
            today: UsageTotal::Unavailable,
            seven_days: UsageTotal::Unavailable,
            thirty_days: UsageTotal::Unavailable,
        }
    }

    fn total(
        evidence_basis: UsageEvidenceBasis,
        tokens: u64,
        observed_at: &str,
        cost: Option<(f64, &str, ApiEquivalentCostQuality, Option<f64>)>,
    ) -> UsageTotal {
        let (usd, basis, quality, coverage_percent) = cost.map_or(
            (None, None, None, None),
            |(usd, basis, quality, coverage_percent)| {
                (
                    Some(usd),
                    Some(basis.to_owned()),
                    Some(quality),
                    coverage_percent,
                )
            },
        );
        UsageTotal::Current {
            evidence_basis,
            coverage: UsageCoverage::Complete,
            observed_at: observed_at.to_owned(),
            observed_tokens: tokens,
            api_equivalent_cost_usd: usd,
            trend_percent: Some(99.0),
            trend_previous_tokens: Some(1),
            api_equivalent_cost_basis: basis,
            api_equivalent_cost_quality: quality,
            api_equivalent_cost_coverage_percent: coverage_percent,
        }
    }

    fn state_with_totals(codex: UsageTotal, claude: UsageTotal) -> SanitizedDesktopStateV3 {
        let provider = |provider, display_name: &str, today| ProviderPresentation {
            provider,
            display_name: display_name.to_owned(),
            presence: ProviderPresenceStatus::Detected,
            quota: ProviderSnapshot::Unavailable {
                provider,
                quota_lanes: [],
            },
            usage: UsagePeriods {
                today,
                seven_days: UsageTotal::Unavailable,
                thirty_days: UsageTotal::Unavailable,
                ..unavailable_periods()
            },
            top_model_usage: Some(TopModelUsage {
                model: Some("private-model-name".to_owned()),
                observed_tokens: 999,
            }),
        };
        SanitizedDesktopStateV3 {
            contract_version: 4,
            generated_at: NOW.to_owned(),
            revision: "private-read-model-revision".to_owned(),
            providers: vec![
                provider(CodingProvider::Codex, "Codex private display", codex),
                provider(CodingProvider::Claude, "Claude private display", claude),
            ],
            top_model_usage: Some(TopModelUsage {
                model: Some("private-combined-model".to_owned()),
                observed_tokens: 1_000,
            }),
            combined_usage: unavailable_periods(),
            sync: SyncState {
                status: SyncStatus::Unavailable,
                last_successful_at: None,
            },
            profile: SanitizedProfileOutcome::NotAuthorized,
        }
    }

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        install_usage_sync_schema(&connection).unwrap();
        connection
    }

    fn aggregate(
        provider: CodingProvider,
        tokens: u64,
        observed_offset_millis: u64,
    ) -> DailyUsageAggregate {
        DailyUsageAggregate {
            provider,
            ranking_day: "2026-08-08".to_owned(),
            evidence_basis: match provider {
                CodingProvider::Codex => SyncEvidenceBasis::ProviderReported,
                CodingProvider::Claude => SyncEvidenceBasis::LocallyDerived,
            },
            coverage: SyncCoverage::Complete,
            observed_at: DAY_START_MILLIS + observed_offset_millis,
            observed_tokens: tokens,
            api_equivalent_cost: None,
            correction_reason: None,
        }
    }

    fn queue_daily_aggregate(
        transaction: &Transaction<'_>,
        active_mac_generation: u64,
        aggregate: DailyUsageAggregate,
    ) -> Result<QueueUpdate, UsageSyncError> {
        super::queue_daily_aggregate(transaction, active_mac_generation, aggregate, now())
    }

    fn acknowledgement(
        snapshot: &UsageSyncSnapshot,
        outcome: AcknowledgementOutcome,
        revision: u64,
    ) -> UsageSyncAcknowledgement {
        UsageSyncAcknowledgement {
            provider: snapshot.provider,
            ranking_day: snapshot.ranking_day.clone(),
            revision,
            outcome,
        }
    }

    #[test]
    fn derives_current_utc_day_for_codex_and_claude() {
        let state = state_with_totals(
            total(
                UsageEvidenceBasis::ProviderReported,
                120,
                NOW,
                Some((
                    1.25,
                    "openai-standard-2026-08-06-v1",
                    ApiEquivalentCostQuality::Reconciled,
                    None,
                )),
            ),
            total(
                UsageEvidenceBasis::LocallyDerived,
                80,
                NOW,
                Some((
                    0.75,
                    "anthropic-standard-2026-08-07-v1",
                    ApiEquivalentCostQuality::Modeled,
                    Some(75.0),
                )),
            ),
        );
        let local_time = now().to_offset(time::UtcOffset::from_hms(-10, 0, 0).unwrap());
        let aggregates = current_utc_daily_aggregates(&state, local_time).unwrap();

        assert_eq!(aggregates.len(), 2);
        assert_eq!(aggregates[0].provider, CodingProvider::Codex);
        assert_eq!(aggregates[1].provider, CodingProvider::Claude);
        assert!(
            aggregates
                .iter()
                .all(|value| value.ranking_day == "2026-08-08")
        );
        assert_eq!(aggregates[0].observed_at, 1_786_192_496_000);
        assert_eq!(
            aggregates[0].api_equivalent_cost.as_ref().unwrap().micros,
            1_250_000
        );
        assert_eq!(
            aggregates[1]
                .api_equivalent_cost
                .as_ref()
                .unwrap()
                .coverage_percent,
            Some(75.0)
        );
    }

    #[test]
    fn prior_day_cache_is_not_relabelled_as_the_current_utc_day() {
        let state = state_with_totals(
            total(
                UsageEvidenceBasis::Mixed,
                120,
                "2026-08-07T23:59:59.000Z",
                Some((
                    1.25,
                    "provider-private-id",
                    ApiEquivalentCostQuality::Reconciled,
                    None,
                )),
            ),
            UsageTotal::Unavailable,
        );

        assert_eq!(current_utc_daily_aggregates(&state, now()).unwrap(), []);
    }

    #[test]
    fn request_has_the_exact_bounded_convex_shape() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 4, aggregate(CodingProvider::Codex, 12, 1000)).unwrap();
        queue_daily_aggregate(&transaction, 4, aggregate(CodingProvider::Claude, 13, 2000))
            .unwrap();
        transaction.commit().unwrap();
        let batch = load_pending_usage_batch(&connection, 4).unwrap().unwrap();
        let value =
            serde_json::to_value(batch.mutation_args(INSTALLATION_CREDENTIAL, now()).unwrap())
                .unwrap();

        assert_eq!(
            value,
            json!({
                "installationCredential": INSTALLATION_CREDENTIAL,
                "activeMacGeneration": 4,
                "snapshots": [
                    {
                        "provider": "claude",
                        "rankingDay": "2026-08-08",
                        "revision": 1,
                        "evidenceBasis": "locally-derived",
                        "coverage": "complete",
                        "observedAt": DAY_START_MILLIS + 2000,
                        "observedTokens": 13,
                        "apiEquivalentCost": null,
                        "correctionReason": null,
                        "correctionRevision": null
                    },
                    {
                        "provider": "codex",
                        "rankingDay": "2026-08-08",
                        "revision": 1,
                        "evidenceBasis": "provider-reported",
                        "coverage": "complete",
                        "observedAt": DAY_START_MILLIS + 1000,
                        "observedTokens": 12,
                        "apiEquivalentCost": null,
                        "correctionReason": null,
                        "correctionRevision": null
                    }
                ]
            })
        );
    }

    #[test]
    fn payload_does_not_copy_private_desktop_fields() {
        let mut state = state_with_totals(
            total(UsageEvidenceBasis::ProviderReported, 120, NOW, None),
            total(UsageEvidenceBasis::LocallyDerived, 80, NOW, None),
        );
        state.providers[0].display_name = "raw-path-/Users/private/session-secret".to_owned();
        state.providers[0].usage.seven_days =
            total(UsageEvidenceBasis::LocallyDerived, 999_999, NOW, None);
        let snapshots = current_utc_daily_aggregates(&state, now()).unwrap();
        let encoded = serde_json::to_string(&snapshots).unwrap();

        for forbidden in [
            "/Users/private",
            "session-secret",
            "private-model-name",
            "private-read-model-revision",
            "999999",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn hostile_and_incomplete_values_fail_closed() {
        let hostile = state_with_totals(
            total(
                UsageEvidenceBasis::ProviderReported,
                10,
                NOW,
                Some((
                    1.0,
                    "/Users/private/session.json",
                    ApiEquivalentCostQuality::Reconciled,
                    None,
                )),
            ),
            UsageTotal::Unavailable,
        );
        assert_eq!(
            current_utc_daily_aggregates(&hostile, now()),
            Err(UsageSyncError::INVALID_VALUE)
        );

        let private_basis = state_with_totals(
            total(
                UsageEvidenceBasis::ProviderReported,
                10,
                NOW,
                Some((
                    1.0,
                    "provider-private-id",
                    ApiEquivalentCostQuality::Reconciled,
                    None,
                )),
            ),
            UsageTotal::Unavailable,
        );
        assert_eq!(
            current_utc_daily_aggregates(&private_basis, now()),
            Err(UsageSyncError::INVALID_VALUE)
        );

        let incomplete = state_with_totals(
            UsageTotal::Current {
                evidence_basis: UsageEvidenceBasis::ProviderReported,
                coverage: UsageCoverage::Complete,
                observed_at: NOW.to_owned(),
                observed_tokens: 10,
                api_equivalent_cost_usd: Some(1.0),
                trend_percent: None,
                trend_previous_tokens: None,
                api_equivalent_cost_basis: None,
                api_equivalent_cost_quality: None,
                api_equivalent_cost_coverage_percent: None,
            },
            UsageTotal::Unavailable,
        );
        assert_eq!(
            current_utc_daily_aggregates(&incomplete, now()),
            Err(UsageSyncError::INVALID_VALUE)
        );

        let mixed = state_with_totals(
            total(UsageEvidenceBasis::Mixed, 10, NOW, None),
            UsageTotal::Unavailable,
        );
        assert_eq!(
            current_utc_daily_aggregates(&mixed, now()),
            Err(UsageSyncError::INVALID_VALUE)
        );
    }

    #[test]
    fn transaction_drop_rolls_back_the_aggregate_and_outbox() {
        let mut connection = connection();
        {
            let transaction = connection.transaction().unwrap();
            for provider in [CodingProvider::Codex, CodingProvider::Claude] {
                queue_daily_aggregate(&transaction, 1, aggregate(provider, 10, 1000)).unwrap();
            }
        }

        let aggregate_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM usage_sync_daily_aggregates",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let outbox_count: i64 = connection
            .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!((aggregate_count, outbox_count), (0, 0));
    }

    #[test]
    fn legacy_outbox_migration_does_not_invent_correction_provenance() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE usage_sync_latest_outbox (
                     active_generation INTEGER NOT NULL,
                     provider TEXT NOT NULL,
                     ranking_day TEXT NOT NULL,
                     revision INTEGER NOT NULL,
                     snapshot_json TEXT NOT NULL,
                     queue_state TEXT NOT NULL,
                     PRIMARY KEY(active_generation, provider, ranking_day)
                 ) STRICT;",
            )
            .unwrap();
        let legacy_snapshot = json!({
            "provider": "claude",
            "rankingDay": "2026-08-08",
            "revision": 2,
            "evidenceBasis": "locally-derived",
            "coverage": "complete",
            "observedAt": DAY_START_MILLIS + 2_000,
            "observedTokens": 80,
            "apiEquivalentCost": null,
            "correctionReason": "parser-correction"
        })
        .to_string();
        connection
            .execute(
                "INSERT INTO usage_sync_latest_outbox(
                     active_generation, provider, ranking_day, revision,
                     snapshot_json, queue_state
                 ) VALUES(1, 'claude', '2026-08-08', 2, ?1, 'active')",
                [&legacy_snapshot],
            )
            .unwrap();

        install_usage_sync_schema(&connection).unwrap();
        let (snapshot_json, reason, correction_revision) = connection
            .query_row(
                "SELECT snapshot_json, correction_reason, correction_revision
                 FROM usage_sync_latest_outbox",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .unwrap();
        let migrated: UsageSyncSnapshot = serde_json::from_str(&snapshot_json).unwrap();
        assert_eq!(migrated.correction_reason, None);
        assert_eq!(migrated.correction_revision, None);
        assert_eq!((reason, correction_revision), (None, None));
        assert!(
            connection
                .execute(
                    "UPDATE usage_sync_latest_outbox
                     SET correction_reason = 'parser-correction'",
                    [],
                )
                .is_err()
        );
    }

    #[test]
    fn one_latest_revision_replaces_older_pending_data_and_survives_retry() {
        let mut connection = connection();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            for (tokens, observed_at) in [(10, 1000), (20, 2000)] {
                let transaction = connection.transaction().unwrap();
                queue_daily_aggregate(&transaction, 1, aggregate(provider, tokens, observed_at))
                    .unwrap();
                transaction.commit().unwrap();
            }
        }

        let first = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let retry = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.snapshots().len(), 2);
        assert!(
            first
                .snapshots()
                .iter()
                .all(|snapshot| snapshot.revision == 2 && snapshot.observed_tokens == 20)
        );
    }

    #[test]
    fn unproved_token_decrease_is_stale_and_typed_correction_is_stored() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let previous = aggregate(CodingProvider::Claude, 100, 1000);
        queue_daily_aggregate(&transaction, 1, previous).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Claude, 90, 2000),)
                .unwrap(),
            QueueUpdate::Stale {
                provider: CodingProvider::Claude,
                revision: 1
            }
        );
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        let corrected = aggregate(CodingProvider::Claude, 90, 2000)
            .with_correction(CorrectionReason::ParserCorrection);
        assert_eq!(
            queue_daily_aggregate(&transaction, 1, corrected).unwrap(),
            QueueUpdate::Stored {
                provider: CodingProvider::Claude,
                revision: 2,
                state: QueueState::Pending
            }
        );
        transaction.commit().unwrap();
    }

    #[test]
    fn provider_replacement_is_derived_from_the_private_aggregate_transition() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let mut local = aggregate(CodingProvider::Codex, 100, 1000);
        local.evidence_basis = SyncEvidenceBasis::LocallyDerived;
        queue_daily_aggregate(&transaction, 1, local).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        assert!(matches!(
            queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 80, 2000),)
                .unwrap(),
            QueueUpdate::Stored { revision: 2, .. }
        ));
        transaction.commit().unwrap();
        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(
            pending.snapshots()[0].correction_reason,
            Some(CorrectionReason::ProviderReplacement)
        );
        assert_eq!(pending.snapshots()[0].correction_revision, Some(2));
    }

    #[test]
    fn parser_correction_marker_reaches_the_latest_cumulative_outbox_revision() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 100, 1000),
        )
        .unwrap();
        transaction.commit().unwrap();

        let state = state_with_totals(
            UsageTotal::Unavailable,
            total(
                UsageEvidenceBasis::LocallyDerived,
                80,
                "2026-08-08T12:34:57Z",
                None,
            ),
        );
        let mut corrections = UsageSyncCorrections::default();
        corrections
            .record_parser_correction(CodingProvider::Claude, 2)
            .unwrap();
        let transaction = connection.transaction().unwrap();
        queue_current_utc_day_with_corrections(&transaction, 1, &state, now(), &corrections)
            .unwrap();
        transaction.commit().unwrap();

        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].revision, 2);
        assert_eq!(pending.snapshots()[0].observed_tokens, 80);
        assert_eq!(
            pending.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
        assert_eq!(pending.snapshots()[0].correction_revision, Some(2));
    }

    #[test]
    fn parser_to_provider_lineage_change_uses_the_new_snapshot_revision() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 100, 1000),
        )
        .unwrap();
        transaction.commit().unwrap();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 80, 2000)
                .with_correction(CorrectionReason::ParserCorrection),
        )
        .unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        let mut provider_reported = aggregate(CodingProvider::Claude, 90, 3000);
        provider_reported.evidence_basis = SyncEvidenceBasis::ProviderReported;
        queue_daily_aggregate(&transaction, 1, provider_reported).unwrap();
        transaction.commit().unwrap();

        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(pending.snapshots()[0].revision, 3);
        assert_eq!(
            pending.snapshots()[0].correction_reason,
            Some(CorrectionReason::ProviderReplacement)
        );
        assert_eq!(pending.snapshots()[0].correction_revision, Some(3));
    }

    #[test]
    fn correction_lineage_waits_for_authority_and_is_consumed_with_the_outbox() {
        let mut connection = connection();
        let mut corrections = UsageSyncCorrections::default();
        corrections
            .record_parser_correction(CodingProvider::Claude, 2)
            .unwrap();
        let transaction = connection.transaction().unwrap();
        stage_usage_sync_corrections(&transaction, now(), &corrections).unwrap();
        transaction.commit().unwrap();

        let corrected_then_increased = state_with_totals(
            UsageTotal::Unavailable,
            total(
                UsageEvidenceBasis::LocallyDerived,
                50,
                "2026-08-08T12:34:57Z",
                None,
            ),
        );
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        queue_current_utc_day(&transaction, 1, &corrected_then_increased, now()).unwrap();
        transaction.commit().unwrap();
        let first = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(
            first.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
        assert_eq!(first.snapshots()[0].correction_revision, Some(1));
        assert_eq!(first.snapshots()[0].observed_tokens, 50);
        assert_eq!(
            connection
                .query_row(
                    "SELECT consumed_generation FROM usage_sync_correction_lineage",
                    [],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap(),
            Some(1)
        );

        let acknowledgement = acknowledgement(
            &first.snapshots()[0],
            AcknowledgementOutcome::Committed,
            first.snapshots()[0].revision,
        );
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &first, &[acknowledgement]).unwrap();
        transaction.commit().unwrap();

        let later_usage = state_with_totals(
            UsageTotal::Unavailable,
            total(
                UsageEvidenceBasis::LocallyDerived,
                60,
                "2026-08-08T12:34:58Z",
                None,
            ),
        );
        let transaction = connection.transaction().unwrap();
        queue_current_utc_day_with_corrections(&transaction, 1, &later_usage, now(), &corrections)
            .unwrap();
        transaction.commit().unwrap();
        let second = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(second.snapshots()[0].observed_tokens, 60);
        assert_eq!(second.snapshots()[0].correction_reason, None);
        assert_eq!(second.snapshots()[0].correction_revision, None);

        let unsupported_decrease = state_with_totals(
            UsageTotal::Unavailable,
            total(
                UsageEvidenceBasis::LocallyDerived,
                40,
                "2026-08-08T12:34:59Z",
                None,
            ),
        );
        let transaction = connection.transaction().unwrap();
        assert!(matches!(
            queue_current_utc_day(&transaction, 1, &unsupported_decrease, now()).unwrap()[0],
            QueueUpdate::Stale { revision: 2, .. }
        ));
        transaction.commit().unwrap();
    }

    #[test]
    fn lost_ack_correction_revision_survives_both_provider_replacements_and_retry() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let mut codex_local = aggregate(CodingProvider::Codex, 100, 1000);
        codex_local.evidence_basis = SyncEvidenceBasis::LocallyDerived;
        queue_daily_aggregate(&transaction, 1, codex_local).unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 100, 1000),
        )
        .unwrap();
        transaction.commit().unwrap();
        let baseline = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let acknowledgements = baseline
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Committed, 1))
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &baseline, &acknowledgements).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 80, 2000)).unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 80, 2000)
                .with_correction(CorrectionReason::ParserCorrection),
        )
        .unwrap();
        transaction.commit().unwrap();
        let corrected = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert!(
            corrected.snapshots().iter().all(|snapshot| {
                snapshot.revision == 2 && snapshot.correction_revision == Some(2)
            })
        );

        // The server commits revision 2, but the client loses the acknowledgement.
        // Reinstalling the schema models the next process start.
        install_usage_sync_schema(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 90, 3000)).unwrap();
        }
        transaction.commit().unwrap();

        let first_retry = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let second_retry = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(first_retry, second_retry);
        for snapshot in first_retry.snapshots() {
            assert_eq!(snapshot.revision, 3);
            assert_eq!(snapshot.observed_tokens, 90);
            assert_eq!(snapshot.correction_revision, Some(2));
            assert_eq!(
                snapshot.correction_reason,
                Some(match snapshot.provider {
                    CodingProvider::Codex => CorrectionReason::ProviderReplacement,
                    CodingProvider::Claude => CorrectionReason::ParserCorrection,
                })
            );
            let stored_pair = connection
                .query_row(
                    "SELECT correction_reason, correction_revision
                     FROM usage_sync_latest_outbox
                     WHERE active_generation = 1 AND provider = ?1",
                    [provider_database_value(snapshot.provider)],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap();
            assert_eq!(
                stored_pair,
                (
                    correction_reason_database_value(snapshot.correction_reason.unwrap())
                        .to_owned(),
                    2
                )
            );
        }

        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            assert_eq!(
                queue_daily_aggregate(&transaction, 1, aggregate(provider, 70, 4000)).unwrap(),
                QueueUpdate::Stale {
                    provider,
                    revision: 3
                }
            );
        }
        transaction.commit().unwrap();
        assert_eq!(
            load_pending_usage_batch(&connection, 1).unwrap().unwrap(),
            first_retry
        );
    }

    #[test]
    fn stale_ack_rebase_keeps_the_original_both_provider_correction_revision() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let mut codex_local = aggregate(CodingProvider::Codex, 100, 1000);
        codex_local.evidence_basis = SyncEvidenceBasis::LocallyDerived;
        queue_daily_aggregate(&transaction, 1, codex_local).unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 100, 1000),
        )
        .unwrap();
        transaction.commit().unwrap();
        let baseline = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let baseline_acknowledgements = baseline
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Committed, 1))
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &baseline, &baseline_acknowledgements).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 80, 2000)).unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 80, 2000)
                .with_correction(CorrectionReason::ParserCorrection),
        )
        .unwrap();
        transaction.commit().unwrap();
        let corrected = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let stale_acknowledgements = corrected
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Stale, 5))
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &corrected, &stale_acknowledgements,)
                .unwrap(),
            2
        );
        transaction.commit().unwrap();

        let rebased = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert!(
            rebased.snapshots().iter().all(|snapshot| {
                snapshot.revision == 6 && snapshot.correction_revision == Some(2)
            })
        );
        install_usage_sync_schema(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 90, 3000)).unwrap();
        }
        transaction.commit().unwrap();
        let cumulative = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert!(
            cumulative.snapshots().iter().all(|snapshot| {
                snapshot.revision == 7 && snapshot.correction_revision == Some(2)
            })
        );
    }

    #[test]
    fn late_acknowledgement_cannot_remove_a_newer_revision() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 10, 1000)).unwrap();
        }
        transaction.commit().unwrap();
        let sent = load_pending_usage_batch(&connection, 1).unwrap().unwrap();

        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 20, 2000)).unwrap();
        }
        transaction.commit().unwrap();

        let acknowledgements = sent
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Committed, 1))
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &acknowledgements).unwrap(),
            0
        );
        transaction.commit().unwrap();

        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 2);
        assert!(
            pending
                .snapshots()
                .iter()
                .all(|snapshot| snapshot.revision == 2)
        );
    }

    #[test]
    fn stale_server_revision_advances_and_requeues_both_provider_floors() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 10, 1000)).unwrap();
        }
        transaction.commit().unwrap();
        let sent = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let acknowledgements = sent
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Stale, 3))
            .collect::<Vec<_>>();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &acknowledgements).unwrap(),
            2
        );
        transaction.commit().unwrap();
        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 2);
        assert!(
            pending
                .snapshots()
                .iter()
                .all(|snapshot| { snapshot.revision == 4 && snapshot.observed_tokens == 10 })
        );
    }

    #[test]
    fn stale_acknowledgement_rebases_a_concurrent_newer_local_row() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 10, 1000)).unwrap();
        }
        transaction.commit().unwrap();
        let sent = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            queue_daily_aggregate(&transaction, 1, aggregate(provider, 20, 2000)).unwrap();
        }
        transaction.commit().unwrap();
        let acknowledgements = sent
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Stale, 3))
            .collect::<Vec<_>>();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &acknowledgements).unwrap(),
            0
        );
        transaction.commit().unwrap();
        let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(pending.snapshots().len(), 2);
        assert!(
            pending
                .snapshots()
                .iter()
                .all(|snapshot| { snapshot.revision == 4 && snapshot.observed_tokens == 20 })
        );
    }

    #[test]
    fn authority_block_and_generation_abandonment_keep_rows() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 10, 1000)).unwrap();
        assert_eq!(
            mark_generation_authority_rejected(&transaction, 1).unwrap(),
            1
        );
        transaction.commit().unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 20, 2000),)
                .unwrap(),
            QueueUpdate::Stored {
                provider: CodingProvider::Codex,
                revision: 2,
                state: QueueState::Blocked
            }
        );
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 2, aggregate(CodingProvider::Claude, 30, 3000))
            .unwrap();
        assert_eq!(activate_generation(&transaction, 2).unwrap(), 1);
        transaction.commit().unwrap();

        let states = connection
            .prepare(
                "SELECT active_generation, queue_state FROM usage_sync_latest_outbox
                 ORDER BY active_generation",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            states,
            vec![(1, "abandoned".to_owned()), (2, "active".to_owned())]
        );
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());
        assert_eq!(
            load_pending_usage_batch(&connection, 2)
                .unwrap()
                .unwrap()
                .snapshots()
                .len(),
            1
        );
    }

    #[test]
    fn pending_batch_is_limited_to_sixty_two_rows() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for day_offset in 0..63 {
            let candidate_now = now() - Duration::days(day_offset);
            let day = candidate_now.date().to_string();
            let mut value = aggregate(CodingProvider::Codex, day_offset as u64, 1000);
            value.ranking_day = day;
            value.observed_at = offset_date_time_millis(candidate_now).unwrap();
            super::queue_daily_aggregate(&transaction, 1, value, candidate_now).unwrap();
        }
        transaction.commit().unwrap();

        let batch = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(batch.snapshots().len(), MAX_USAGE_SYNC_BATCH);
    }

    #[test]
    fn acknowledgement_parser_is_strict_and_bounded() {
        let valid = br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"committed"}]"#;
        assert_eq!(parse_usage_acknowledgements(valid).unwrap().len(), 1);

        for invalid in [
            br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"committed","detail":"private"}]"#.as_slice(),
            br#"[{"provider":"hostile","rankingDay":"2026-08-08","revision":1,"outcome":"committed"}]"#.as_slice(),
            br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"accepted"}]"#.as_slice(),
            br#"[{"provider":"codex","rankingDay":"2026-02-30","revision":1,"outcome":"committed"}]"#.as_slice(),
            br#"[]"#.as_slice(),
        ] {
            assert_eq!(
                parse_usage_acknowledgements(invalid),
                Err(UsageSyncError::INVALID_RESPONSE)
            );
        }

        let oversized = vec![b' '; MAX_ACKNOWLEDGEMENT_BYTES + 1];
        assert_eq!(
            parse_usage_acknowledgements(&oversized),
            Err(UsageSyncError::INVALID_RESPONSE)
        );
    }

    #[test]
    fn acknowledgement_must_cover_the_exact_submitted_batch() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 10, 1000)).unwrap();
        transaction.commit().unwrap();
        let batch = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let wrong = acknowledgement(&batch.snapshots()[0], AcknowledgementOutcome::Committed, 2);

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &batch, &[wrong]),
            Err(UsageSyncError::INVALID_RESPONSE)
        );
        transaction.commit().unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_some());
    }

    #[test]
    fn corrupt_local_payload_does_not_enter_a_request() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 1, aggregate(CodingProvider::Codex, 10, 1000)).unwrap();
        transaction.commit().unwrap();
        connection
            .execute(
                "UPDATE usage_sync_latest_outbox
                 SET snapshot_json = json_set(snapshot_json, '$.privatePath', '/Users/private')",
                [],
            )
            .unwrap();

        assert_eq!(
            load_pending_usage_batch(&connection, 1),
            Err(UsageSyncError::STORAGE_UNAVAILABLE)
        );
    }

    #[test]
    fn credential_validation_matches_the_protected_boundary() {
        let batch = PendingUsageBatch {
            active_mac_generation: 1,
            snapshots: vec![UsageSyncSnapshot::from_aggregate(
                aggregate(CodingProvider::Codex, 10, 1000),
                1,
                None,
            )],
        };
        assert!(batch.mutation_args(INSTALLATION_CREDENTIAL, now()).is_ok());
        assert!(matches!(
            batch.mutation_args("raw-session-secret", now()),
            Err(UsageSyncError::INVALID_VALUE)
        ));
    }

    #[test]
    fn serialized_snapshot_has_only_the_allowlisted_fields() {
        let snapshot =
            UsageSyncSnapshot::from_aggregate(aggregate(CodingProvider::Claude, 10, 1000), 1, None);
        let Value::Object(fields) = serde_json::to_value(snapshot).unwrap() else {
            panic!("snapshot must be an object");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "apiEquivalentCost",
                "correctionReason",
                "correctionRevision",
                "coverage",
                "evidenceBasis",
                "observedAt",
                "observedTokens",
                "provider",
                "rankingDay",
                "revision",
            ])
        );
    }
}
