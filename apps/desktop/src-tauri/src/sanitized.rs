use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDesktopState {
    pub contract_version: u8,
    pub generated_at: String,
    pub providers: Vec<ProviderSnapshot>,
    pub usage: UsageByProvider,
    pub sync: SyncState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub provider: &'static str,
    pub detected: bool,
    pub freshness: &'static str,
    pub observed_at: String,
    pub quota_lanes: Vec<QuotaLane>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaLane {
    pub label: &'static str,
    pub unit: &'static str,
    pub allowance: Option<f64>,
    pub remaining: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct UsageByProvider {
    pub codex: UsagePeriods,
    pub claude: UsagePeriods,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriods {
    pub today: UsageTotal,
    pub seven_days: UsageTotal,
    pub thirty_days: UsageTotal,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotal {
    pub observed_tokens: u64,
    pub api_equivalent_cost_usd: Option<f64>,
    pub cost_is_complete: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub status: &'static str,
    pub last_successful_at: Option<String>,
}

fn empty_usage() -> UsagePeriods {
    let total = UsageTotal {
        observed_tokens: 0,
        api_equivalent_cost_usd: None,
        cost_is_complete: false,
    };
    UsagePeriods {
        today: total.clone(),
        seven_days: total.clone(),
        thirty_days: total,
    }
}

pub fn unavailable_state() -> SanitizedDesktopState {
    let observed_at = "2026-08-03T00:00:00Z".to_owned();
    SanitizedDesktopState {
        contract_version: 1,
        generated_at: observed_at.clone(),
        providers: vec![
            ProviderSnapshot {
                provider: "codex",
                detected: false,
                freshness: "unavailable",
                observed_at: observed_at.clone(),
                quota_lanes: Vec::new(),
            },
            ProviderSnapshot {
                provider: "claude",
                detected: false,
                freshness: "unavailable",
                observed_at,
                quota_lanes: Vec::new(),
            },
        ],
        usage: UsageByProvider {
            codex: empty_usage(),
            claude: empty_usage(),
        },
        sync: SyncState {
            status: "unavailable",
            last_successful_at: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_state_has_only_supported_providers() {
        let state = unavailable_state();
        let providers = state
            .providers
            .iter()
            .map(|snapshot| snapshot.provider)
            .collect::<Vec<_>>();
        assert_eq!(providers, ["codex", "claude"]);
    }
}
