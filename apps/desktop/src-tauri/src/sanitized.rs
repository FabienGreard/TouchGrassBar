use std::sync::Mutex;

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const CONTRACT_VERSION: u8 = 1;
pub const REVISION_NOTICE_EVENT: &str = "sanitized-desktop-state-revision";

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDesktopStateV1 {
    pub contract_version: u8,
    pub generated_at: String,
    pub revision: String,
    pub providers: [ProviderSnapshot; 2],
    pub usage: UsageByProvider,
    pub sync: SyncState,
}

#[allow(dead_code)]
#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "availability"
)]
pub enum ProviderSnapshot {
    Unavailable {
        provider: CodingProvider,
        quota_lanes: [QuotaLane; 0],
    },
    Current {
        provider: CodingProvider,
        observed_at: String,
        #[schemars(length(min = 1))]
        quota_lanes: Vec<QuotaLane>,
    },
    Stale {
        provider: CodingProvider,
        observed_at: String,
        #[schemars(length(min = 1))]
        quota_lanes: Vec<QuotaLane>,
    },
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodingProvider {
    Codex,
    Claude,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaLane {
    #[schemars(length(min = 1))]
    pub label: String,
    #[schemars(length(min = 1))]
    pub unit: String,
    pub allowance: Option<f64>,
    pub remaining: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct UsageByProvider {
    pub codex: UsagePeriods,
    pub claude: UsagePeriods,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriods {
    pub today: UsageTotal,
    pub seven_days: UsageTotal,
    pub thirty_days: UsageTotal,
}

#[allow(dead_code)]
#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "availability"
)]
pub enum UsageTotal {
    Unavailable,
    Current {
        evidence_basis: UsageEvidenceBasis,
        coverage: UsageCoverage,
        observed_at: String,
        observed_tokens: u64,
        api_equivalent_cost_usd: Option<f64>,
    },
    Stale {
        evidence_basis: UsageEvidenceBasis,
        coverage: UsageCoverage,
        observed_at: String,
        observed_tokens: u64,
        api_equivalent_cost_usd: Option<f64>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageEvidenceBasis {
    ProviderReported,
    LocallyDerived,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageCoverage {
    Complete,
    Partial,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub status: SyncStatus,
    pub last_successful_at: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Synced,
    Pending,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct RevisionNotice {
    pub revision: String,
}

#[derive(Debug)]
pub struct NativeCore {
    state: Mutex<SanitizedDesktopStateV1>,
}

impl NativeCore {
    pub fn unavailable() -> Self {
        Self {
            state: Mutex::new(unavailable_state(1)),
        }
    }

    pub fn panel_state(&self) -> Result<SanitizedDesktopStateV1, &'static str> {
        self.state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "native state unavailable")
    }

    pub fn request_refresh(&self) -> Result<RevisionNotice, &'static str> {
        let mut state = self.state.lock().map_err(|_| "native state unavailable")?;
        let next_revision = state
            .revision
            .parse::<u64>()
            .unwrap_or_default()
            .saturating_add(1);
        *state = unavailable_state(next_revision);
        Ok(RevisionNotice {
            revision: state.revision.clone(),
        })
    }
}

fn unavailable_periods() -> UsagePeriods {
    UsagePeriods {
        today: UsageTotal::Unavailable,
        seven_days: UsageTotal::Unavailable,
        thirty_days: UsageTotal::Unavailable,
    }
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

pub fn unavailable_state(revision: u64) -> SanitizedDesktopStateV1 {
    SanitizedDesktopStateV1 {
        contract_version: CONTRACT_VERSION,
        generated_at: now(),
        revision: revision.max(1).to_string(),
        providers: [
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Codex,
                quota_lanes: [],
            },
            ProviderSnapshot::Unavailable {
                provider: CodingProvider::Claude,
                quota_lanes: [],
            },
        ],
        usage: UsageByProvider {
            codex: unavailable_periods(),
            claude: unavailable_periods(),
        },
        sync: SyncState {
            status: SyncStatus::Unavailable,
            last_successful_at: None,
        },
    }
}

pub fn native_contract_schema() -> Schema {
    schema_for!(SanitizedDesktopStateV1)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn unavailable_snapshot_never_invents_zero_usage() {
        let value = serde_json::to_value(unavailable_state(1)).unwrap();
        assert_eq!(value["contractVersion"], CONTRACT_VERSION);
        assert_eq!(value["revision"], "1");
        assert_eq!(
            value["usage"]["codex"]["today"],
            json!({ "availability": "unavailable" })
        );
        assert!(value.to_string().find("observedTokens").is_none());
    }

    #[test]
    fn sanitized_snapshot_cannot_contain_privileged_field_names() {
        let value = serde_json::to_value(unavailable_state(1)).unwrap();
        let prohibited = [
            "credential",
            "cookie",
            "path",
            "prompt",
            "raw",
            "session",
            "tokenmaxxerId",
        ];

        fn assert_clean(value: &Value, prohibited: &[&str]) {
            match value {
                Value::Object(fields) => {
                    for (key, child) in fields {
                        let normalized = key.to_lowercase();
                        assert!(
                            prohibited.iter().all(|word| !normalized.contains(word)),
                            "prohibited field serialized: {key}"
                        );
                        assert_clean(child, prohibited);
                    }
                }
                Value::Array(values) => {
                    for child in values {
                        assert_clean(child, prohibited);
                    }
                }
                _ => {}
            }
        }

        assert_clean(&value, &prohibited);
    }

    #[test]
    fn refresh_commits_before_returning_a_higher_revision() {
        let core = NativeCore::unavailable();
        let notice = core.request_refresh().unwrap();
        let state = core.panel_state().unwrap();
        assert_eq!(notice.revision, "2");
        assert_eq!(state.revision, notice.revision);
    }
}
