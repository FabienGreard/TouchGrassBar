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
use time::{Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::daily_usage_aggregate::project_retained_cost;
use crate::providers::{
    PROVIDER_REGISTRY, ProviderCorrection, ProviderDailyUsage, load_daily_usage_history,
};
use crate::sanitized::{
    ApiEquivalentCostQuality, CodingProvider, SanitizedDesktopStateV3, UsageCoverage,
    UsageEvidenceBasis, UsageTotal,
};

pub(crate) const MAX_USAGE_SYNC_BATCH: usize = 62;
const MAX_TRANSFER_DAY_CARRYOVER_MARKERS: usize = 2;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_PRICING_BASIS_BYTES: usize = 256;
const MAX_LOCAL_VALUE_BYTES: usize = 4_096;
const MAX_ACKNOWLEDGEMENT_BYTES: usize = 64 * 1_024;
const INSTALLATION_CREDENTIAL_BYTES: usize = 52;
const FUTURE_OBSERVATION_TOLERANCE_MILLIS: u64 = 5 * 60 * 1_000;
const USAGE_HISTORY_RETENTION_DAYS: i64 = 60;

const GENERATION_ACTIVE: &str = "active";
const GENERATION_BLOCKED: &str = "blocked";
const GENERATION_ABANDONED: &str = "abandoned";
const SETTINGS_PENDING: &str = "pending";
const SETTINGS_SYNCED: &str = "synced";
const SETTINGS_BLOCKED: &str = "blocked";
const SETTINGS_ABANDONED: &str = "abandoned";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationOneProfileBackfillState {
    Pending { activated_at: u64 },
    Complete,
}

fn generation_one_profile_backfill_state(
    stored_activated_at: u64,
    stored_completed: i64,
) -> Result<GenerationOneProfileBackfillState, UsageSyncError> {
    validate_safe_integer(stored_activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(stored_activated_at) * 1_000_000)
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    match stored_completed {
        0 => Ok(GenerationOneProfileBackfillState::Pending {
            activated_at: stored_activated_at,
        }),
        1 => Ok(GenerationOneProfileBackfillState::Complete),
        _ => Err(UsageSyncError::STORAGE_UNAVAILABLE),
    }
}

pub(crate) fn generation_one_profile_backfill_is_pending(
    connection: &Connection,
) -> Result<bool, UsageSyncError> {
    let activation = connection
        .query_row(
            "SELECT activated_at, profile_backfill_completed
             FROM usage_sync_generation_activations
             WHERE active_generation = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((stored_activated_at, stored_completed)) = activation else {
        return Ok(false);
    };
    let stored_activated_at =
        u64::try_from(stored_activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    Ok(matches!(
        generation_one_profile_backfill_state(stored_activated_at, stored_completed)?,
        GenerationOneProfileBackfillState::Pending { .. }
    ))
}

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

    pub(crate) fn merge_from(&mut self, other: &Self) {
        for (provider, correction) in &other.0 {
            let current = self.0.entry(*provider).or_insert(*correction);
            if correction.source_revision > current.source_revision {
                *current = *correction;
            }
        }
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
    Conflict,
    Idempotent,
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UsageSyncAcknowledgement {
    pub(crate) provider: CodingProvider,
    pub(crate) ranking_day: String,
    /// This is the revision that resolved the submitted stale payload.
    pub(crate) revision: u64,
    pub(crate) outcome: AcknowledgementOutcome,
}

impl UsageSyncAcknowledgement {
    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_ranking_day(&self.ranking_day).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
        validate_revision(self.revision).map_err(|_| UsageSyncError::INVALID_RESPONSE)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ProviderSettingsAcknowledgement {
    pub(crate) revision: u64,
    pub(crate) outcome: AcknowledgementOutcome,
}

impl ProviderSettingsAcknowledgement {
    fn validate(&self) -> Result<(), UsageSyncError> {
        validate_revision(self.revision).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
        if self.outcome == AcknowledgementOutcome::Conflict {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageSyncAcknowledgements {
    pub(crate) provider_settings: Option<ProviderSettingsAcknowledgement>,
    pub(crate) usage: Vec<UsageSyncAcknowledgement>,
    /// True only after `sync:dailyUsage` returns a parsed success value.
    pub(crate) usage_mutation_completed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderSettingsSnapshot {
    revision: u64,
    enabled_providers: Vec<CodingProvider>,
}

impl ProviderSettingsSnapshot {
    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub(crate) fn enabled_providers(&self) -> &[CodingProvider] {
        &self.enabled_providers
    }
}

/// A sanitized batch that is safe to give to the protected transport adapter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingUsageBatch {
    active_mac_generation: u64,
    provider_settings: Option<ProviderSettingsSnapshot>,
    snapshots: Vec<UsageSyncSnapshot>,
    transfer_day_carryover: Option<TransferDayCarryover>,
    profile_backfill_anchor: Option<String>,
    retained_history: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct TransferDayCarryover {
    ranking_day: String,
    activated_at: u64,
    kind: TransferDayCarryoverKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferDayCarryoverKind {
    DelayedInstallationMarker,
    PendingSegment,
}

impl TransferDayCarryoverKind {
    fn as_database_value(self) -> &'static str {
        match self {
            Self::DelayedInstallationMarker => "delayed-installation-marker",
            Self::PendingSegment => "pending-segment",
        }
    }

    fn from_database_value(value: &str) -> Result<Self, UsageSyncError> {
        match value {
            "delayed-installation-marker" => Ok(Self::DelayedInstallationMarker),
            "pending-segment" => Ok(Self::PendingSegment),
            _ => Err(UsageSyncError::STORAGE_UNAVAILABLE),
        }
    }
}

impl PendingUsageBatch {
    pub(crate) fn active_mac_generation(&self) -> u64 {
        self.active_mac_generation
    }

    #[cfg(test)]
    pub(crate) fn provider_settings(&self) -> Option<&ProviderSettingsSnapshot> {
        self.provider_settings.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn snapshots(&self) -> &[UsageSyncSnapshot] {
        &self.snapshots
    }

    pub(crate) fn has_usage_snapshots(&self) -> bool {
        !self.snapshots.is_empty()
    }

    pub(crate) fn requires_usage_mutation(&self) -> bool {
        self.has_usage_snapshots() || self.profile_backfill_anchor.is_some()
    }

    pub(crate) fn is_empty_profile_backfill(&self) -> bool {
        self.snapshots.is_empty() && self.profile_backfill_anchor.is_some()
    }

    pub(crate) fn has_successful_current_day_acknowledgement(
        &self,
        acknowledgements: &[UsageSyncAcknowledgement],
        now: OffsetDateTime,
    ) -> bool {
        let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
        acknowledgements.iter().any(|acknowledgement| {
            acknowledgement.ranking_day == ranking_day
                && acknowledgement.outcome != AcknowledgementOutcome::Conflict
                && self.snapshots.iter().any(|snapshot| {
                    snapshot.provider == acknowledgement.provider
                        && snapshot.ranking_day == acknowledgement.ranking_day
                })
        })
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
        if let Some(carryover) = self.transfer_day_carryover.as_ref() {
            validate_transfer_day_carryover_batch(&self.snapshots, carryover, now)?;
        } else if let Some(anchor_day) = self.profile_backfill_anchor.as_deref() {
            validate_profile_backfill_batch(&self.snapshots, anchor_day, now)?;
        } else if self.retained_history {
            validate_retained_history_batch(&self.snapshots, now)?;
        } else {
            validate_current_day_batch(&self.snapshots, now)?;
        }
        Ok(UsageSyncMutationArgs {
            installation_credential,
            active_mac_generation: self.active_mac_generation,
            profile_backfill_anchor: self.profile_backfill_anchor.as_deref(),
            snapshots: &self.snapshots,
        })
    }

    pub(crate) fn provider_settings_mutation_args<'a>(
        &'a self,
        installation_credential: &'a str,
    ) -> Result<Option<ProviderSettingsMutationArgs<'a>>, UsageSyncError> {
        if !valid_installation_credential(installation_credential) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        validate_generation(self.active_mac_generation)?;
        let Some(settings) = self.provider_settings.as_ref() else {
            return Ok(None);
        };
        validate_revision(settings.revision)?;
        validate_enabled_providers(&settings.enabled_providers)?;
        Ok(Some(ProviderSettingsMutationArgs {
            installation_credential,
            active_mac_generation: self.active_mac_generation,
            revision: settings.revision,
            enabled_providers: &settings.enabled_providers,
        }))
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
    profile_backfill_anchor: Option<&'a str>,
    snapshots: &'a [UsageSyncSnapshot],
}

/// Exact arguments for the `sync:providerSettings` Convex mutation.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderSettingsMutationArgs<'a> {
    installation_credential: &'a str,
    active_mac_generation: u64,
    revision: u64,
    enabled_providers: &'a [CodingProvider],
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

         CREATE TABLE IF NOT EXISTS usage_sync_generation_baselines (
             active_generation INTEGER NOT NULL,
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             aggregate_json TEXT NOT NULL CHECK(length(aggregate_json) <= 4096),
             PRIMARY KEY(active_generation, provider, ranking_day),
             FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS usage_sync_generation_activations (
             active_generation INTEGER PRIMARY KEY,
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             activated_at INTEGER NOT NULL
                 CHECK(activated_at >= 0 AND activated_at <= 9007199254740991),
             profile_backfill_completed INTEGER NOT NULL DEFAULT 0
                 CHECK(
                     profile_backfill_completed IN (0, 1)
                     AND (profile_backfill_completed = 0 OR active_generation = 1)
                 ),
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
                     AND correction_revision IS NOT NULL
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

         CREATE TABLE IF NOT EXISTS usage_sync_transfer_day_carryovers (
             active_generation INTEGER NOT NULL,
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             carryover_kind TEXT NOT NULL CHECK(carryover_kind IN (
                 'delayed-installation-marker', 'pending-segment'
             )),
             PRIMARY KEY(active_generation, provider, ranking_day),
             FOREIGN KEY(active_generation, provider, ranking_day)
                 REFERENCES usage_sync_latest_outbox(
                     active_generation, provider, ranking_day
                 ) ON DELETE CASCADE
         ) STRICT;

         CREATE TABLE IF NOT EXISTS usage_sync_terminal_conflicts (
             active_generation INTEGER NOT NULL,
             provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
             ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
             revision INTEGER NOT NULL
                 CHECK(revision >= 1 AND revision <= 9007199254740991),
             PRIMARY KEY(active_generation, provider, ranking_day, revision),
             FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS usage_sync_provider_settings_outbox (
             active_generation INTEGER PRIMARY KEY,
             revision INTEGER NOT NULL
                 CHECK(revision >= 1 AND revision <= 9007199254740991),
             codex_enabled INTEGER NOT NULL CHECK(codex_enabled IN (0, 1)),
             claude_enabled INTEGER NOT NULL CHECK(claude_enabled IN (0, 1)),
             delivery_state TEXT NOT NULL
                 CHECK(delivery_state IN ('pending', 'synced', 'blocked', 'abandoned')),
             FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
         ) STRICT;

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
    Ok(())
}

/// Add the generation-one Profile completion state to the released v6 schema.
///
/// SQLite keeps every existing activation row and gives it the pending value.
/// The caller owns the transaction and the sanitized read-model version write.
pub(crate) fn migrate_usage_sync_schema_from_v6(
    connection: &Connection,
) -> Result<(), UsageSyncError> {
    connection.execute_batch(
        "ALTER TABLE usage_sync_generation_activations
         ADD COLUMN profile_backfill_completed INTEGER NOT NULL DEFAULT 0
         CHECK(
             profile_backfill_completed IN (0, 1)
             AND (profile_backfill_completed = 0 OR active_generation = 1)
         );",
    )?;
    install_usage_sync_schema(connection)
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
        "DELETE FROM usage_sync_correction_lineage
         WHERE ranking_day != ?1 AND consumed_generation IS NULL",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParserCorrectionLineage {
    source_revision: u64,
    consumed_generation: Option<u64>,
}

fn load_parser_correction_lineage(
    connection: &Connection,
    provider: CodingProvider,
    ranking_day: &str,
) -> Result<Option<ParserCorrectionLineage>, UsageSyncError> {
    validate_ranking_day(ranking_day)?;
    let lineage = connection
        .query_row(
            "SELECT source_revision, consumed_generation
             FROM usage_sync_correction_lineage
             WHERE provider = ?1 AND ranking_day = ?2
               AND reason = 'parser-correction'",
            params![provider_database_value(provider), ranking_day],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?;
    lineage
        .map(|(source_revision, consumed_generation)| {
            let source_revision =
                u64::try_from(source_revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
            validate_revision(source_revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
            let consumed_generation = consumed_generation
                .map(|generation| {
                    let generation = u64::try_from(generation)
                        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
                    validate_generation(generation)
                        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
                    Ok::<u64, UsageSyncError>(generation)
                })
                .transpose()?;
            Ok(ParserCorrectionLineage {
                source_revision,
                consumed_generation,
            })
        })
        .transpose()
}

fn record_parser_correction_source_revision(
    transaction: &Transaction<'_>,
    provider: CodingProvider,
    ranking_day: &str,
    source_revision: u64,
    active_mac_generation: u64,
) -> Result<(), UsageSyncError> {
    validate_ranking_day(ranking_day)?;
    validate_revision(source_revision)?;
    validate_generation(active_mac_generation)?;
    transaction.execute(
        "INSERT INTO usage_sync_correction_lineage(
             provider, ranking_day, source_revision, reason, consumed_generation
         ) VALUES(?1, ?2, ?3, 'parser-correction', ?4)
         ON CONFLICT(provider, ranking_day) DO UPDATE SET
             source_revision=excluded.source_revision,
             consumed_generation=CASE
                 WHEN excluded.source_revision > usage_sync_correction_lineage.source_revision
                     THEN excluded.consumed_generation
                 WHEN usage_sync_correction_lineage.consumed_generation IS NULL
                     THEN excluded.consumed_generation
                 ELSE usage_sync_correction_lineage.consumed_generation
             END
         WHERE excluded.source_revision >= usage_sync_correction_lineage.source_revision",
        params![
            provider_database_value(provider),
            ranking_day,
            to_database_integer(source_revision)?,
            to_database_integer(active_mac_generation)?
        ],
    )?;
    Ok(())
}

fn unconsumed_parser_correction_revision(
    connection: &Connection,
    daily: &ProviderDailyUsage,
) -> Result<Option<u64>, UsageSyncError> {
    let Some(ProviderCorrection::ParserCorrection { source_revision }) = daily.correction else {
        return Ok(None);
    };
    validate_revision(source_revision)?;
    let ranking_day = daily.day.to_string();
    let known = load_parser_correction_lineage(connection, daily.provider, &ranking_day)?;
    Ok(known
        .is_none_or(|lineage| {
            source_revision > lineage.source_revision
                || (source_revision == lineage.source_revision
                    && lineage.consumed_generation.is_none())
        })
        .then_some(source_revision))
}

fn record_parser_correction_if_queued(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    daily: &ProviderDailyUsage,
    source_revision: Option<u64>,
    update: &QueueUpdate,
) -> Result<(), UsageSyncError> {
    if let Some(source_revision) = source_revision
        && matches!(
            update,
            QueueUpdate::Stored { .. } | QueueUpdate::Unchanged { .. }
        )
    {
        record_parser_correction_source_revision(
            transaction,
            daily.provider,
            &daily.day.to_string(),
            source_revision,
            active_mac_generation,
        )?;
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
    let enabled_providers = state
        .providers
        .iter()
        .map(|presentation| presentation.provider)
        .collect();
    current_utc_daily_aggregates_with_corrections(
        state,
        now,
        &UsageSyncCorrections::default(),
        &enabled_providers,
    )
}

fn current_utc_daily_aggregates_with_corrections(
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    corrections: &UsageSyncCorrections,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Vec<DailyUsageAggregate>, UsageSyncError> {
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    let mut seen = BTreeSet::new();
    let mut aggregates = Vec::new();
    for presentation in &state.providers {
        if !seen.insert(presentation.provider) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        if !enabled_providers.contains(&presentation.provider) {
            continue;
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

fn abandoned_transfer_day_aggregates(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    ranking_day: &str,
) -> Result<Vec<DailyUsageAggregate>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    validate_ranking_day(ranking_day)?;
    let generation = to_database_integer(active_mac_generation)?;
    let mut statement = transaction.prepare(
        "SELECT aggregates.provider, aggregates.aggregate_json
         FROM usage_sync_daily_aggregates AS aggregates
         JOIN usage_sync_latest_outbox AS outbox
           ON outbox.active_generation = aggregates.active_generation
          AND outbox.provider = aggregates.provider
          AND outbox.ranking_day = aggregates.ranking_day
         WHERE outbox.active_generation < ?1
           AND outbox.ranking_day = ?2
           AND outbox.queue_state = 'abandoned'
           AND outbox.active_generation = (
               SELECT max(candidate.active_generation)
               FROM usage_sync_latest_outbox AS candidate
               WHERE candidate.active_generation < ?1
                 AND candidate.provider = outbox.provider
                 AND candidate.ranking_day = ?2
                 AND candidate.queue_state = 'abandoned'
           )
         ORDER BY aggregates.provider
         LIMIT 2",
    )?;
    let rows = statement.query_map(params![generation, ranking_day], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut aggregates = Vec::with_capacity(MAX_TRANSFER_DAY_CARRYOVER_MARKERS);
    for row in rows {
        let (provider, aggregate_json) = row?;
        if aggregate_json.len() > MAX_LOCAL_VALUE_BYTES {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let expected_provider = provider_from_database_value(&provider)?;
        let aggregate: DailyUsageAggregate = serde_json::from_str(&aggregate_json)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        aggregate
            .validate()
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if aggregate.provider != expected_provider || aggregate.ranking_day != ranking_day {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        aggregates.push(aggregate);
    }
    Ok(aggregates)
}

fn queue_transfer_day_carryover_markers(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    for abandoned in
        abandoned_transfer_day_aggregates(transaction, active_mac_generation, ranking_day)?
    {
        let marker = DailyUsageAggregate {
            provider: abandoned.provider,
            ranking_day: ranking_day.to_owned(),
            evidence_basis: abandoned.evidence_basis,
            coverage: SyncCoverage::Partial,
            observed_at: activated_at,
            observed_tokens: 0,
            api_equivalent_cost: None,
            correction_reason: None,
        };
        validate_transfer_day_carryover_marker(&marker, ranking_day, activated_at)?;
        let update = queue_validated_daily_aggregate(transaction, active_mac_generation, marker)?;
        if !matches!(
            update,
            QueueUpdate::Stored {
                revision: 1,
                state: QueueState::Pending,
                ..
            }
        ) {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let stored = load_outbox_snapshot(
            transaction,
            active_mac_generation,
            abandoned.provider,
            ranking_day,
        )?
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)?;
        validate_delayed_installation_marker_snapshot(&stored, ranking_day, activated_at)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        transaction.execute(
            "INSERT OR IGNORE INTO usage_sync_transfer_day_carryovers(
                 active_generation, provider, ranking_day, carryover_kind
             )
             SELECT ?1, ?2, ?3, 'delayed-installation-marker'
             WHERE EXISTS (
                 SELECT 1 FROM usage_sync_latest_outbox
                 WHERE active_generation = ?1
                   AND provider = ?2
                   AND ranking_day = ?3
                   AND revision = 1
                   AND queue_state = 'active'
             )",
            params![
                to_database_integer(active_mac_generation)?,
                provider_database_value(abandoned.provider),
                ranking_day
            ],
        )?;
    }
    Ok(())
}

fn link_pending_transfer_day_segments(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    validate_generation(active_mac_generation)?;
    validate_ranking_day(ranking_day)?;
    validate_safe_integer(activated_at)?;
    let generation = to_database_integer(active_mac_generation)?;
    let mut statement = transaction.prepare(
        "SELECT outbox.provider, outbox.revision, outbox.snapshot_json,
                outbox.correction_reason, outbox.correction_revision
         FROM usage_sync_latest_outbox AS outbox
         WHERE outbox.active_generation = ?1
           AND outbox.ranking_day = ?2
           AND outbox.queue_state = 'active'
           AND NOT EXISTS (
               SELECT 1
               FROM usage_sync_terminal_conflicts AS terminal_conflict
               WHERE terminal_conflict.active_generation = outbox.active_generation
                 AND terminal_conflict.provider = outbox.provider
                 AND terminal_conflict.ranking_day = outbox.ranking_day
                 AND terminal_conflict.revision = outbox.revision
           )
         ORDER BY outbox.provider
         LIMIT 2",
    )?;
    let rows = statement.query_map(params![generation, ranking_day], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    for row in rows {
        let (provider, revision, snapshot_json, correction_reason, correction_revision) = row?;
        if snapshot_json.len() > MAX_LOCAL_VALUE_BYTES {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let expected_provider = provider_from_database_value(&provider)?;
        let revision = u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        let snapshot: UsageSyncSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        snapshot
            .validate()
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if snapshot.provider != expected_provider
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
        if validate_transfer_day_pending_segment(&snapshot, ranking_day, activated_at).is_err() {
            continue;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO usage_sync_transfer_day_carryovers(
                 active_generation, provider, ranking_day, carryover_kind
             )
             SELECT ?1, ?2, ?3, 'pending-segment'
             WHERE EXISTS (
                 SELECT 1 FROM usage_sync_latest_outbox AS outbox
                 WHERE outbox.active_generation = ?1
                   AND outbox.provider = ?2
                   AND outbox.ranking_day = ?3
                   AND outbox.revision = ?4
                   AND outbox.queue_state = 'active'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM usage_sync_terminal_conflicts AS terminal_conflict
                       WHERE terminal_conflict.active_generation = outbox.active_generation
                         AND terminal_conflict.provider = outbox.provider
                         AND terminal_conflict.ranking_day = outbox.ranking_day
                         AND terminal_conflict.revision = outbox.revision
                   )
             )",
            params![
                generation,
                provider,
                ranking_day,
                to_database_integer(revision)?
            ],
        )?;
    }
    Ok(())
}

/// Store cumulative provider totals observed at the exact transfer boundary.
/// A stale pre-transfer total cannot become a baseline. The first later
/// observation becomes a partial baseline instead. A delayed installation
/// after UTC midnight stores a zero-token partial marker for abandoned
/// transfer-day usage. A partial row that was already pending stays eligible
/// after rollover. Generation one has no earlier Active Mac contribution.
pub(crate) fn capture_generation_baselines(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    activated_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let ranking_day = activated_at.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    let generation = to_database_integer(active_mac_generation)?;
    let requested_activated_at = offset_date_time_millis(activated_at)?;
    let database_activated_at = to_database_integer(requested_activated_at)?;
    let activation_inserted = transaction.execute(
        "INSERT INTO usage_sync_generation_activations(
             active_generation, ranking_day, activated_at
         ) VALUES(?1, ?2, ?3)
         ON CONFLICT(active_generation) DO NOTHING",
        params![generation, ranking_day, database_activated_at],
    )? == 1;
    let (activation_day, stored_activated_at, profile_backfill_completed) = transaction.query_row(
        "SELECT ranking_day, activated_at, profile_backfill_completed
         FROM usage_sync_generation_activations
         WHERE active_generation = ?1",
        [generation],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    validate_ranking_day(&activation_day).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let stored_activated_at =
        u64::try_from(stored_activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    validate_safe_integer(stored_activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if activation_day != ranking_day {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    if active_mac_generation == 1 {
        return match generation_one_profile_backfill_state(
            stored_activated_at,
            profile_backfill_completed,
        )? {
            GenerationOneProfileBackfillState::Pending { activated_at }
                if activated_at == requested_activated_at =>
            {
                Ok(())
            }
            GenerationOneProfileBackfillState::Complete => Ok(()),
            GenerationOneProfileBackfillState::Pending { .. } => {
                Err(UsageSyncError::STORAGE_UNAVAILABLE)
            }
        };
    }
    if stored_activated_at != requested_activated_at {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    if activated_at.to_offset(UtcOffset::UTC).date() != now.to_offset(UtcOffset::UTC).date() {
        if activation_inserted {
            queue_transfer_day_carryover_markers(
                transaction,
                active_mac_generation,
                &ranking_day,
                requested_activated_at,
            )?;
        } else {
            link_pending_transfer_day_segments(
                transaction,
                active_mac_generation,
                &ranking_day,
                requested_activated_at,
            )?;
        }
        return Ok(());
    }
    let providers = state
        .providers
        .iter()
        .map(|presentation| presentation.provider)
        .collect();
    for mut aggregate in current_utc_daily_aggregates_with_corrections(
        state,
        activated_at,
        &UsageSyncCorrections::default(),
        &providers,
    )? {
        if aggregate.observed_at != requested_activated_at {
            continue;
        }
        aggregate.correction_reason = None;
        let abandoned_transfer_usage = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM usage_sync_latest_outbox
                 WHERE active_generation < ?1
                   AND provider = ?2
                   AND ranking_day = ?3
                   AND queue_state = 'abandoned'
             )",
            params![
                generation,
                provider_database_value(aggregate.provider),
                ranking_day
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if abandoned_transfer_usage {
            aggregate.coverage = SyncCoverage::Partial;
        }
        let provider = provider_database_value(aggregate.provider);
        let aggregate_json = encode_local_value(&aggregate)?;
        transaction.execute(
            "INSERT INTO usage_sync_generation_baselines(
                 active_generation, provider, ranking_day, aggregate_json
             ) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(active_generation, provider, ranking_day) DO NOTHING",
            params![
                to_database_integer(active_mac_generation)?,
                provider,
                aggregate.ranking_day,
                aggregate_json
            ],
        )?;
    }
    Ok(())
}

enum GenerationSegment {
    BaselineOnly,
    Queue(DailyUsageAggregate),
}

fn generation_segment(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    mut aggregate: DailyUsageAggregate,
) -> Result<GenerationSegment, UsageSyncError> {
    if active_mac_generation == 1 {
        return Ok(GenerationSegment::Queue(aggregate));
    }
    let provider = provider_database_value(aggregate.provider);
    let generation = to_database_integer(active_mac_generation)?;
    let activation = transaction
        .query_row(
            "SELECT ranking_day, activated_at FROM usage_sync_generation_activations
             WHERE active_generation = ?1",
            [generation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((activation_day, activated_at)) = activation else {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    };
    validate_ranking_day(&activation_day).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let activated_at =
        u64::try_from(activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    validate_safe_integer(activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if aggregate.ranking_day != activation_day {
        return Ok(GenerationSegment::Queue(aggregate));
    }
    let baseline_json = transaction
        .query_row(
            "SELECT aggregate_json
             FROM usage_sync_generation_baselines
             WHERE active_generation = ?1 AND provider = ?2 AND ranking_day = ?3",
            params![generation, provider, aggregate.ranking_day],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(baseline_json) = baseline_json else {
        if aggregate.observed_at < activated_at {
            return Ok(GenerationSegment::BaselineOnly);
        }
        let mut baseline = aggregate.clone();
        baseline.correction_reason = None;
        baseline.coverage = SyncCoverage::Partial;
        let aggregate_json = encode_local_value(&baseline)?;
        transaction.execute(
            "INSERT INTO usage_sync_generation_baselines(
                 active_generation, provider, ranking_day, aggregate_json
             ) VALUES(?1, ?2, ?3, ?4)",
            params![generation, provider, baseline.ranking_day, aggregate_json],
        )?;
        aggregate.correction_reason = None;
        aggregate.coverage = SyncCoverage::Partial;
        aggregate.observed_tokens = 0;
        aggregate.api_equivalent_cost = None;
        aggregate.validate()?;
        return Ok(GenerationSegment::Queue(aggregate));
    };
    if baseline_json.len() > MAX_LOCAL_VALUE_BYTES {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let baseline: DailyUsageAggregate =
        serde_json::from_str(&baseline_json).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    baseline
        .validate()
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if baseline.provider != aggregate.provider || baseline.ranking_day != aggregate.ranking_day {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    if aggregate.observed_at < baseline.observed_at {
        return Ok(GenerationSegment::BaselineOnly);
    }
    if aggregate.correction_reason.is_none()
        && baseline.evidence_basis == SyncEvidenceBasis::LocallyDerived
        && aggregate.evidence_basis == SyncEvidenceBasis::ProviderReported
        && aggregate.observed_tokens < baseline.observed_tokens
    {
        aggregate.correction_reason = Some(CorrectionReason::ProviderReplacement);
    }
    let observed_tokens = aggregate
        .observed_tokens
        .saturating_sub(baseline.observed_tokens);
    let existing = load_aggregate(
        transaction,
        active_mac_generation,
        aggregate.provider,
        &aggregate.ranking_day,
    )?;
    // A partial transfer baseline still needs a zero marker. The marker keeps
    // the new generation in the transfer-day history after UTC rollover.
    if observed_tokens == 0 && existing.is_none() && baseline.coverage == SyncCoverage::Complete {
        return Ok(GenerationSegment::BaselineOnly);
    }
    aggregate.coverage = if aggregate.coverage == SyncCoverage::Complete
        && baseline.coverage == SyncCoverage::Complete
    {
        SyncCoverage::Complete
    } else {
        SyncCoverage::Partial
    };
    aggregate.api_equivalent_cost = segment_cost(&baseline, &aggregate, observed_tokens);
    aggregate.observed_tokens = observed_tokens;
    aggregate.validate()?;
    Ok(GenerationSegment::Queue(aggregate))
}

fn segment_cost(
    baseline: &DailyUsageAggregate,
    candidate: &DailyUsageAggregate,
    segment_tokens: u64,
) -> Option<SyncApiEquivalentCost> {
    if segment_tokens == 0 {
        return None;
    }
    let baseline_cost = baseline.api_equivalent_cost.as_ref()?;
    let candidate_cost = candidate.api_equivalent_cost.as_ref()?;
    if baseline_cost.pricing_basis != candidate_cost.pricing_basis
        || baseline_cost.quality != candidate_cost.quality
    {
        return None;
    }
    let micros = candidate_cost.micros.checked_sub(baseline_cost.micros)?;
    let coverage_percent = match candidate_cost.quality {
        SyncCostQuality::Reconciled | SyncCostQuality::LocalOnly => None,
        SyncCostQuality::Modeled => {
            let baseline_coverage = baseline_cost.coverage_percent?;
            let candidate_coverage = candidate_cost.coverage_percent?;
            let baseline_covered = baseline.observed_tokens as f64 * baseline_coverage / 100.0;
            let candidate_covered = candidate.observed_tokens as f64 * candidate_coverage / 100.0;
            let segment_covered = candidate_covered - baseline_covered;
            if !segment_covered.is_finite()
                || segment_covered < 0.0
                || segment_covered > segment_tokens as f64
            {
                return None;
            }
            Some(segment_covered * 100.0 / segment_tokens as f64)
        }
    };
    Some(SyncApiEquivalentCost {
        micros,
        pricing_basis: candidate_cost.pricing_basis.clone(),
        quality: candidate_cost.quality,
        coverage_percent,
    })
}

/// The caller states why it needs a new ledger projection. This module owns
/// the history window, provider scope, and queue order for that request.
#[derive(Clone, Copy)]
pub(crate) enum UsageQueueRequest<'a> {
    Refresh(&'a UsageSyncCorrections),
    ProfileActivation { anchor_day: time::Date },
    AfterAcknowledgement,
}

/// Project all eligible provider-day facts in one caller-owned transaction.
pub(crate) fn queue_usage_for_commit(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    enabled_providers: &BTreeSet<CodingProvider>,
    request: UsageQueueRequest<'_>,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let profile_backfill_is_pending =
        active_mac_generation == 1 && generation_one_profile_backfill_is_pending(transaction)?;
    let mut updates = Vec::new();

    match request {
        UsageQueueRequest::ProfileActivation { anchor_day } if active_mac_generation == 1 => {
            let history = load_daily_usage_history(transaction, now, anchor_day, 30)
                .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
            updates.extend(queue_profile_backfill(
                transaction,
                active_mac_generation,
                &history,
                anchor_day,
                now,
            )?);
        }
        UsageQueueRequest::Refresh(_) if active_mac_generation == 1 => {
            if !profile_backfill_is_pending {
                let today = now.to_offset(UtcOffset::UTC).date();
                let history = load_daily_usage_history(transaction, now, today, 60)
                    .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
                updates.extend(queue_retained_history_corrections(
                    transaction,
                    active_mac_generation,
                    &history,
                    now,
                    enabled_providers,
                )?);
            }
        }
        UsageQueueRequest::Refresh(_)
        | UsageQueueRequest::ProfileActivation { .. }
        | UsageQueueRequest::AfterAcknowledgement => {}
    }

    let all_providers = PROVIDER_REGISTRY
        .iter()
        .map(|descriptor| descriptor.provider)
        .collect::<BTreeSet<_>>();
    let current_providers = if profile_backfill_is_pending {
        &all_providers
    } else {
        enabled_providers
    };
    let no_corrections = UsageSyncCorrections::default();
    let corrections = match request {
        UsageQueueRequest::Refresh(corrections) => corrections,
        UsageQueueRequest::ProfileActivation { .. } | UsageQueueRequest::AfterAcknowledgement => {
            &no_corrections
        }
    };
    updates.extend(queue_current_utc_day_with_corrections(
        transaction,
        active_mac_generation,
        state,
        now,
        corrections,
        current_providers,
    )?);
    Ok(updates)
}

/// Store all current-day candidates in one caller-owned transaction.
#[cfg(test)]
fn queue_current_utc_day(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    queue_current_utc_day_with_corrections(
        transaction,
        active_mac_generation,
        state,
        now,
        &UsageSyncCorrections::default(),
        enabled_providers,
    )
}

fn queue_current_utc_day_with_corrections(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    state: &SanitizedDesktopStateV3,
    now: OffsetDateTime,
    corrections: &UsageSyncCorrections,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    prune_expired_usage_sync_rows(transaction, now)?;
    stage_usage_sync_corrections(transaction, now, corrections)?;
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    let staged = load_staged_usage_sync_corrections(transaction, &ranking_day)?;
    let aggregates =
        current_utc_daily_aggregates_with_corrections(state, now, &staged, enabled_providers)?;
    let mut updates = Vec::with_capacity(aggregates.len());
    for aggregate in aggregates {
        let provider = aggregate.provider;
        let ranking_day = aggregate.ranking_day.clone();
        let aggregate = match generation_segment(transaction, active_mac_generation, aggregate)? {
            GenerationSegment::BaselineOnly => {
                if let Some(correction) = staged.0.get(&provider).copied() {
                    consume_staged_usage_sync_correction(
                        transaction,
                        provider,
                        &ranking_day,
                        correction,
                        active_mac_generation,
                    )?;
                }
                continue;
            }
            GenerationSegment::Queue(aggregate) => aggregate,
        };
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

/// Store the one sparse generation-one Profile backfill in the caller's transaction.
fn queue_profile_backfill(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    history: &[ProviderDailyUsage],
    anchor_day: time::Date,
    now: OffsetDateTime,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    if active_mac_generation != 1 || history.len() > 60 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    if anchor_day > now.to_offset(UtcOffset::UTC).date() {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    validate_generation(active_mac_generation)?;
    let anchor_day_value = anchor_day.to_string();
    let stored_activation = transaction
        .query_row(
            "SELECT ranking_day, activated_at, profile_backfill_completed
             FROM usage_sync_generation_activations
             WHERE active_generation = ?1",
            [to_database_integer(active_mac_generation)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((stored_anchor, stored_activation, profile_backfill_completed)) = stored_activation
    else {
        return Err(UsageSyncError::INVALID_VALUE);
    };
    let stored_activation =
        u64::try_from(stored_activation).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if stored_anchor != anchor_day_value
        || generation_one_profile_backfill_state(stored_activation, profile_backfill_completed)?
            == GenerationOneProfileBackfillState::Complete
    {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    prune_expired_usage_sync_rows(transaction, now)?;
    let first_day = anchor_day
        .checked_sub(Duration::days(29))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    let mut keys = BTreeSet::new();
    let mut updates = Vec::with_capacity(history.len());
    for daily in history {
        if daily.day < first_day
            || daily.day > anchor_day
            || !keys.insert((daily.provider, daily.day))
        {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        let Some(mut aggregate) = aggregate_from_total_with_day_policy(
            daily.provider,
            daily.day.to_string(),
            &daily.total,
            false,
        )?
        else {
            continue;
        };
        let correction_source_revision = unconsumed_parser_correction_revision(transaction, daily)?;
        aggregate.correction_reason = correction_source_revision
            .is_some()
            .then_some(CorrectionReason::ParserCorrection);
        validate_profile_backfill_aggregate(&aggregate, anchor_day, now)?;
        let update =
            queue_validated_daily_aggregate(transaction, active_mac_generation, aggregate)?;
        record_parser_correction_if_queued(
            transaction,
            active_mac_generation,
            daily,
            correction_source_revision,
            &update,
        )?;
        updates.push(update);
    }
    Ok(updates)
}

/// Queue higher-revision corrections for retained generation-one days.
///
/// A day that was missing from the atomic Profile backfill stays missing. A
/// disappearing local day also leaves the last accepted aggregate unchanged.
fn queue_retained_history_corrections(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    history: &[ProviderDailyUsage],
    now: OffsetDateTime,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Vec<QueueUpdate>, UsageSyncError> {
    if active_mac_generation != 1 {
        return Ok(Vec::new());
    }
    if history.len() > 120 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let initial_backfill_is_pending = generation_one_profile_backfill_is_pending(transaction)?;
    if initial_backfill_is_pending {
        return Ok(Vec::new());
    }
    prune_expired_usage_sync_rows(transaction, now)?;
    let today = now.to_offset(UtcOffset::UTC).date();
    let first_day = today
        .checked_sub(Duration::days(USAGE_HISTORY_RETENTION_DAYS - 1))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    let mut keys = BTreeSet::new();
    let mut updates = Vec::new();
    for daily in history {
        if !enabled_providers.contains(&daily.provider) || daily.day == today {
            continue;
        }
        if daily.day < first_day || daily.day > today || !keys.insert((daily.provider, daily.day)) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        let Some(existing) = load_aggregate(
            transaction,
            active_mac_generation,
            daily.provider,
            &daily.day.to_string(),
        )?
        else {
            continue;
        };
        let Some(mut aggregate) = aggregate_from_total_with_day_policy(
            daily.provider,
            daily.day.to_string(),
            &daily.total,
            false,
        )?
        else {
            continue;
        };
        let correction_source_revision = unconsumed_parser_correction_revision(transaction, daily)?;
        aggregate.correction_reason = correction_source_revision
            .is_some()
            .then_some(CorrectionReason::ParserCorrection);
        if pending_in_day_snapshot_precedes_late_history(
            transaction,
            active_mac_generation,
            daily,
            aggregate.observed_at,
        )? {
            continue;
        }
        preserve_retained_cost(&existing.aggregate, &mut aggregate)?;
        validate_retained_history_aggregate(&aggregate, now)?;
        let update =
            queue_validated_daily_aggregate(transaction, active_mac_generation, aggregate)?;
        record_parser_correction_if_queued(
            transaction,
            active_mac_generation,
            daily,
            correction_source_revision,
            &update,
        )?;
        updates.push(update);
    }
    Ok(updates)
}

fn pending_in_day_snapshot_precedes_late_history(
    connection: &Connection,
    active_mac_generation: u64,
    daily: &ProviderDailyUsage,
    candidate_observed_at: u64,
) -> Result<bool, UsageSyncError> {
    let Some(pending) = load_outbox_snapshot(
        connection,
        active_mac_generation,
        daily.provider,
        &daily.day.to_string(),
    )?
    else {
        return Ok(false);
    };
    let terminal_conflict = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM usage_sync_terminal_conflicts
             WHERE active_generation = ?1
               AND provider = ?2
               AND ranking_day = ?3
               AND revision = ?4
         )",
        params![
            to_database_integer(active_mac_generation)?,
            provider_database_value(daily.provider),
            daily.day.to_string(),
            to_database_integer(pending.revision)?
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if terminal_conflict {
        return Ok(false);
    }
    let day_start = daily
        .day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let next_day = daily
        .day
        .checked_add(Duration::days(1))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    let next_day_start = next_day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let earliest_millis = offset_date_time_millis(day_start)?;
    let latest_millis = offset_date_time_millis(next_day_start)?;
    Ok(
        (earliest_millis..latest_millis).contains(&pending.observed_at)
            && candidate_observed_at >= latest_millis,
    )
}

fn preserve_retained_cost(
    existing: &DailyUsageAggregate,
    candidate: &mut DailyUsageAggregate,
) -> Result<(), UsageSyncError> {
    if candidate.api_equivalent_cost.is_some() {
        return Ok(());
    }
    let Some(previous) = existing.api_equivalent_cost.as_ref() else {
        return Ok(());
    };
    let previous_quality = match previous.quality {
        SyncCostQuality::Reconciled => ApiEquivalentCostQuality::Reconciled,
        SyncCostQuality::Modeled => ApiEquivalentCostQuality::Modeled,
        SyncCostQuality::LocalOnly => ApiEquivalentCostQuality::LocalOnly,
    };
    let Some(projection) = project_retained_cost(
        previous.micros as f64,
        existing.observed_tokens,
        Some(previous_quality),
        previous.coverage_percent,
        candidate.observed_tokens,
    ) else {
        return Ok(());
    };
    let scaled = projection.amount.round();
    if !scaled.is_finite() || scaled < 0.0 || scaled > MAX_SAFE_INTEGER as f64 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let quality = match projection.quality {
        Some(ApiEquivalentCostQuality::Reconciled) => SyncCostQuality::Reconciled,
        Some(ApiEquivalentCostQuality::Modeled) => SyncCostQuality::Modeled,
        Some(ApiEquivalentCostQuality::LocalOnly) => SyncCostQuality::LocalOnly,
        None => return Err(UsageSyncError::INVALID_VALUE),
    };
    candidate.api_equivalent_cost = Some(SyncApiEquivalentCost {
        micros: scaled as u64,
        pricing_basis: previous.pricing_basis.clone(),
        quality,
        coverage_percent: projection.coverage_percent,
    });
    Ok(())
}

/// Store one validated aggregate and its latest cumulative outbox revision.
///
/// Both writes use the supplied transaction. A blocked generation stays
/// blocked when a newer local aggregate replaces its outbox row.
pub(crate) fn queue_daily_aggregate(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    aggregate: DailyUsageAggregate,
    now: OffsetDateTime,
) -> Result<QueueUpdate, UsageSyncError> {
    validate_current_day_aggregate(&aggregate, now)?;
    queue_validated_daily_aggregate(transaction, active_mac_generation, aggregate)
}

fn queue_validated_daily_aggregate(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    mut aggregate: DailyUsageAggregate,
) -> Result<QueueUpdate, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    aggregate.validate()?;
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
        if existing.aggregate.evidence_basis == SyncEvidenceBasis::ProviderReported
            && aggregate.evidence_basis == SyncEvidenceBasis::LocallyDerived
        {
            return Ok(QueueUpdate::Stale {
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

fn prune_expired_usage_sync_rows(
    transaction: &Transaction<'_>,
    now: OffsetDateTime,
) -> Result<usize, UsageSyncError> {
    let first_retained_day = now
        .to_offset(UtcOffset::UTC)
        .date()
        .checked_sub(Duration::days(USAGE_HISTORY_RETENTION_DAYS - 1))
        .ok_or(UsageSyncError::INVALID_VALUE)?
        .to_string();
    let aggregates = transaction.execute(
        "DELETE FROM usage_sync_daily_aggregates WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let transfer_day_carryovers = transaction.execute(
        "DELETE FROM usage_sync_transfer_day_carryovers WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let outbox = transaction.execute(
        "DELETE FROM usage_sync_latest_outbox WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let terminal_conflicts = transaction.execute(
        "DELETE FROM usage_sync_terminal_conflicts WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let baselines = transaction.execute(
        "DELETE FROM usage_sync_generation_baselines WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let correction_lineage = transaction.execute(
        "DELETE FROM usage_sync_correction_lineage WHERE ranking_day < ?1",
        [&first_retained_day],
    )?;
    let activations = transaction.execute(
        "DELETE FROM usage_sync_generation_activations
         WHERE ranking_day < ?1
           AND active_generation IN (
               SELECT active_generation
               FROM usage_sync_generations
               WHERE queue_state = 'abandoned'
           )",
        [&first_retained_day],
    )?;
    let provider_settings = transaction.execute(
        "DELETE FROM usage_sync_provider_settings_outbox
         WHERE delivery_state = 'abandoned'
           AND active_generation IN (
               SELECT generations.active_generation
               FROM usage_sync_generations AS generations
               WHERE generations.queue_state = 'abandoned'
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_daily_aggregates AS aggregates
                     WHERE aggregates.active_generation = generations.active_generation
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_latest_outbox AS outbox
                     WHERE outbox.active_generation = generations.active_generation
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_terminal_conflicts AS conflicts
                     WHERE conflicts.active_generation = generations.active_generation
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_generation_baselines AS baselines
                     WHERE baselines.active_generation = generations.active_generation
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_generation_activations AS activations
                     WHERE activations.active_generation = generations.active_generation
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM usage_sync_correction_lineage AS corrections
                     WHERE corrections.consumed_generation = generations.active_generation
                 )
           )",
        [],
    )?;
    let generations = transaction.execute(
        "DELETE FROM usage_sync_generations
         WHERE queue_state = 'abandoned'
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_daily_aggregates AS aggregates
               WHERE aggregates.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_latest_outbox AS outbox
               WHERE outbox.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_terminal_conflicts AS conflicts
               WHERE conflicts.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_generation_baselines AS baselines
               WHERE baselines.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_generation_activations AS activations
               WHERE activations.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_provider_settings_outbox AS settings
               WHERE settings.active_generation = usage_sync_generations.active_generation
           )
           AND NOT EXISTS (
               SELECT 1 FROM usage_sync_correction_lineage AS corrections
               WHERE corrections.consumed_generation = usage_sync_generations.active_generation
           )",
        [],
    )?;
    aggregates
        .checked_add(transfer_day_carryovers)
        .and_then(|rows| rows.checked_add(outbox))
        .and_then(|rows| rows.checked_add(terminal_conflicts))
        .and_then(|rows| rows.checked_add(baselines))
        .and_then(|rows| rows.checked_add(correction_lineage))
        .and_then(|rows| rows.checked_add(activations))
        .and_then(|rows| rows.checked_add(provider_settings))
        .and_then(|rows| rows.checked_add(generations))
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)
}

fn provider_settings_delivery_state(queue_state: QueueState) -> &'static str {
    match queue_state {
        QueueState::Pending => SETTINGS_PENDING,
        QueueState::Blocked => SETTINGS_BLOCKED,
        QueueState::Abandoned => SETTINGS_ABANDONED,
    }
}

/// Queue the latest complete provider setting for one Active Mac generation.
pub(crate) fn queue_provider_settings(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<bool, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let generation = to_database_integer(active_mac_generation)?;
    let codex_enabled = i64::from(enabled_providers.contains(&CodingProvider::Codex));
    let claude_enabled = i64::from(enabled_providers.contains(&CodingProvider::Claude));
    let queue_state = ensure_generation(transaction, active_mac_generation)?;
    let delivery_state = provider_settings_delivery_state(queue_state);
    let existing = transaction
        .query_row(
            "SELECT revision, codex_enabled, claude_enabled, delivery_state
             FROM usage_sync_provider_settings_outbox
             WHERE active_generation = ?1",
            [generation],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((revision, stored_codex, stored_claude, stored_state)) = existing {
        let revision = u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        validate_revision(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if !matches!(
            stored_state.as_str(),
            SETTINGS_PENDING | SETTINGS_SYNCED | SETTINGS_BLOCKED | SETTINGS_ABANDONED
        ) || !matches!(stored_codex, 0 | 1)
            || !matches!(stored_claude, 0 | 1)
        {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        if stored_codex == codex_enabled && stored_claude == claude_enabled {
            return Ok(stored_state == SETTINGS_PENDING);
        }
        let next_revision = revision
            .checked_add(1)
            .ok_or(UsageSyncError::INVALID_VALUE)?;
        validate_revision(next_revision)?;
        let updated = transaction.execute(
            "UPDATE usage_sync_provider_settings_outbox
             SET revision = ?1, codex_enabled = ?2, claude_enabled = ?3,
                 delivery_state = ?4
             WHERE active_generation = ?5 AND revision = ?6",
            params![
                to_database_integer(next_revision)?,
                codex_enabled,
                claude_enabled,
                delivery_state,
                generation,
                to_database_integer(revision)?
            ],
        )?;
        if updated != 1 {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        return Ok(delivery_state == SETTINGS_PENDING);
    }

    transaction.execute(
        "INSERT INTO usage_sync_provider_settings_outbox(
             active_generation, revision, codex_enabled, claude_enabled, delivery_state
         ) VALUES(?1, 1, ?2, ?3, ?4)",
        params![generation, codex_enabled, claude_enabled, delivery_state],
    )?;
    Ok(delivery_state == SETTINGS_PENDING)
}

fn load_pending_provider_settings(
    connection: &Connection,
    active_mac_generation: u64,
) -> Result<Option<ProviderSettingsSnapshot>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let row = connection
        .query_row(
            "SELECT revision, codex_enabled, claude_enabled
             FROM usage_sync_provider_settings_outbox
             WHERE active_generation = ?1 AND delivery_state = 'pending'",
            [to_database_integer(active_mac_generation)?],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((revision, codex_enabled, claude_enabled)) = row else {
        return Ok(None);
    };
    if !matches!(codex_enabled, 0 | 1) || !matches!(claude_enabled, 0 | 1) {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let revision = u64::try_from(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    validate_revision(revision).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    let mut enabled_providers = Vec::with_capacity(2);
    if codex_enabled == 1 {
        enabled_providers.push(CodingProvider::Codex);
    }
    if claude_enabled == 1 {
        enabled_providers.push(CodingProvider::Claude);
    }
    validate_enabled_providers(&enabled_providers)
        .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    Ok(Some(ProviderSettingsSnapshot {
        revision,
        enabled_providers,
    }))
}

/// Load no more than 62 latest pending revisions for one generation.
#[cfg(test)]
pub(crate) fn load_pending_usage_batch(
    connection: &Connection,
    active_mac_generation: u64,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    load_pending_usage_batch_for_day(
        connection,
        active_mac_generation,
        None,
        None,
        None,
        None,
        false,
    )
}

fn load_transfer_day_carryover_kind(
    connection: &Connection,
    active_mac_generation: u64,
    ranking_day: &str,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Option<TransferDayCarryoverKind>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    validate_ranking_day(ranking_day)?;
    let mut statement = connection.prepare(
        "SELECT carryover.carryover_kind
         FROM usage_sync_transfer_day_carryovers AS carryover
         JOIN usage_sync_latest_outbox AS outbox
           ON outbox.active_generation = carryover.active_generation
          AND outbox.provider = carryover.provider
          AND outbox.ranking_day = carryover.ranking_day
         WHERE carryover.active_generation = ?1
           AND carryover.ranking_day = ?2
           AND outbox.queue_state = 'active'
           AND (
               (outbox.provider = 'codex' AND ?3 = 1)
               OR (outbox.provider = 'claude' AND ?4 = 1)
           )
           AND NOT EXISTS (
               SELECT 1
               FROM usage_sync_terminal_conflicts AS terminal_conflict
               WHERE terminal_conflict.active_generation = outbox.active_generation
                 AND terminal_conflict.provider = outbox.provider
                 AND terminal_conflict.ranking_day = outbox.ranking_day
                 AND terminal_conflict.revision = outbox.revision
           )
         GROUP BY carryover.carryover_kind
         ORDER BY carryover.carryover_kind
         LIMIT 2",
    )?;
    let mut rows = statement.query(params![
        to_database_integer(active_mac_generation)?,
        ranking_day,
        i64::from(enabled_providers.contains(&CodingProvider::Codex)),
        i64::from(enabled_providers.contains(&CodingProvider::Claude))
    ])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let kind = TransferDayCarryoverKind::from_database_value(&row.get::<_, String>(0)?)?;
    if rows.next()?.is_some() {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(Some(kind))
}

/// Load the next bounded Profile backfill, historical retry, transfer
/// carryover, or current UTC day batch.
pub(crate) fn load_next_pending_usage_batch(
    connection: &Connection,
    active_mac_generation: u64,
    now: OffsetDateTime,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    if load_usage_sync_generation_state(connection, active_mac_generation)?
        != Some(QueueState::Pending)
    {
        return Ok(None);
    }
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    let activation = connection
        .query_row(
            "SELECT ranking_day, activated_at, profile_backfill_completed
             FROM usage_sync_generation_activations
             WHERE active_generation = ?1",
            [to_database_integer(active_mac_generation)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((activation_day, activated_at, profile_backfill_completed)) = activation {
        validate_ranking_day(&activation_day).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        let activated_at =
            u64::try_from(activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        validate_safe_integer(activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if active_mac_generation == 1 {
            if generation_one_profile_backfill_state(activated_at, profile_backfill_completed)?
                != GenerationOneProfileBackfillState::Complete
            {
                return load_pending_usage_batch_for_day(
                    connection,
                    active_mac_generation,
                    None,
                    None,
                    None,
                    Some(activation_day),
                    false,
                );
            }
            let retained_day = connection
                .query_row(
                    "SELECT ranking_day
                     FROM usage_sync_latest_outbox
                     WHERE active_generation = 1
                       AND queue_state = 'active'
                       AND ranking_day < ?1
                       AND (revision > 1 OR ranking_day > ?4)
                       AND NOT EXISTS (
                           SELECT 1
                           FROM usage_sync_terminal_conflicts AS terminal_conflict
                           WHERE terminal_conflict.active_generation =
                                     usage_sync_latest_outbox.active_generation
                             AND terminal_conflict.provider = usage_sync_latest_outbox.provider
                             AND terminal_conflict.ranking_day =
                                     usage_sync_latest_outbox.ranking_day
                             AND terminal_conflict.revision = usage_sync_latest_outbox.revision
                       )
                       AND (
                           (provider = 'codex' AND ?2 = 1)
                           OR (provider = 'claude' AND ?3 = 1)
                       )
                     ORDER BY ranking_day
                     LIMIT 1",
                    params![
                        ranking_day,
                        i64::from(enabled_providers.contains(&CodingProvider::Codex)),
                        i64::from(enabled_providers.contains(&CodingProvider::Claude)),
                        activation_day
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(retained_day) = retained_day {
                return load_pending_usage_batch_for_day(
                    connection,
                    active_mac_generation,
                    Some(&retained_day),
                    Some(enabled_providers),
                    None,
                    None,
                    true,
                );
            }
        }
        if activation_day < ranking_day
            && let Some(kind) = load_transfer_day_carryover_kind(
                connection,
                active_mac_generation,
                &activation_day,
                enabled_providers,
            )?
        {
            let carryover = TransferDayCarryover {
                ranking_day: activation_day.clone(),
                activated_at,
                kind,
            };
            let pending = load_pending_usage_batch_for_day(
                connection,
                active_mac_generation,
                Some(&activation_day),
                Some(enabled_providers),
                Some(carryover),
                None,
                false,
            )?;
            if let Some(batch) = pending.filter(PendingUsageBatch::has_usage_snapshots) {
                validate_transfer_day_carryover_batch(
                    &batch.snapshots,
                    batch
                        .transfer_day_carryover
                        .as_ref()
                        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)?,
                    now,
                )
                .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
                return Ok(Some(batch));
            }
        }
    }
    load_pending_usage_batch_for_day(
        connection,
        active_mac_generation,
        Some(&ranking_day),
        Some(enabled_providers),
        None,
        None,
        false,
    )
}

/// Report only whether an enabled provider has a terminal current-day conflict.
pub(crate) fn has_current_terminal_usage_conflict(
    connection: &Connection,
    active_mac_generation: u64,
    now: OffsetDateTime,
    enabled_providers: &BTreeSet<CodingProvider>,
) -> Result<bool, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let ranking_day = now.to_offset(UtcOffset::UTC).date().to_string();
    validate_ranking_day(&ranking_day)?;
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM usage_sync_terminal_conflicts AS terminal_conflict
                 JOIN usage_sync_latest_outbox AS outbox
                   ON outbox.active_generation = terminal_conflict.active_generation
                  AND outbox.provider = terminal_conflict.provider
                  AND outbox.ranking_day = terminal_conflict.ranking_day
                  AND outbox.revision = terminal_conflict.revision
                 WHERE terminal_conflict.active_generation = ?1
                   AND terminal_conflict.ranking_day = ?2
                   AND outbox.queue_state = 'active'
                   AND (
                       (terminal_conflict.provider = 'codex' AND ?3 = 1)
                       OR (terminal_conflict.provider = 'claude' AND ?4 = 1)
                   )
             )",
            params![
                to_database_integer(active_mac_generation)?,
                ranking_day,
                i64::from(enabled_providers.contains(&CodingProvider::Codex)),
                i64::from(enabled_providers.contains(&CodingProvider::Claude))
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(UsageSyncError::from)
}

fn load_pending_usage_batch_for_day(
    connection: &Connection,
    active_mac_generation: u64,
    ranking_day: Option<&str>,
    enabled_providers: Option<&BTreeSet<CodingProvider>>,
    transfer_day_carryover: Option<TransferDayCarryover>,
    profile_backfill_anchor: Option<String>,
    retained_history: bool,
) -> Result<Option<PendingUsageBatch>, UsageSyncError> {
    validate_generation(active_mac_generation)?;
    let provider_settings = load_pending_provider_settings(connection, active_mac_generation)?;
    let carryover_kind = transfer_day_carryover
        .as_ref()
        .map(|carryover| carryover.kind.as_database_value());
    let mut statement = connection.prepare(
        "SELECT provider, ranking_day, revision, snapshot_json,
                correction_reason, correction_revision
         FROM usage_sync_latest_outbox
         WHERE active_generation = ?1
           AND queue_state = 'active'
           AND (?2 IS NULL OR ranking_day = ?2)
           AND (
               ?5 IS NULL
               OR (
                   ranking_day >= date(?5, '-29 days')
                   AND ranking_day <= ?5
               )
           )
           AND (
               ?3 = 0
               OR EXISTS (
                   SELECT 1
                   FROM usage_sync_transfer_day_carryovers AS carryover
                   WHERE carryover.active_generation =
                             usage_sync_latest_outbox.active_generation
                     AND carryover.provider = usage_sync_latest_outbox.provider
                     AND carryover.ranking_day = usage_sync_latest_outbox.ranking_day
                     AND carryover.carryover_kind = ?4
               )
           )
           AND NOT EXISTS (
               SELECT 1
               FROM usage_sync_terminal_conflicts AS terminal_conflict
               WHERE terminal_conflict.active_generation =
                         usage_sync_latest_outbox.active_generation
                 AND terminal_conflict.provider = usage_sync_latest_outbox.provider
                 AND terminal_conflict.ranking_day = usage_sync_latest_outbox.ranking_day
                 AND terminal_conflict.revision = usage_sync_latest_outbox.revision
           )
         ORDER BY ranking_day, provider
         LIMIT 62",
    )?;
    let rows = statement.query_map(
        params![
            to_database_integer(active_mac_generation)?,
            ranking_day,
            i64::from(transfer_day_carryover.is_some()),
            carryover_kind,
            profile_backfill_anchor.as_deref()
        ],
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
        let expected_provider = provider_from_database_value(&provider)?;
        if enabled_providers
            .is_some_and(|enabled_providers| !enabled_providers.contains(&expected_provider))
        {
            continue;
        }
        if snapshot_json.len() > MAX_LOCAL_VALUE_BYTES {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
        let snapshot: UsageSyncSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        snapshot
            .validate()
            .map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
        if snapshot.provider != expected_provider
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
    if snapshots.is_empty() && provider_settings.is_none() && profile_backfill_anchor.is_none() {
        return Ok(None);
    }
    if !snapshots.is_empty() {
        validate_batch(&snapshots)?;
    }
    Ok(Some(PendingUsageBatch {
        active_mac_generation,
        provider_settings,
        snapshots,
        transfer_day_carryover,
        profile_backfill_anchor,
        retained_history,
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
    if acknowledgements.len() > MAX_USAGE_SYNC_BATCH {
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

pub(crate) fn parse_provider_settings_acknowledgement(
    bytes: &[u8],
) -> Result<ProviderSettingsAcknowledgement, UsageSyncError> {
    if bytes.is_empty() || bytes.len() > MAX_ACKNOWLEDGEMENT_BYTES {
        return Err(UsageSyncError::INVALID_RESPONSE);
    }
    let acknowledgement: ProviderSettingsAcknowledgement =
        serde_json::from_slice(bytes).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
    acknowledgement.validate()?;
    Ok(acknowledgement)
}

/// Apply one complete success value to the exact submitted batch.
///
/// A committed, conflict, or idempotent acknowledgement must name the
/// submitted revision. A conflict stores a terminal marker and keeps the
/// uncommitted payload. An equal-revision stale acknowledgement resolves the
/// submitted payload. A higher stale revision establishes a new local revision
/// floor. Each write uses the submitted revision. Therefore, a late response
/// cannot remove or stop a newer local revision.
pub(crate) fn apply_usage_acknowledgements(
    transaction: &Transaction<'_>,
    batch: &PendingUsageBatch,
    acknowledgements: &[UsageSyncAcknowledgement],
) -> Result<usize, UsageSyncError> {
    validate_generation(batch.active_mac_generation)?;
    if batch.snapshots.is_empty() {
        if !batch.is_empty_profile_backfill() || !acknowledgements.is_empty() {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
        mark_profile_backfill_complete(transaction, batch)?;
        return Ok(0);
    }
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
            AcknowledgementOutcome::Committed
            | AcknowledgementOutcome::Conflict
            | AcknowledgementOutcome::Idempotent => acknowledgement.revision == submitted_revision,
            AcknowledgementOutcome::Stale => acknowledgement.revision >= submitted_revision,
        };
        if !revision_is_valid {
            return Err(UsageSyncError::INVALID_RESPONSE);
        }
    }

    let generation = to_database_integer(batch.active_mac_generation)?;
    let mut applied = 0;
    for snapshot in &batch.snapshots {
        let acknowledgement = acknowledgements
            .iter()
            .find(|acknowledgement| {
                acknowledgement.provider == snapshot.provider
                    && acknowledgement.ranking_day == snapshot.ranking_day
            })
            .ok_or(UsageSyncError::INVALID_RESPONSE)?;
        let provider = provider_database_value(snapshot.provider);
        let revision = to_database_integer(snapshot.revision)?;
        match acknowledgement.outcome {
            AcknowledgementOutcome::Conflict => {
                applied += transaction.execute(
                    "INSERT INTO usage_sync_terminal_conflicts(
                         active_generation, provider, ranking_day, revision
                     )
                     SELECT ?1, ?2, ?3, ?4
                     WHERE EXISTS (
                         SELECT 1 FROM usage_sync_latest_outbox
                         WHERE active_generation = ?1
                           AND provider = ?2
                           AND ranking_day = ?3
                           AND revision = ?4
                           AND queue_state = 'active'
                     )
                     ON CONFLICT(active_generation, provider, ranking_day, revision)
                     DO NOTHING",
                    params![generation, provider, snapshot.ranking_day, revision],
                )?;
            }
            AcknowledgementOutcome::Committed
            | AcknowledgementOutcome::Idempotent
            | AcknowledgementOutcome::Stale => {
                let carryover_kind = if acknowledgement.outcome == AcknowledgementOutcome::Stale {
                    load_submitted_transfer_day_carryover_kind(
                        transaction,
                        batch.active_mac_generation,
                        snapshot,
                    )?
                } else {
                    None
                };
                let resolves_zero_carryover = match carryover_kind {
                    Some(TransferDayCarryoverKind::DelayedInstallationMarker) => {
                        if snapshot.observed_tokens != 0 {
                            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
                        }
                        true
                    }
                    Some(TransferDayCarryoverKind::PendingSegment) => snapshot.observed_tokens == 0,
                    None => false,
                };
                transaction.execute(
                    "DELETE FROM usage_sync_transfer_day_carryovers
                     WHERE active_generation = ?1
                       AND provider = ?2
                       AND ranking_day = ?3
                       AND EXISTS (
                           SELECT 1 FROM usage_sync_latest_outbox
                           WHERE active_generation = ?1
                             AND provider = ?2
                             AND ranking_day = ?3
                             AND revision = ?4
                             AND queue_state = 'active'
                       )",
                    params![generation, provider, snapshot.ranking_day, revision],
                )?;
                applied += transaction.execute(
                    "DELETE FROM usage_sync_latest_outbox
                     WHERE active_generation = ?1
                       AND provider = ?2
                       AND ranking_day = ?3
                       AND revision = ?4
                       AND queue_state = 'active'",
                    params![generation, provider, snapshot.ranking_day, revision],
                )?;
                if acknowledgement.outcome == AcknowledgementOutcome::Stale
                    && !resolves_zero_carryover
                    && acknowledgement.revision > snapshot.revision
                {
                    advance_local_revision_floor(
                        transaction,
                        batch.active_mac_generation,
                        snapshot,
                        acknowledgement.revision,
                        carryover_kind,
                    )?;
                }
            }
        }
    }
    if batch.profile_backfill_anchor.is_some() {
        mark_profile_backfill_complete(transaction, batch)?;
    }
    Ok(applied)
}

fn mark_profile_backfill_complete(
    transaction: &Transaction<'_>,
    batch: &PendingUsageBatch,
) -> Result<(), UsageSyncError> {
    let Some(anchor_day) = batch.profile_backfill_anchor.as_deref() else {
        return Err(UsageSyncError::INVALID_VALUE);
    };
    if batch.active_mac_generation != 1 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    validate_ranking_day(anchor_day)?;
    let activation = transaction
        .query_row(
            "SELECT ranking_day, activated_at, profile_backfill_completed
             FROM usage_sync_generation_activations
             WHERE active_generation = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((stored_anchor, stored_activated_at, profile_backfill_completed)) = activation else {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    };
    if stored_anchor != anchor_day {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    let stored_activated_at =
        u64::try_from(stored_activated_at).map_err(|_| UsageSyncError::STORAGE_UNAVAILABLE)?;
    if generation_one_profile_backfill_state(stored_activated_at, profile_backfill_completed)?
        == GenerationOneProfileBackfillState::Complete
    {
        return Ok(());
    }
    let updated = transaction.execute(
        "UPDATE usage_sync_generation_activations
         SET profile_backfill_completed = 1
         WHERE active_generation = 1
           AND ranking_day = ?1
           AND activated_at = ?2
           AND profile_backfill_completed = 0",
        params![anchor_day, to_database_integer(stored_activated_at)?],
    )?;
    if updated != 1 {
        return Err(UsageSyncError::STORAGE_UNAVAILABLE);
    }
    Ok(())
}

fn load_submitted_transfer_day_carryover_kind(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    snapshot: &UsageSyncSnapshot,
) -> Result<Option<TransferDayCarryoverKind>, UsageSyncError> {
    let carryover_kind = transaction
        .query_row(
            "SELECT carryover.carryover_kind
             FROM usage_sync_transfer_day_carryovers AS carryover
             JOIN usage_sync_latest_outbox AS outbox
               ON outbox.active_generation = carryover.active_generation
              AND outbox.provider = carryover.provider
              AND outbox.ranking_day = carryover.ranking_day
             WHERE carryover.active_generation = ?1
               AND carryover.provider = ?2
               AND carryover.ranking_day = ?3
               AND outbox.revision = ?4
               AND outbox.queue_state = 'active'",
            params![
                to_database_integer(active_mac_generation)?,
                provider_database_value(snapshot.provider),
                snapshot.ranking_day,
                to_database_integer(snapshot.revision)?
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    carryover_kind
        .as_deref()
        .map(TransferDayCarryoverKind::from_database_value)
        .transpose()
}

pub(crate) fn apply_provider_settings_acknowledgement(
    transaction: &Transaction<'_>,
    batch: &PendingUsageBatch,
    acknowledgement: Option<&ProviderSettingsAcknowledgement>,
) -> Result<bool, UsageSyncError> {
    let Some(submitted) = batch.provider_settings.as_ref() else {
        return acknowledgement
            .is_none()
            .then_some(false)
            .ok_or(UsageSyncError::INVALID_RESPONSE);
    };
    let Some(acknowledgement) = acknowledgement else {
        return Err(UsageSyncError::INVALID_RESPONSE);
    };
    acknowledgement.validate()?;
    let generation = to_database_integer(batch.active_mac_generation)?;
    let submitted_revision = submitted.revision;
    match acknowledgement.outcome {
        AcknowledgementOutcome::Committed | AcknowledgementOutcome::Idempotent => {
            if acknowledgement.revision != submitted_revision {
                return Err(UsageSyncError::INVALID_RESPONSE);
            }
            transaction.execute(
                "UPDATE usage_sync_provider_settings_outbox
                 SET delivery_state = 'synced'
                 WHERE active_generation = ?1 AND revision = ?2
                   AND delivery_state = 'pending'",
                params![generation, to_database_integer(submitted_revision)?],
            )?;
            Ok(false)
        }
        AcknowledgementOutcome::Conflict => Err(UsageSyncError::INVALID_RESPONSE),
        AcknowledgementOutcome::Stale => {
            if acknowledgement.revision < submitted_revision {
                return Err(UsageSyncError::INVALID_RESPONSE);
            }
            let rebased_revision = acknowledgement
                .revision
                .checked_add(1)
                .ok_or(UsageSyncError::INVALID_RESPONSE)?;
            validate_revision(rebased_revision).map_err(|_| UsageSyncError::INVALID_RESPONSE)?;
            let updated = transaction.execute(
                "UPDATE usage_sync_provider_settings_outbox
                 SET revision = ?1
                 WHERE active_generation = ?2 AND revision = ?3
                   AND delivery_state = 'pending'",
                params![
                    to_database_integer(rebased_revision)?,
                    generation,
                    to_database_integer(submitted_revision)?
                ],
            )?;
            Ok(updated == 1)
        }
    }
}

fn advance_local_revision_floor(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
    submitted_snapshot: &UsageSyncSnapshot,
    server_revision: u64,
    carryover_kind: Option<TransferDayCarryoverKind>,
) -> Result<(), UsageSyncError> {
    let provider = submitted_snapshot.provider;
    let ranking_day = &submitted_snapshot.ranking_day;
    let stored = load_aggregate(transaction, active_mac_generation, provider, ranking_day)?
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)?;
    if stored.revision > server_revision {
        if carryover_kind.is_some() {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
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
    if let Some(carryover_kind) = carryover_kind {
        let restored = transaction.execute(
            "INSERT INTO usage_sync_transfer_day_carryovers(
                 active_generation, provider, ranking_day, carryover_kind
             )
             SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS (
                 SELECT 1 FROM usage_sync_latest_outbox
                 WHERE active_generation = ?1
                   AND provider = ?2
                   AND ranking_day = ?3
                   AND revision = ?5
                   AND queue_state = 'active'
             )
             ON CONFLICT(active_generation, provider, ranking_day) DO UPDATE SET
                 carryover_kind=excluded.carryover_kind",
            params![
                generation,
                provider_value,
                ranking_day,
                carryover_kind.as_database_value(),
                revision_value
            ],
        )?;
        if restored != 1 {
            return Err(UsageSyncError::STORAGE_UNAVAILABLE);
        }
    }
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
    let usage_rows = transaction.execute(
        "UPDATE usage_sync_latest_outbox
         SET queue_state = 'blocked'
         WHERE active_generation = ?1 AND queue_state != 'abandoned'",
        [generation],
    )?;
    let settings_rows = transaction.execute(
        "UPDATE usage_sync_provider_settings_outbox
         SET delivery_state = 'blocked'
         WHERE active_generation = ?1 AND delivery_state != 'abandoned'",
        [generation],
    )?;
    usage_rows
        .checked_add(settings_rows)
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)
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
    let usage_rows = transaction.execute(
        "UPDATE usage_sync_latest_outbox
         SET queue_state = 'abandoned'
         WHERE active_generation < ?1 AND queue_state != 'abandoned'",
        [generation],
    )?;
    let settings_rows = transaction.execute(
        "UPDATE usage_sync_provider_settings_outbox
         SET delivery_state = 'abandoned'
         WHERE active_generation < ?1 AND delivery_state != 'abandoned'",
        [generation],
    )?;
    usage_rows
        .checked_add(settings_rows)
        .ok_or(UsageSyncError::STORAGE_UNAVAILABLE)
}

/// Start one Profile's synchronization ledger without retaining another Profile's rows.
pub(crate) fn replace_profile_generation(
    transaction: &Transaction<'_>,
    active_mac_generation: u64,
) -> Result<(), UsageSyncError> {
    validate_generation(active_mac_generation)?;
    transaction.execute("DELETE FROM usage_sync_transfer_day_carryovers", [])?;
    transaction.execute("DELETE FROM usage_sync_terminal_conflicts", [])?;
    transaction.execute("DELETE FROM usage_sync_latest_outbox", [])?;
    transaction.execute("DELETE FROM usage_sync_provider_settings_outbox", [])?;
    transaction.execute("DELETE FROM usage_sync_generation_baselines", [])?;
    transaction.execute("DELETE FROM usage_sync_generation_activations", [])?;
    transaction.execute("DELETE FROM usage_sync_daily_aggregates", [])?;
    transaction.execute("DELETE FROM usage_sync_generations", [])?;
    transaction.execute(
        "UPDATE usage_sync_correction_lineage SET consumed_generation = NULL",
        [],
    )?;
    activate_generation(transaction, active_mac_generation)?;
    Ok(())
}

fn aggregate_from_total(
    provider: CodingProvider,
    ranking_day: String,
    total: &UsageTotal,
) -> Result<Option<DailyUsageAggregate>, UsageSyncError> {
    aggregate_from_total_with_day_policy(provider, ranking_day, total, true)
}

fn aggregate_from_total_with_day_policy(
    provider: CodingProvider,
    ranking_day: String,
    total: &UsageTotal,
    observed_at_must_be_in_ranking_day: bool,
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
    if observed_at_must_be_in_ranking_day
        && observed_at.to_offset(UtcOffset::UTC).date().to_string() != ranking_day
    {
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

fn validate_enabled_providers(enabled_providers: &[CodingProvider]) -> Result<(), UsageSyncError> {
    if enabled_providers.len() > 2 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let mut providers = BTreeSet::new();
    for provider in enabled_providers {
        if !providers.insert(*provider) {
            return Err(UsageSyncError::INVALID_VALUE);
        }
    }
    Ok(())
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

fn validate_profile_backfill_batch(
    snapshots: &[UsageSyncSnapshot],
    anchor_day: &str,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    if snapshots.len() > 60 {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    if !snapshots.is_empty() {
        validate_batch(snapshots)?;
    }
    let anchor_day = parse_ranking_day_value(anchor_day)?;
    if anchor_day > now.to_offset(UtcOffset::UTC).date() {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    for snapshot in snapshots {
        validate_profile_backfill_aggregate(&snapshot.as_aggregate(), anchor_day, now)?;
    }
    Ok(())
}

fn validate_profile_backfill_aggregate(
    aggregate: &DailyUsageAggregate,
    anchor_day: time::Date,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    aggregate.validate()?;
    let ranking_day = parse_ranking_day_value(&aggregate.ranking_day)?;
    let first_day = anchor_day
        .checked_sub(Duration::days(29))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    if anchor_day > now.to_offset(UtcOffset::UTC).date()
        || ranking_day < first_day
        || ranking_day > anchor_day
    {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let day_start = ranking_day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let earliest_millis = offset_date_time_millis(day_start)?;
    let latest_millis = offset_date_time_millis(now)?
        .checked_add(FUTURE_OBSERVATION_TOLERANCE_MILLIS)
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    if aggregate.observed_at < earliest_millis || aggregate.observed_at > latest_millis {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_retained_history_aggregate(
    aggregate: &DailyUsageAggregate,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    aggregate.validate()?;
    let ranking_day = parse_ranking_day_value(&aggregate.ranking_day)?;
    let today = now.to_offset(UtcOffset::UTC).date();
    let first_day = today
        .checked_sub(Duration::days(USAGE_HISTORY_RETENTION_DAYS - 1))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    if ranking_day < first_day || ranking_day >= today {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    let day_start = ranking_day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let earliest_millis = offset_date_time_millis(day_start)?;
    let latest_millis = offset_date_time_millis(now)?
        .checked_add(FUTURE_OBSERVATION_TOLERANCE_MILLIS)
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    if aggregate.observed_at < earliest_millis || aggregate.observed_at > latest_millis {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_retained_history_batch(
    snapshots: &[UsageSyncSnapshot],
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    validate_batch(snapshots)?;
    for snapshot in snapshots {
        validate_retained_history_aggregate(&snapshot.as_aggregate(), now)?;
        if snapshot.revision == 1 {
            validate_delayed_current_day_retry(snapshot)?;
        }
    }
    Ok(())
}

fn validate_delayed_current_day_retry(snapshot: &UsageSyncSnapshot) -> Result<(), UsageSyncError> {
    let ranking_day = parse_ranking_day_value(&snapshot.ranking_day)?;
    let day_start = ranking_day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let next_day = ranking_day
        .checked_add(Duration::days(1))
        .ok_or(UsageSyncError::INVALID_VALUE)?;
    let next_day_start = next_day
        .with_hms(0, 0, 0)
        .map_err(|_| UsageSyncError::INVALID_VALUE)?
        .assume_utc();
    let earliest_millis = offset_date_time_millis(day_start)?;
    let latest_millis = offset_date_time_millis(next_day_start)?;
    if snapshot.observed_at < earliest_millis || snapshot.observed_at >= latest_millis {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_transfer_day_carryover_batch(
    snapshots: &[UsageSyncSnapshot],
    carryover: &TransferDayCarryover,
    now: OffsetDateTime,
) -> Result<(), UsageSyncError> {
    validate_batch(snapshots)?;
    if snapshots.len() > MAX_TRANSFER_DAY_CARRYOVER_MARKERS {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    validate_ranking_day(&carryover.ranking_day)?;
    validate_safe_integer(carryover.activated_at)?;
    let current_day = now.to_offset(UtcOffset::UTC).date().to_string();
    if carryover.ranking_day >= current_day {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    for snapshot in snapshots {
        match carryover.kind {
            TransferDayCarryoverKind::DelayedInstallationMarker => {
                validate_delayed_installation_marker_snapshot(
                    snapshot,
                    &carryover.ranking_day,
                    carryover.activated_at,
                )?;
            }
            TransferDayCarryoverKind::PendingSegment => {
                validate_transfer_day_pending_segment(
                    snapshot,
                    &carryover.ranking_day,
                    carryover.activated_at,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_transfer_day_pending_segment(
    snapshot: &UsageSyncSnapshot,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    snapshot.validate()?;
    if snapshot.observed_tokens == 0 {
        validate_transfer_day_aggregate(&snapshot.as_aggregate(), ranking_day, activated_at)?;
        if snapshot.revision != 1
            || snapshot.api_equivalent_cost.is_some()
            || snapshot.correction_reason.is_some()
            || snapshot.correction_revision.is_some()
        {
            return Err(UsageSyncError::INVALID_VALUE);
        }
        return Ok(());
    }
    validate_transfer_day_aggregate(&snapshot.as_aggregate(), ranking_day, activated_at)
}

fn validate_delayed_installation_marker_snapshot(
    snapshot: &UsageSyncSnapshot,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    snapshot.validate()?;
    validate_transfer_day_carryover_marker(&snapshot.as_aggregate(), ranking_day, activated_at)?;
    if snapshot.revision != 1 || snapshot.correction_revision.is_some() {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_transfer_day_aggregate(
    aggregate: &DailyUsageAggregate,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    aggregate.validate()?;
    validate_ranking_day(ranking_day)?;
    validate_safe_integer(activated_at)?;
    let activated_at_time =
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(activated_at) * 1_000_000)
            .map_err(|_| UsageSyncError::INVALID_VALUE)?;
    let observed_at_time =
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(aggregate.observed_at) * 1_000_000)
            .map_err(|_| UsageSyncError::INVALID_VALUE)?;
    if activated_at_time
        .to_offset(UtcOffset::UTC)
        .date()
        .to_string()
        != ranking_day
        || observed_at_time
            .to_offset(UtcOffset::UTC)
            .date()
            .to_string()
            != ranking_day
        || aggregate.ranking_day != ranking_day
        || aggregate.coverage != SyncCoverage::Partial
        || aggregate.observed_at < activated_at
    {
        return Err(UsageSyncError::INVALID_VALUE);
    }
    Ok(())
}

fn validate_transfer_day_carryover_marker(
    aggregate: &DailyUsageAggregate,
    ranking_day: &str,
    activated_at: u64,
) -> Result<(), UsageSyncError> {
    validate_transfer_day_aggregate(aggregate, ranking_day, activated_at)?;
    if aggregate.observed_at != activated_at
        || aggregate.observed_tokens != 0
        || aggregate.api_equivalent_cost.is_some()
        || aggregate.correction_reason.is_some()
    {
        return Err(UsageSyncError::INVALID_VALUE);
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

fn parse_ranking_day_value(value: &str) -> Result<time::Date, UsageSyncError> {
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
    time::Date::from_calendar_date(year, month, day).map_err(|_| UsageSyncError::INVALID_VALUE)
}

fn validate_ranking_day(value: &str) -> Result<(), UsageSyncError> {
    parse_ranking_day_value(value).map(|_| ())
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
        // Keep the bounded prior Codex catalog while a retained 60-day row
        // can still prove the exact effective-dated cost basis.
        (CodingProvider::Codex, "openai-standard-2026-08-06-v1")
            | (CodingProvider::Codex, "openai-api-2026-08-09-v3")
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
    use std::{
        env, fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;
    use serde_json::{Value, json};
    use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::providers::ProviderDailyUsage;
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

    fn enabled_providers() -> BTreeSet<CodingProvider> {
        [CodingProvider::Codex, CodingProvider::Claude]
            .into_iter()
            .collect()
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

    struct PersistentTestDatabase(PathBuf);

    impl PersistentTestDatabase {
        fn new(label: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(env::temp_dir().join(format!(
                "touchgrassbar-usage-sync-{label}-{}-{timestamp}.sqlite3",
                process::id()
            )))
        }

        fn connect(&self) -> Connection {
            let connection = Connection::open(&self.0).unwrap();
            connection
                .execute_batch("PRAGMA foreign_keys = ON;")
                .unwrap();
            install_usage_sync_schema(&connection).unwrap();
            connection
        }
    }

    impl Drop for PersistentTestDatabase {
        fn drop(&mut self) {
            for path in [
                self.0.clone(),
                self.0.with_extension("sqlite3-journal"),
                self.0.with_extension("sqlite3-shm"),
                self.0.with_extension("sqlite3-wal"),
            ] {
                let _ = fs::remove_file(path);
            }
        }
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
                    "openai-api-2026-08-09-v3",
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
                "profileBackfillAnchor": null,
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
    fn first_profile_backfill_queues_at_most_thirty_sparse_utc_days_for_both_providers() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let anchor = now().date();
        let history = (0..30)
            .flat_map(|offset| {
                [CodingProvider::Codex, CodingProvider::Claude].map(|provider| ProviderDailyUsage {
                    provider,
                    day: anchor - Duration::days(offset),
                    total: total(
                        match provider {
                            CodingProvider::Codex => UsageEvidenceBasis::ProviderReported,
                            CodingProvider::Claude => UsageEvidenceBasis::LocallyDerived,
                        },
                        100 + u64::try_from(offset).unwrap(),
                        NOW,
                        None,
                    ),
                    correction: None,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(history.len(), 60);
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        let updates = queue_profile_backfill(&transaction, 1, &history, anchor, now()).unwrap();
        assert_eq!(updates.len(), 60);
        transaction.commit().unwrap();

        let batch = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        assert_eq!(batch.snapshots().len(), 60);
        assert_eq!(batch.snapshots()[0].ranking_day, "2026-07-10");
        assert_eq!(batch.snapshots()[0].provider, CodingProvider::Claude);
        assert_eq!(batch.snapshots()[1].provider, CodingProvider::Codex);
        assert!(batch.mutation_args(INSTALLATION_CREDENTIAL, now()).is_ok());
        let retry = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        assert_eq!(retry, batch);
        let encoded =
            serde_json::to_string(&retry.mutation_args(INSTALLATION_CREDENTIAL, now()).unwrap())
                .unwrap();
        for forbidden in [
            "/Users/private",
            "PRIVATE-CONTENT",
            "PRIVATE-SESSION",
            "private-model-name",
            "provider-private-id",
        ] {
            assert!(!encoded.contains(forbidden));
        }

        let too_old = ProviderDailyUsage {
            provider: CodingProvider::Codex,
            day: anchor - Duration::days(30),
            total: total(UsageEvidenceBasis::ProviderReported, 1, NOW, None),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            queue_profile_backfill(&transaction, 1, &[too_old], anchor, now(),),
            Err(UsageSyncError::INVALID_VALUE)
        );
    }

    #[test]
    fn profile_backfill_is_all_provider_bounded_and_durable_across_utc_rollover() {
        let database = PersistentTestDatabase::new("profile-backfill-completion");
        let mut connection = database.connect();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let anchor = now().date();
        let history = vec![
            ProviderDailyUsage {
                provider: CodingProvider::Codex,
                day: anchor - Duration::days(29),
                total: total(UsageEvidenceBasis::ProviderReported, 10, NOW, None),
                correction: None,
            },
            ProviderDailyUsage {
                provider: CodingProvider::Claude,
                day: anchor,
                total: total(UsageEvidenceBasis::LocallyDerived, 20, NOW, None),
                correction: None,
            },
        ];
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &history, anchor, now()).unwrap();
        // This active row is outside the fixed Profile window and must not
        // leak into that first atomic mutation.
        let mut outside = aggregate(CodingProvider::Codex, 99, 1_000);
        outside.ranking_day = (anchor - Duration::days(30)).to_string();
        outside.observed_at = offset_date_time_millis(now() - Duration::days(30)).unwrap();
        queue_validated_daily_aggregate(&transaction, 1, outside).unwrap();
        transaction.commit().unwrap();

        let next_day = now() + Duration::days(1);
        let batch = load_next_pending_usage_batch(
            &connection,
            1,
            next_day,
            &BTreeSet::from([CodingProvider::Codex]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(batch.snapshots().len(), 2);
        assert!(
            batch
                .snapshots()
                .iter()
                .any(|snapshot| snapshot.provider == CodingProvider::Claude)
        );
        assert!(
            batch
                .snapshots()
                .iter()
                .all(|snapshot| snapshot.ranking_day.as_str() >= "2026-07-10")
        );
        let value = serde_json::to_value(
            batch
                .mutation_args(INSTALLATION_CREDENTIAL, next_day)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["profileBackfillAnchor"], "2026-08-08");

        let acknowledgements = batch
            .snapshots()
            .iter()
            .map(|snapshot| {
                acknowledgement(
                    snapshot,
                    AcknowledgementOutcome::Committed,
                    snapshot.revision,
                )
            })
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &batch, &acknowledgements).unwrap();
        transaction.commit().unwrap();
        let (stored_activated_at, profile_backfill_completed) = connection
            .query_row(
                "SELECT activated_at, profile_backfill_completed
                 FROM usage_sync_generation_activations
                 WHERE active_generation = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(
            u64::try_from(stored_activated_at).unwrap(),
            offset_date_time_millis(now()).unwrap()
        );
        assert_eq!(profile_backfill_completed, 1);
        drop(connection);

        let mut connection = database.connect();
        assert!(!generation_one_profile_backfill_is_pending(&connection).unwrap());

        let transaction = connection.transaction().unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), next_day).unwrap();
        transaction.commit().unwrap();
        assert!(
            load_next_pending_usage_batch(
                &connection,
                1,
                next_day,
                &BTreeSet::from([CodingProvider::Codex]),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn completed_profile_backfill_releases_a_later_current_day_retry() {
        let mut connection = connection();
        let unavailable = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let anchor = now().date();
        let history = [ProviderDailyUsage {
            provider: CodingProvider::Codex,
            day: anchor - Duration::days(1),
            total: total(UsageEvidenceBasis::ProviderReported, 10, NOW, None),
            correction: None,
        }];
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &unavailable, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &history, anchor, now()).unwrap();
        transaction.commit().unwrap();

        let next_day = now() + Duration::days(1);
        let next_state = state_with_totals(
            total(
                UsageEvidenceBasis::ProviderReported,
                20,
                "2026-08-09T12:34:56Z",
                None,
            ),
            UsageTotal::Unavailable,
        );
        let transaction = connection.transaction().unwrap();
        let updates = queue_usage_for_commit(
            &transaction,
            1,
            &next_state,
            next_day,
            &BTreeSet::from([CodingProvider::Codex]),
            UsageQueueRequest::Refresh(&UsageSyncCorrections::default()),
        )
        .unwrap();
        assert!(matches!(
            updates.as_slice(),
            [QueueUpdate::Stored { revision: 1, .. }]
        ));
        transaction.commit().unwrap();

        let retry_day = next_day + Duration::days(1);
        let profile_batch = load_next_pending_usage_batch(
            &connection,
            1,
            retry_day,
            &BTreeSet::from([CodingProvider::Codex]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(profile_batch.snapshots().len(), 1);
        assert_eq!(profile_batch.snapshots()[0].ranking_day, "2026-08-07");
        let acknowledgements = profile_batch
            .snapshots()
            .iter()
            .map(|snapshot| {
                acknowledgement(
                    snapshot,
                    AcknowledgementOutcome::Committed,
                    snapshot.revision,
                )
            })
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &profile_batch, &acknowledgements).unwrap();
        transaction.commit().unwrap();

        let late_revision = ProviderDailyUsage {
            provider: CodingProvider::Codex,
            day: next_day.date(),
            total: total(
                UsageEvidenceBasis::ProviderReported,
                25,
                "2026-08-10T12:34:56Z",
                None,
            ),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        assert!(
            queue_retained_history_corrections(
                &transaction,
                1,
                std::slice::from_ref(&late_revision),
                retry_day,
                &BTreeSet::from([CodingProvider::Codex]),
            )
            .unwrap()
            .is_empty()
        );
        transaction.commit().unwrap();

        let delayed = load_next_pending_usage_batch(
            &connection,
            1,
            retry_day,
            &BTreeSet::from([CodingProvider::Codex]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(delayed.snapshots().len(), 1);
        assert_eq!(delayed.snapshots()[0].ranking_day, "2026-08-09");
        assert_eq!(delayed.snapshots()[0].revision, 1);
        assert!(
            delayed
                .mutation_args(INSTALLATION_CREDENTIAL, retry_day)
                .is_ok()
        );
        let acknowledgement =
            acknowledgement(&delayed.snapshots()[0], AcknowledgementOutcome::Conflict, 1);
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &delayed, &[acknowledgement]).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        assert!(matches!(
            queue_retained_history_corrections(
                &transaction,
                1,
                &[late_revision],
                retry_day,
                &BTreeSet::from([CodingProvider::Codex]),
            )
            .unwrap()
            .as_slice(),
            [QueueUpdate::Stored { revision: 2, .. }]
        ));
        transaction.commit().unwrap();
    }

    #[test]
    fn empty_profile_backfill_retries_until_the_usage_mutation_is_acknowledged() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &[], now().date(), now()).unwrap();
        transaction.commit().unwrap();

        let batch = load_next_pending_usage_batch(&connection, 1, now(), &BTreeSet::new())
            .unwrap()
            .unwrap();
        assert!(batch.is_empty_profile_backfill());
        assert!(!batch.has_usage_snapshots());
        assert!(batch.requires_usage_mutation());
        let value =
            serde_json::to_value(batch.mutation_args(INSTALLATION_CREDENTIAL, now()).unwrap())
                .unwrap();
        assert_eq!(value["profileBackfillAnchor"], "2026-08-08");
        assert_eq!(value["snapshots"], json!([]));

        // A settings-only response did not run dailyUsage. The caller must
        // keep the marker and must not call the usage acknowledgement path.
        assert!(
            load_next_pending_usage_batch(&connection, 1, now(), &BTreeSet::new())
                .unwrap()
                .unwrap()
                .is_empty_profile_backfill()
        );
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &batch, &[]).unwrap(),
            0
        );
        transaction.commit().unwrap();
        assert!(
            load_next_pending_usage_batch(&connection, 1, now(), &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn empty_profile_backfill_rejects_a_future_anchor() {
        let batch = PendingUsageBatch {
            active_mac_generation: 1,
            provider_settings: None,
            snapshots: Vec::new(),
            transfer_day_carryover: None,
            profile_backfill_anchor: Some("2026-08-09".to_owned()),
            retained_history: false,
        };

        assert!(matches!(
            batch.mutation_args(INSTALLATION_CREDENTIAL, now()),
            Err(UsageSyncError::INVALID_VALUE)
        ));
    }

    #[test]
    fn historical_corrections_keep_one_latest_revision_per_provider_day() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let day = now().date() - Duration::days(1);
        let initial = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(UsageEvidenceBasis::LocallyDerived, 100, NOW, None),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &[initial], now().date(), now()).unwrap();
        transaction.commit().unwrap();
        let first = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        let acknowledgement =
            acknowledgement(&first.snapshots()[0], AcknowledgementOutcome::Committed, 1);
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &first, &[acknowledgement]).unwrap();
        transaction.commit().unwrap();

        let corrected = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(
                UsageEvidenceBasis::LocallyDerived,
                80,
                "2026-08-08T12:34:57Z",
                None,
            ),
            correction: Some(ProviderCorrection::ParserCorrection { source_revision: 7 }),
        };
        let transaction = connection.transaction().unwrap();
        let updates = queue_retained_history_corrections(
            &transaction,
            1,
            &[corrected],
            now(),
            &enabled_providers(),
        )
        .unwrap();
        assert!(matches!(
            updates.as_slice(),
            [QueueUpdate::Stored { revision: 2, .. }]
        ));
        transaction.commit().unwrap();

        let batch = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        assert_eq!(batch.snapshots().len(), 1);
        assert_eq!(batch.snapshots()[0].revision, 2);
        assert_eq!(
            batch.snapshots()[0].correction_reason,
            Some(CorrectionReason::ParserCorrection)
        );
        assert!(batch.mutation_args(INSTALLATION_CREDENTIAL, now()).is_ok());

        let unproved = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(
                UsageEvidenceBasis::LocallyDerived,
                60,
                "2026-08-08T12:34:58Z",
                None,
            ),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        assert!(matches!(
            queue_retained_history_corrections(
                &transaction,
                1,
                &[unproved],
                now(),
                &enabled_providers(),
            )
            .unwrap()
            .as_slice(),
            [QueueUpdate::Stale { revision: 2, .. }]
        ));
    }

    #[test]
    fn same_claude_correction_source_does_not_create_a_second_audit() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let day = now().date() - Duration::days(1);
        let initial = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(UsageEvidenceBasis::LocallyDerived, 100, NOW, None),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &[initial], now().date(), now()).unwrap();
        transaction.commit().unwrap();
        let initial_batch =
            load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
                .unwrap()
                .unwrap();
        let initial_ack = acknowledgement(
            &initial_batch.snapshots()[0],
            AcknowledgementOutcome::Committed,
            1,
        );
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &initial_batch, &[initial_ack]).unwrap();
        transaction.commit().unwrap();

        let corrected = |tokens, observed_at| ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(
                UsageEvidenceBasis::LocallyDerived,
                tokens,
                observed_at,
                None,
            ),
            correction: Some(ProviderCorrection::ParserCorrection { source_revision: 7 }),
        };
        let transaction = connection.transaction().unwrap();
        queue_retained_history_corrections(
            &transaction,
            1,
            &[corrected(80, "2026-08-08T12:34:57Z")],
            now(),
            &enabled_providers(),
        )
        .unwrap();
        transaction.commit().unwrap();
        let correction_batch =
            load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
                .unwrap()
                .unwrap();
        assert_eq!(correction_batch.snapshots()[0].correction_revision, Some(2));
        let correction_ack = acknowledgement(
            &correction_batch.snapshots()[0],
            AcknowledgementOutcome::Committed,
            2,
        );
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &correction_batch, &[correction_ack]).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        queue_retained_history_corrections(
            &transaction,
            1,
            &[corrected(90, "2026-08-08T12:34:58Z")],
            now(),
            &enabled_providers(),
        )
        .unwrap();
        transaction.commit().unwrap();
        let continuation =
            load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
                .unwrap()
                .unwrap();
        assert_eq!(continuation.snapshots()[0].revision, 3);
        assert_eq!(continuation.snapshots()[0].correction_reason, None);
        assert_eq!(continuation.snapshots()[0].correction_revision, None);
    }

    #[test]
    fn terminal_profile_conflict_completes_and_a_new_local_revision_recovers() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let day = now().date() - Duration::days(1);
        let initial = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(UsageEvidenceBasis::LocallyDerived, 100, NOW, None),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &[initial], now().date(), now()).unwrap();
        transaction.commit().unwrap();
        let batch = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        let conflict = acknowledgement(&batch.snapshots()[0], AcknowledgementOutcome::Conflict, 1);
        let transaction = connection.transaction().unwrap();
        apply_usage_acknowledgements(&transaction, &batch, &[conflict]).unwrap();
        transaction.commit().unwrap();
        assert!(
            load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
                .unwrap()
                .is_none()
        );

        let correction = ProviderDailyUsage {
            provider: CodingProvider::Claude,
            day,
            total: total(
                UsageEvidenceBasis::LocallyDerived,
                80,
                "2026-08-08T12:34:57Z",
                None,
            ),
            correction: Some(ProviderCorrection::ParserCorrection { source_revision: 8 }),
        };
        let transaction = connection.transaction().unwrap();
        queue_retained_history_corrections(
            &transaction,
            1,
            &[correction],
            now(),
            &enabled_providers(),
        )
        .unwrap();
        transaction.commit().unwrap();
        let recovered = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        assert_eq!(recovered.snapshots()[0].revision, 2);
        assert_eq!(recovered.snapshots()[0].observed_tokens, 80);
    }

    #[test]
    fn retained_correction_accepts_the_bounded_prior_codex_pricing_basis() {
        let mut connection = connection();
        let state = state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable);
        let day = now().date() - Duration::days(1);
        let initial = ProviderDailyUsage {
            provider: CodingProvider::Codex,
            day,
            total: total(
                UsageEvidenceBasis::ProviderReported,
                100,
                NOW,
                Some((
                    1.0,
                    "openai-standard-2026-08-06-v1",
                    ApiEquivalentCostQuality::Reconciled,
                    None,
                )),
            ),
            correction: None,
        };
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 1).unwrap();
        capture_generation_baselines(&transaction, 1, &state, now(), now()).unwrap();
        queue_profile_backfill(&transaction, 1, &[initial], now().date(), now()).unwrap();
        transaction.commit().unwrap();
        let batch = load_next_pending_usage_batch(&connection, 1, now(), &enabled_providers())
            .unwrap()
            .unwrap();
        assert_eq!(
            batch.snapshots()[0]
                .api_equivalent_cost
                .as_ref()
                .unwrap()
                .pricing_basis,
            "openai-standard-2026-08-06-v1"
        );
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
    fn final_outbox_schema_rejects_incomplete_correction_provenance() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Claude, 80, 2_000),
        )
        .unwrap();
        transaction.commit().unwrap();

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
    fn provider_owned_evidence_rejects_higher_and_lower_local_replacements() {
        for local_tokens in [80, 120] {
            let mut connection = connection();
            let transaction = connection.transaction().unwrap();
            queue_daily_aggregate(
                &transaction,
                1,
                aggregate(CodingProvider::Codex, 100, 1_000),
            )
            .unwrap();
            transaction.commit().unwrap();

            let transaction = connection.transaction().unwrap();
            let mut local = aggregate(CodingProvider::Codex, local_tokens, 2_000);
            local.evidence_basis = SyncEvidenceBasis::LocallyDerived;
            assert_eq!(
                queue_daily_aggregate(&transaction, 1, local).unwrap(),
                QueueUpdate::Stale {
                    provider: CodingProvider::Codex,
                    revision: 1,
                }
            );
            transaction.commit().unwrap();

            let pending = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
            assert_eq!(pending.snapshots()[0].revision, 1);
            assert_eq!(pending.snapshots()[0].observed_tokens, 100);
            assert_eq!(
                pending.snapshots()[0].evidence_basis,
                SyncEvidenceBasis::ProviderReported
            );
        }
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
        queue_current_utc_day_with_corrections(
            &transaction,
            1,
            &state,
            now(),
            &corrections,
            &enabled_providers(),
        )
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
        queue_current_utc_day(
            &transaction,
            1,
            &corrected_then_increased,
            now(),
            &enabled_providers(),
        )
        .unwrap();
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
        queue_current_utc_day_with_corrections(
            &transaction,
            1,
            &later_usage,
            now(),
            &corrections,
            &enabled_providers(),
        )
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
            queue_current_utc_day(
                &transaction,
                1,
                &unsupported_decrease,
                now(),
                &enabled_providers(),
            )
            .unwrap()[0],
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
    fn equal_revision_stale_resolves_both_submitted_provider_days() {
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
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Stale, 1))
            .collect::<Vec<_>>();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &acknowledgements).unwrap(),
            2
        );
        transaction.commit().unwrap();

        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());
    }

    #[test]
    fn equal_revision_stale_resolves_a_rejected_higher_revision_local_row() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let mut first = aggregate(CodingProvider::Codex, 10, 1_000);
        first.evidence_basis = SyncEvidenceBasis::LocallyDerived;
        queue_daily_aggregate(&transaction, 1, first).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        let mut replacement = aggregate(CodingProvider::Codex, 20, 2_000);
        replacement.evidence_basis = SyncEvidenceBasis::LocallyDerived;
        queue_daily_aggregate(&transaction, 1, replacement).unwrap();
        transaction.commit().unwrap();
        let sent = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(sent.snapshots()[0].revision, 2);
        let stale = acknowledgement(&sent.snapshots()[0], AcknowledgementOutcome::Stale, 2);

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &[stale]).unwrap(),
            1
        );
        transaction.commit().unwrap();

        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());
    }

    #[test]
    fn conflict_keeps_both_payloads_terminal_until_a_new_revision_exists() {
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
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Conflict, 1))
            .collect::<Vec<_>>();

        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &acknowledgements).unwrap(),
            2
        );
        transaction.commit().unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_terminal_conflicts",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );

        install_usage_sync_schema(&connection).unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());

        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            assert_eq!(
                queue_daily_aggregate(&transaction, 1, aggregate(provider, 10, 1000)).unwrap(),
                QueueUpdate::Unchanged {
                    provider,
                    revision: 1,
                }
            );
        }
        transaction.commit().unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());

        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            assert_eq!(
                queue_daily_aggregate(&transaction, 1, aggregate(provider, 20, 2000)).unwrap(),
                QueueUpdate::Stored {
                    provider,
                    revision: 2,
                    state: QueueState::Pending,
                }
            );
        }
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
    fn provider_settings_outbox_is_latest_only_and_rebases_a_stale_acknowledgement() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        assert!(queue_provider_settings(&transaction, 1, &enabled_providers()).unwrap());
        transaction.commit().unwrap();
        let first = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let settings = first.provider_settings().unwrap();
        assert_eq!(settings.revision(), 1);
        assert_eq!(
            settings.enabled_providers(),
            &[CodingProvider::Codex, CodingProvider::Claude]
        );

        let transaction = connection.transaction().unwrap();
        assert!(
            !apply_provider_settings_acknowledgement(
                &transaction,
                &first,
                Some(&ProviderSettingsAcknowledgement {
                    revision: 1,
                    outcome: AcknowledgementOutcome::Committed,
                }),
            )
            .unwrap()
        );
        transaction.commit().unwrap();
        assert!(load_pending_usage_batch(&connection, 1).unwrap().is_none());

        let transaction = connection.transaction().unwrap();
        assert!(
            queue_provider_settings(&transaction, 1, &BTreeSet::from([CodingProvider::Codex]),)
                .unwrap()
        );
        transaction.commit().unwrap();
        let second = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(second.provider_settings().unwrap().revision(), 2);

        let transaction = connection.transaction().unwrap();
        assert!(
            apply_provider_settings_acknowledgement(
                &transaction,
                &second,
                Some(&ProviderSettingsAcknowledgement {
                    revision: 2,
                    outcome: AcknowledgementOutcome::Stale,
                }),
            )
            .unwrap()
        );
        transaction.commit().unwrap();
        let rebased = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        let settings = rebased.provider_settings().unwrap();
        assert_eq!(settings.revision(), 3);
        assert_eq!(settings.enabled_providers(), &[CodingProvider::Codex]);

        let transaction = connection.transaction().unwrap();
        assert!(
            !apply_provider_settings_acknowledgement(
                &transaction,
                &second,
                Some(&ProviderSettingsAcknowledgement {
                    revision: 2,
                    outcome: AcknowledgementOutcome::Committed,
                }),
            )
            .unwrap()
        );
        transaction.commit().unwrap();
        assert_eq!(
            load_pending_usage_batch(&connection, 1)
                .unwrap()
                .unwrap()
                .provider_settings()
                .unwrap()
                .revision(),
            3
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
    fn profile_replacement_discards_the_prior_profiles_sync_ledger() {
        let mut connection = connection();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(&transaction, 7, aggregate(CodingProvider::Codex, 10, 1_000))
            .unwrap();
        queue_provider_settings(&transaction, 7, &BTreeSet::from([CodingProvider::Codex])).unwrap();
        transaction
            .execute(
                "INSERT INTO usage_sync_correction_lineage(
                     provider, ranking_day, source_revision, reason, consumed_generation
                 ) VALUES('codex', '2026-08-08', 1, 'parser-correction', 7)",
                [],
            )
            .unwrap();

        replace_profile_generation(&transaction, 2).unwrap();
        transaction.commit().unwrap();

        let generations = connection
            .prepare("SELECT active_generation, queue_state FROM usage_sync_generations")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(generations, vec![(2, "active".to_owned())]);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_sync_provider_settings_outbox",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT consumed_generation FROM usage_sync_correction_lineage",
                    [],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn pending_batch_is_limited_to_sixty_two_rows() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for day_offset in (0..32).rev() {
            let candidate_now = now() - Duration::days(day_offset);
            let day = candidate_now.date().to_string();
            for provider in [CodingProvider::Codex, CodingProvider::Claude] {
                let mut value = aggregate(provider, day_offset as u64, 1000);
                value.ranking_day = day.clone();
                value.observed_at = offset_date_time_millis(candidate_now).unwrap();
                super::queue_daily_aggregate(&transaction, 1, value, candidate_now).unwrap();
            }
        }
        transaction.commit().unwrap();

        let batch = load_pending_usage_batch(&connection, 1).unwrap().unwrap();
        assert_eq!(batch.snapshots().len(), MAX_USAGE_SYNC_BATCH);
    }

    #[test]
    fn terminal_or_complete_transfer_day_rows_are_not_linked_as_carryovers() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        let mut partial = aggregate(CodingProvider::Codex, 50, 1_000);
        partial.coverage = SyncCoverage::Partial;
        super::queue_daily_aggregate(&transaction, 2, partial, now()).unwrap();
        super::queue_daily_aggregate(
            &transaction,
            2,
            aggregate(CodingProvider::Claude, 75, 2_000),
            now(),
        )
        .unwrap();
        transaction
            .execute(
                "INSERT INTO usage_sync_terminal_conflicts(
                     active_generation, provider, ranking_day, revision
                 ) VALUES(2, 'codex', '2026-08-08', 1)",
                [],
            )
            .unwrap();

        link_pending_transfer_day_segments(&transaction, 2, "2026-08-08", DAY_START_MILLIS)
            .unwrap();
        assert_eq!(
            transaction
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn delayed_installation_does_not_retag_an_existing_non_marker_row() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        super::queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Codex, 100, 1_000),
            now(),
        )
        .unwrap();
        activate_generation(&transaction, 2).unwrap();
        let mut existing = aggregate(CodingProvider::Codex, 50, 2_000);
        existing.coverage = SyncCoverage::Partial;
        super::queue_daily_aggregate(&transaction, 2, existing, now()).unwrap();

        assert_eq!(
            queue_transfer_day_carryover_markers(&transaction, 2, "2026-08-08", DAY_START_MILLIS,),
            Err(UsageSyncError::STORAGE_UNAVAILABLE)
        );
        assert_eq!(
            transaction
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            load_outbox_snapshot(&transaction, 2, CodingProvider::Codex, "2026-08-08",)
                .unwrap()
                .unwrap()
                .observed_tokens,
            50
        );
    }

    #[test]
    fn partial_activation_baseline_without_growth_retries_zero_after_restart() {
        let database = PersistentTestDatabase::new("partial-baseline-zero-rollover");
        let mut connection = database.connect();
        let next_day = now() + Duration::days(1);
        let enabled = BTreeSet::from([CodingProvider::Codex]);
        let state = state_with_totals(
            total(UsageEvidenceBasis::ProviderReported, 100, NOW, None),
            UsageTotal::Unavailable,
        );
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Codex, 100, 1_000),
        )
        .unwrap();
        activate_generation(&transaction, 2).unwrap();
        capture_generation_baselines(&transaction, 2, &state, now(), now()).unwrap();
        let updates = queue_current_utc_day(&transaction, 2, &state, now(), &enabled).unwrap();
        assert_eq!(
            updates,
            vec![QueueUpdate::Stored {
                provider: CodingProvider::Codex,
                revision: 1,
                state: QueueState::Pending,
            }]
        );
        let marker = load_outbox_snapshot(&transaction, 2, CodingProvider::Codex, "2026-08-08")
            .unwrap()
            .unwrap();
        assert_eq!(marker.observed_tokens, 0);
        assert_eq!(marker.coverage, SyncCoverage::Partial);
        capture_generation_baselines(&transaction, 2, &state, now(), next_day).unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let mut connection = database.connect();
        let pending = load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
            .unwrap()
            .unwrap();
        assert_eq!(pending.snapshots().len(), 1);
        assert_eq!(pending.snapshots()[0].revision, 1);
        assert_eq!(pending.snapshots()[0].observed_tokens, 0);
        assert_eq!(pending.snapshots()[0].coverage, SyncCoverage::Partial);
        assert_eq!(
            pending.transfer_day_carryover.as_ref().unwrap().kind,
            TransferDayCarryoverKind::PendingSegment
        );
        pending
            .mutation_args(INSTALLATION_CREDENTIAL, next_day)
            .unwrap();

        let committed = acknowledgement(
            &pending.snapshots()[0],
            AcknowledgementOutcome::Committed,
            1,
        );
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &pending, &[committed]).unwrap(),
            1
        );
        transaction.commit().unwrap();
        assert!(
            load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_latest_outbox
                     WHERE active_generation = 2",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn complete_activation_baseline_without_growth_does_not_queue_a_marker() {
        let mut connection = connection();
        let next_day = now() + Duration::days(1);
        let enabled = BTreeSet::from([CodingProvider::Codex]);
        let state = state_with_totals(
            total(UsageEvidenceBasis::ProviderReported, 100, NOW, None),
            UsageTotal::Unavailable,
        );
        let transaction = connection.transaction().unwrap();
        activate_generation(&transaction, 2).unwrap();
        capture_generation_baselines(&transaction, 2, &state, now(), now()).unwrap();
        assert!(
            queue_current_utc_day(&transaction, 2, &state, now(), &enabled)
                .unwrap()
                .is_empty()
        );
        capture_generation_baselines(&transaction, 2, &state, now(), next_day).unwrap();
        transaction.commit().unwrap();

        assert!(
            load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn stale_delayed_installation_marker_is_resolved_without_a_higher_revision() {
        let database = PersistentTestDatabase::new("stale-delayed-marker");
        let mut connection = database.connect();
        let next_day = now() + Duration::days(1);
        let enabled = BTreeSet::from([CodingProvider::Codex]);
        let transaction = connection.transaction().unwrap();
        queue_daily_aggregate(
            &transaction,
            1,
            aggregate(CodingProvider::Codex, 100, 1_000),
        )
        .unwrap();
        activate_generation(&transaction, 2).unwrap();
        capture_generation_baselines(
            &transaction,
            2,
            &state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable),
            now(),
            next_day,
        )
        .unwrap();
        transaction.commit().unwrap();

        let sent = load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
            .unwrap()
            .unwrap();
        assert_eq!(sent.snapshots().len(), 1);
        assert_eq!(sent.snapshots()[0].revision, 1);
        assert_eq!(sent.snapshots()[0].observed_tokens, 0);
        assert_eq!(
            sent.transfer_day_carryover.as_ref().unwrap().kind,
            TransferDayCarryoverKind::DelayedInstallationMarker
        );
        let stale = acknowledgement(&sent.snapshots()[0], AcknowledgementOutcome::Stale, 3);
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &[stale]).unwrap(),
            1
        );
        transaction.commit().unwrap();
        drop(connection);

        let connection = database.connect();
        assert!(
            load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn stale_zero_pending_segment_is_resolved_without_a_higher_revision() {
        let database = PersistentTestDatabase::new("stale-zero-pending-segment");
        let mut connection = database.connect();
        let next_day = now() + Duration::days(1);
        let enabled = BTreeSet::from([CodingProvider::Codex]);
        let transaction = connection.transaction().unwrap();
        let mut marker = aggregate(CodingProvider::Codex, 0, 2_000);
        marker.coverage = SyncCoverage::Partial;
        queue_validated_daily_aggregate(&transaction, 2, marker).unwrap();
        transaction
            .execute(
                "INSERT INTO usage_sync_generation_activations(
                     active_generation, ranking_day, activated_at
                 ) VALUES(2, '2026-08-08', ?1)",
                [to_database_integer(DAY_START_MILLIS + 1_000).unwrap()],
            )
            .unwrap();
        link_pending_transfer_day_segments(&transaction, 2, "2026-08-08", DAY_START_MILLIS + 1_000)
            .unwrap();
        transaction.commit().unwrap();

        let sent = load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
            .unwrap()
            .unwrap();
        assert_eq!(sent.snapshots().len(), 1);
        assert_eq!(sent.snapshots()[0].revision, 1);
        assert_eq!(sent.snapshots()[0].observed_tokens, 0);
        assert_eq!(
            sent.transfer_day_carryover.as_ref().unwrap().kind,
            TransferDayCarryoverKind::PendingSegment
        );
        let stale = acknowledgement(&sent.snapshots()[0], AcknowledgementOutcome::Stale, 3);
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &[stale]).unwrap(),
            1
        );
        transaction.commit().unwrap();
        drop(connection);

        let connection = database.connect();
        assert!(
            load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM usage_sync_latest_outbox", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn stale_nonzero_pending_segments_keep_their_tag_and_retry_after_restart() {
        let database = PersistentTestDatabase::new("stale-nonzero-pending-segments");
        let mut connection = database.connect();
        let next_day = now() + Duration::days(1);
        let enabled = enabled_providers();
        let transaction = connection.transaction().unwrap();
        for provider in [CodingProvider::Codex, CodingProvider::Claude] {
            let mut segment = aggregate(provider, 50, 2_000);
            segment.coverage = SyncCoverage::Partial;
            queue_validated_daily_aggregate(&transaction, 2, segment).unwrap();
        }
        transaction
            .execute(
                "INSERT INTO usage_sync_generation_activations(
                     active_generation, ranking_day, activated_at
                 ) VALUES(2, '2026-08-08', ?1)",
                [to_database_integer(DAY_START_MILLIS + 1_000).unwrap()],
            )
            .unwrap();
        link_pending_transfer_day_segments(&transaction, 2, "2026-08-08", DAY_START_MILLIS + 1_000)
            .unwrap();
        transaction.commit().unwrap();

        let sent = load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
            .unwrap()
            .unwrap();
        assert_eq!(sent.snapshots().len(), 2);
        assert_eq!(
            sent.transfer_day_carryover.as_ref().unwrap().kind,
            TransferDayCarryoverKind::PendingSegment
        );
        let stale = sent
            .snapshots()
            .iter()
            .map(|snapshot| acknowledgement(snapshot, AcknowledgementOutcome::Stale, 3))
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &sent, &stale).unwrap(),
            2
        );
        transaction.commit().unwrap();
        drop(connection);

        let mut connection = database.connect();
        let retry = load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
            .unwrap()
            .unwrap();
        assert_eq!(retry.snapshots().len(), 2);
        assert!(
            retry
                .snapshots()
                .iter()
                .all(|snapshot| snapshot.revision == 4 && snapshot.observed_tokens == 50)
        );
        assert_eq!(
            retry.transfer_day_carryover.as_ref().unwrap().kind,
            TransferDayCarryoverKind::PendingSegment
        );
        retry
            .mutation_args(INSTALLATION_CREDENTIAL, next_day)
            .unwrap();

        let terminal = retry
            .snapshots()
            .iter()
            .map(|snapshot| {
                acknowledgement(
                    snapshot,
                    match snapshot.provider {
                        CodingProvider::Codex => AcknowledgementOutcome::Committed,
                        CodingProvider::Claude => AcknowledgementOutcome::Idempotent,
                    },
                    4,
                )
            })
            .collect::<Vec<_>>();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            apply_usage_acknowledgements(&transaction, &retry, &terminal).unwrap(),
            2
        );
        transaction.commit().unwrap();
        assert!(
            load_next_pending_usage_batch(&connection, 2, next_day, &enabled)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn empty_current_day_queue_prunes_rows_older_than_sixty_utc_days() {
        let mut connection = connection();
        let transaction = connection.transaction().unwrap();
        for day_offset in (0..=USAGE_HISTORY_RETENTION_DAYS).rev() {
            let candidate_now = now() - Duration::days(day_offset);
            let day = candidate_now.date().to_string();
            for provider in [CodingProvider::Codex, CodingProvider::Claude] {
                let mut value = aggregate(provider, day_offset as u64, 1_000);
                value.ranking_day = day.clone();
                value.observed_at = offset_date_time_millis(candidate_now).unwrap();
                super::queue_daily_aggregate(&transaction, 1, value, candidate_now).unwrap();
            }
        }
        let expired_at = now() - Duration::days(USAGE_HISTORY_RETENTION_DAYS);
        transaction
            .execute(
                "INSERT INTO usage_sync_generations(active_generation, queue_state)
                 VALUES(2, 'abandoned')",
                [],
            )
            .unwrap();
        for generation in [1_i64, 2] {
            transaction
                .execute(
                    "INSERT INTO usage_sync_generation_activations(
                         active_generation, ranking_day, activated_at
                     ) VALUES(?1, ?2, ?3)",
                    params![
                        generation,
                        expired_at.date().to_string(),
                        offset_date_time_millis(expired_at).unwrap()
                    ],
                )
                .unwrap();
        }
        transaction
            .execute(
                "INSERT INTO usage_sync_transfer_day_carryovers(
                     active_generation, provider, ranking_day, carryover_kind
                 ) VALUES(1, 'codex', ?1, 'pending-segment')",
                [expired_at.date().to_string()],
            )
            .unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        queue_current_utc_day(
            &transaction,
            1,
            &state_with_totals(UsageTotal::Unavailable, UsageTotal::Unavailable),
            now(),
            &BTreeSet::new(),
        )
        .unwrap();
        transaction.commit().unwrap();

        let first_retained_day = (now() - Duration::days(USAGE_HISTORY_RETENTION_DAYS - 1))
            .date()
            .to_string();
        for table in ["usage_sync_daily_aggregates", "usage_sync_latest_outbox"] {
            let (count, first_day, last_day): (i64, String, String) = connection
                .query_row(
                    &format!("SELECT count(*), min(ranking_day), max(ranking_day) FROM {table}"),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(count, USAGE_HISTORY_RETENTION_DAYS * 2);
            assert_eq!(first_day, first_retained_day);
            assert_eq!(last_day, "2026-08-08");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM usage_sync_transfer_day_carryovers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        let activations = connection
            .prepare(
                "SELECT active_generation FROM usage_sync_generation_activations
                 ORDER BY active_generation",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(activations, vec![1]);
    }

    #[test]
    fn pruning_removes_only_unreferenced_abandoned_generation_metadata() {
        let mut connection = connection();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        let expired_at = now() - Duration::days(USAGE_HISTORY_RETENTION_DAYS);
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "INSERT INTO usage_sync_generations(active_generation, queue_state)
                     VALUES(1, 'active'), (2, 'abandoned'), (3, 'blocked'), (4, 'active');
                 INSERT INTO usage_sync_provider_settings_outbox(
                     active_generation, revision, codex_enabled, claude_enabled, delivery_state
                 ) VALUES
                     (1, 1, 1, 1, 'synced'),
                     (2, 1, 1, 1, 'abandoned'),
                     (3, 1, 1, 1, 'blocked'),
                     (4, 1, 1, 1, 'abandoned');",
            )
            .unwrap();
        for generation in 1_i64..=4 {
            transaction
                .execute(
                    "INSERT INTO usage_sync_generation_activations(
                         active_generation, ranking_day, activated_at
                     ) VALUES(?1, ?2, ?3)",
                    params![
                        generation,
                        expired_at.date().to_string(),
                        offset_date_time_millis(expired_at).unwrap()
                    ],
                )
                .unwrap();
        }
        super::queue_daily_aggregate(
            &transaction,
            4,
            aggregate(CodingProvider::Codex, 100, 1_000),
            now(),
        )
        .unwrap();
        transaction
            .execute(
                "UPDATE usage_sync_generations
                 SET queue_state = 'abandoned'
                 WHERE active_generation = 4",
                [],
            )
            .unwrap();
        transaction
            .execute(
                "UPDATE usage_sync_latest_outbox
                 SET queue_state = 'abandoned'
                 WHERE active_generation = 4",
                [],
            )
            .unwrap();

        assert_eq!(
            prune_expired_usage_sync_rows(&transaction, now()).unwrap(),
            4
        );
        transaction.commit().unwrap();

        let generations = connection
            .prepare(
                "SELECT active_generation FROM usage_sync_generations
                 ORDER BY active_generation",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(generations, vec![1, 3, 4]);
        let settings = connection
            .prepare(
                "SELECT active_generation FROM usage_sync_provider_settings_outbox
                 ORDER BY active_generation",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(settings, vec![1, 3, 4]);
        let activations = connection
            .prepare(
                "SELECT active_generation FROM usage_sync_generation_activations
                 ORDER BY active_generation",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(activations, vec![1, 3]);
    }

    #[test]
    fn acknowledgement_parser_is_strict_and_bounded() {
        let valid = br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"committed"}]"#;
        assert_eq!(parse_usage_acknowledgements(valid).unwrap().len(), 1);
        let conflict = br#"[{"provider":"claude","rankingDay":"2026-08-08","revision":1,"outcome":"conflict"}]"#;
        assert_eq!(
            parse_usage_acknowledgements(conflict).unwrap()[0].outcome,
            AcknowledgementOutcome::Conflict
        );
        assert_eq!(
            parse_provider_settings_acknowledgement(br#"{"revision":1,"outcome":"conflict"}"#),
            Err(UsageSyncError::INVALID_RESPONSE)
        );
        assert_eq!(parse_usage_acknowledgements(br#"[]"#).unwrap(), []);

        for invalid in [
            br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"committed","detail":"private"}]"#.as_slice(),
            br#"[{"provider":"hostile","rankingDay":"2026-08-08","revision":1,"outcome":"committed"}]"#.as_slice(),
            br#"[{"provider":"codex","rankingDay":"2026-08-08","revision":1,"outcome":"accepted"}]"#.as_slice(),
            br#"[{"provider":"codex","rankingDay":"2026-02-30","revision":1,"outcome":"committed"}]"#.as_slice(),
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
            provider_settings: None,
            snapshots: vec![UsageSyncSnapshot::from_aggregate(
                aggregate(CodingProvider::Codex, 10, 1000),
                1,
                None,
            )],
            transfer_day_carryover: None,
            profile_backfill_anchor: None,
            retained_history: false,
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
