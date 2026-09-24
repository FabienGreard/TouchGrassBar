use serde::{Deserialize, Serialize};

pub(crate) use crate::providers::CodingProvider as Provider;

macro_rules! codes {
    ($name:ident { $($value:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        // Keep the Rust names identical to the stable, prefixed wire codes.
        #[allow(clippy::enum_variant_names)]
        pub(crate) enum $name { $($value),+ }
    };
}

codes!(DatabaseCode {
    DatabaseOpenFailed,
    DatabaseOperationFailed,
    DatabaseMigrationFailed,
    DatabaseInvariantFailed,
    DatabaseVersionUnsupported
});
codes!(ParserCode {
    ParserScanFailed,
    ParserRecordInvalid,
    ParserInvariantFailed
});
codes!(PricingCode {
    PricingCalculationFailed,
    PricingCatalogNotApproved
});
codes!(SyncCode {
    SyncRequestFailed,
    SyncRevisionConflict
});
codes!(ProviderAccessCode {
    ProviderAccessFailed
});
codes!(BackupState {
    Absent,
    Present,
    Validated,
    Invalid,
    Unknown
});
codes!(ReviewStatus {
    Reviewed,
    Unreviewed,
    Mixed,
    Unknown
});
codes!(ParserReason {
    ReadFailed,
    ScanIncomplete,
    InvalidJson,
    InvalidUsageShape,
    InvalidCounter,
    UnsupportedRecord,
    InvariantFailed,
    RecordTooLarge,
    MissingCacheCounters,
    CacheSplitMismatch,
    IterationShapeMismatch,
    ThinkingCounterInvalid,
    UnknownUsageFields,
    FallbackCreditUnsupported
});
codes!(PricingReason {
    UnknownModel,
    CatalogUnavailable,
    CatalogNotApproved,
    MissingUsageMetadata,
    UnsupportedModifier,
    CounterOverflow,
    InvalidCost,
    InvalidCounter,
    MissingCacheWriteSplit,
    MissingWebSearchUsage,
    UnpricedCodeExecution,
    UnknownPaidServerTool,
    MissingEffectivePrice,
    UnknownServiceTier,
    UnknownInferenceGeo,
    FastBatchCombination,
    MissingFastPrice,
    MissingSpeed,
    UnknownSpeed,
    MissingCacheWritePrice
});
codes!(SyncStage {
    ProviderSettings,
    DailyUsage
});
codes!(SyncReason {
    TransportFailed,
    ServerRejected,
    AuthorityRejected,
    RevisionConflict,
    InvalidResponse
});
codes!(AccessOperation {
    ReadUsage,
    ReadQuota,
    RefreshCredentials,
    RefreshProvider
});
codes!(AccessReason {
    ReadFailed,
    PermissionDenied,
    RequestFailed,
    InvalidResponse,
    CredentialsRejected,
    DeadlineExceeded,
    AdapterPanicked
});
codes!(Architecture {
    Aarch64,
    X86_64,
    Unknown
});

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DatabaseModule {
    pub module: String,
    pub observed_version: Option<u64>,
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DatabaseContext {
    pub stage: Option<String>,
    pub observed_format: Option<u64>,
    pub expected_format: Option<u64>,
    pub modules: Vec<DatabaseModule>,
    pub backup_state: BackupState,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ParserContext {
    pub parser_version: Option<u64>,
    pub source_versions: Vec<String>,
    pub review_status: ReviewStatus,
    pub files_seen: Option<u64>,
    pub records_accepted: Option<u64>,
    pub records_rejected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records_affected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records_excluded: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<super::scan::UsageScanContext>,
    pub reason: ParserReason,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PricingContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub ranking_day: Option<String>,
    pub revision: Option<u64>,
    pub parser_version: Option<u64>,
    pub catalog_version: Option<String>,
    pub reason: PricingReason,
    pub observed_tokens: Option<u64>,
    pub priced_tokens: Option<u64>,
    pub local_cost_micros: Option<u64>,
    pub outgoing_cost_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<super::scan::UsageScanContext>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SyncContext {
    pub stage: SyncStage,
    pub reason: SyncReason,
    pub status_code: Option<u64>,
    pub ranking_day: Option<String>,
    pub attempted_revision: Option<u64>,
    pub last_acknowledged_revision: Option<u64>,
    pub pending_count: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProviderAccessContext {
    pub operation: AccessOperation,
    pub reason: AccessReason,
    pub status_code: Option<u64>,
    pub retry_count: u64,
}

/// This enum is the complete upload boundary. It has no free-form error or log field.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "area", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Failure {
    Database {
        code: DatabaseCode,
        provider: Option<Provider>,
        context: DatabaseContext,
    },
    Parser {
        code: ParserCode,
        provider: Provider,
        context: ParserContext,
    },
    Pricing {
        code: PricingCode,
        provider: Provider,
        context: PricingContext,
    },
    Sync {
        code: SyncCode,
        provider: Option<Provider>,
        context: SyncContext,
    },
    ProviderAccess {
        code: ProviderAccessCode,
        provider: Provider,
        context: ProviderAccessContext,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppContext {
    pub version: String,
    pub build: Option<String>,
    pub os_version: Option<String>,
    pub architecture: Architecture,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Report {
    pub schema_version: u8,
    pub report_id: String,
    pub first_occurred_at: u64,
    pub last_occurred_at: u64,
    pub context_captured_at: u64,
    pub occurrence_count: u64,
    pub app: AppContext,
    pub failure: Failure,
}

pub(super) fn numeric_version(value: &str) -> bool {
    value.len() <= 32
        && (1..=4).contains(&value.split('.').count())
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn source_version(value: &str) -> bool {
    if value.len() > 32 {
        return false;
    }
    if let Some((release, prerelease)) = value.split_once('-') {
        let Some((channel, sequence)) = prerelease.split_once('.') else {
            return false;
        };
        numeric_version(release)
            && matches!(channel, "alpha" | "beta" | "rc")
            && (1..=3).contains(&sequence.split('.').count())
            && sequence
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    } else {
        numeric_version(value)
    }
}

pub(super) fn day(value: &str) -> bool {
    value.len() == 10 && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

impl Failure {
    pub(super) fn group(&self) -> String {
        // The context can change between occurrences. It must not create a new rate-limit group.
        let value = serde_json::to_value(self).expect("fixed diagnostic fields serialize");
        let context = &value["context"];
        let mut group = serde_json::json!([
            value["area"],
            value["code"],
            value["provider"],
            context["reason"],
            context["rankingDay"],
            context["stage"],
            context["parserVersion"],
            context["catalogVersion"],
            context["sourceVersions"],
            context["reviewStatus"],
            context["revision"],
            context["attemptedRevision"],
            context["modules"],
            context["observedFormat"],
            context["expectedFormat"]
        ]);
        if let Some(model) = context.get("model") {
            group
                .as_array_mut()
                .expect("fixed group is an array")
                .push(model.clone());
        }
        group.to_string()
    }

    pub(super) fn valid(&self) -> bool {
        let strings_valid = match self {
            Self::Database {
                provider, context, ..
            } => {
                provider.is_none()
                    && context
                        .stage
                        .as_deref()
                        .is_none_or(super::validation::database_stage)
                    && context.modules.len() <= 16
                    && context
                        .modules
                        .iter()
                        .all(|item| super::validation::database_module(&item.module))
            }
            Self::Parser { context, .. } => {
                context.ranking_day.as_deref().is_none_or(day)
                    && context.scan.as_ref().is_none_or(|scan| scan.valid())
                    && context.records_excluded.is_none_or(|excluded| {
                        context
                            .records_affected
                            .is_some_and(|affected| excluded <= affected)
                    })
                    && context.source_versions.len() <= 8
                    && context
                        .source_versions
                        .iter()
                        .all(|value| source_version(value))
            }
            Self::Pricing {
                code,
                context,
                provider,
            } => {
                context.ranking_day.as_deref().is_none_or(day)
                    && context.model.as_deref().is_none_or(|model| {
                        context.reason == PricingReason::UnknownModel
                            && super::validation::model_identifier(*provider, model)
                    })
                    && context.scan.as_ref().is_none_or(|scan| scan.valid())
                    && context.revision.is_none_or(|value| value > 0)
                    && context
                        .catalog_version
                        .as_deref()
                        .is_none_or(super::validation::catalog_version)
                    && context
                        .priced_tokens
                        .zip(context.observed_tokens)
                        .is_none_or(|(priced, observed)| priced <= observed)
                    && (*code != PricingCode::PricingCatalogNotApproved
                        || (context.reason == PricingReason::CatalogNotApproved
                            && context.local_cost_micros.is_some()
                            && context.outgoing_cost_micros.is_none()))
            }
            Self::Sync {
                code,
                provider,
                context,
            } => {
                context.ranking_day.as_deref().is_none_or(day)
                    && context
                        .status_code
                        .is_none_or(|value| (100..=599).contains(&value))
                    && context.attempted_revision.is_none_or(|value| value > 0)
                    && context
                        .last_acknowledged_revision
                        .is_none_or(|value| value > 0)
                    && match context.stage {
                        SyncStage::ProviderSettings => {
                            provider.is_none() && context.ranking_day.is_none()
                        }
                        SyncStage::DailyUsage => {
                            provider.is_some()
                                || (*code == SyncCode::SyncRequestFailed
                                    && context.ranking_day.is_none()
                                    && context.attempted_revision.is_none()
                                    && context.last_acknowledged_revision.is_none()
                                    && context.pending_count == Some(0))
                        }
                    }
                    && (*code != SyncCode::SyncRevisionConflict
                        || context.reason == SyncReason::RevisionConflict)
            }
            Self::ProviderAccess { context, .. } => context
                .status_code
                .is_none_or(|value| (100..=599).contains(&value)),
        };
        fn safe_numbers(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::Number(number) => {
                    number.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991)
                }
                serde_json::Value::Array(items) => items.iter().all(safe_numbers),
                serde_json::Value::Object(items) => items.iter().all(|(key, value)| {
                    safe_numbers(value)
                        && (!matches!(
                            key.as_str(),
                            "parserVersion"
                                | "aggregateParserVersion"
                                | "observedFormat"
                                | "expectedFormat"
                                | "observedVersion"
                                | "expectedVersion"
                        ) || value.is_null()
                            || value.as_u64().is_some_and(|n| n <= 2_147_483_647))
                }),
                _ => true,
            }
        }
        strings_valid && serde_json::to_value(self).is_ok_and(|value| safe_numbers(&value))
    }
}

impl Report {
    pub(super) fn valid(&self, now: u64) -> bool {
        self.schema_version == 1
            && self.report_id.len() == 36
            && self.report_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                }
            })
            && (1..=10_000).contains(&self.occurrence_count)
            && self.first_occurred_at <= self.last_occurred_at
            && self.context_captured_at >= self.first_occurred_at
            && self.context_captured_at <= self.last_occurred_at
            && self.last_occurred_at <= now.saturating_add(5 * 60_000)
            && numeric_version(&self.app.version)
            && self.app.os_version.as_deref().is_none_or(numeric_version)
            && self.app.build.as_ref().is_none_or(|value| {
                !value.is_empty()
                    && value.len() <= 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            && self.failure.valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_model_validation_and_grouping_match_the_backend_contract() {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/contracts/fixtures/diagnostic-reports-v1.json"
        ))
        .unwrap();
        let reports: Vec<Report> = serde_json::from_value(raw).unwrap();
        let original = reports.iter().find(|r| matches!(&r.failure, Failure::Pricing { context, .. } if context.model.is_some())).unwrap().failure.clone();
        let mut other = original.clone();
        if let Failure::Pricing { context, .. } = &mut other {
            context.model = Some("gpt-99-two".to_owned());
        }
        assert_ne!(original.group(), other.group());
        for model in [
            "/private/model",
            "gpt-99 secret",
            "gpt-99\nsecret",
            "gpt-99@host",
            "claude-future-99",
        ] {
            if let Failure::Pricing { context, .. } = &mut other {
                context.model = Some(model.to_owned());
            }
            assert!(!other.valid());
        }
        if let Failure::Pricing { context, .. } = &mut other {
            context.model = Some("gpt-99-one".to_owned());
            context.reason = PricingReason::MissingEffectivePrice;
        }
        assert!(!other.valid());
    }

    #[test]
    fn shared_backend_fixtures_round_trip_without_schema_drift() {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/contracts/fixtures/diagnostic-reports-v1.json"
        ))
        .unwrap();
        let reports: Vec<Report> = serde_json::from_value(raw.clone()).unwrap();
        let areas: std::collections::BTreeSet<_> = raw
            .as_array()
            .unwrap()
            .iter()
            .map(|report| report["failure"]["area"].as_str().unwrap())
            .collect();
        assert_eq!(
            areas,
            std::collections::BTreeSet::from([
                "database",
                "parser",
                "pricing",
                "sync",
                "provider_access"
            ])
        );
        for report in &reports {
            assert!(report.valid(1_789_128_000_000), "{}", report.report_id);
        }
        assert_eq!(serde_json::to_value(reports).unwrap(), raw);
    }
}
