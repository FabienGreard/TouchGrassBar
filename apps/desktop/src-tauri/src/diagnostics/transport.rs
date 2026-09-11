#[cfg(target_os = "macos")]
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
use crate::profile::{
    ActiveSyncCredentials, HttpProfileTransport, MacKeychain, ProfileCoordinator,
};
use crate::profile::{Secret, SecretCustody, SecretKind};

#[cfg(target_os = "macos")]
use super::{Delivery, Report, Shared};
use super::{DiagnosticError, queue::Binding};

/// The secret and its authority are one atomic Keychain item. The queue contains
/// only Binding. No Profile or SQLite token is needed to read this item.
pub(super) struct Credential {
    touch_grass_id: String,
    generation: u64,
    reporter_id: Option<String>,
    installation_fingerprint: String,
    secret: Secret,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCredential {
    touch_grass_id: String,
    generation: u64,
    reporter_id: Option<String>,
    installation_fingerprint: String,
    diagnostic_credential: String,
}

impl Credential {
    pub fn binding(&self) -> Option<Binding> {
        self.reporter_id.as_ref().map(|reporter_id| Binding {
            reporter_id: reporter_id.clone(),
            generation: self.generation,
        })
    }

    fn decode(secret: &Secret) -> Result<Self, DiagnosticError> {
        if secret.expose().len() > 1024 {
            return Err(DiagnosticError);
        }
        let stored: StoredCredential =
            serde_json::from_str(secret.expose()).map_err(|_| DiagnosticError)?;
        let diagnostic_secret = Secret::new(stored.diagnostic_credential);
        if !crate::profile::valid_touch_grass_id(&stored.touch_grass_id)
            || !(1..=9_007_199_254_740_991).contains(&stored.generation)
            || stored.reporter_id.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            || diagnostic_secret.expose().len() != 64
            || stored.installation_fingerprint.len() != 64
            || !stored
                .installation_fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !diagnostic_secret
                .expose()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(DiagnosticError);
        }
        Ok(Self {
            touch_grass_id: stored.touch_grass_id,
            generation: stored.generation,
            reporter_id: stored.reporter_id,
            installation_fingerprint: stored.installation_fingerprint,
            secret: diagnostic_secret,
        })
    }

    fn save(&self, custody: &dyn SecretCustody) -> Result<(), DiagnosticError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Stored<'a> {
            touch_grass_id: &'a str,
            generation: u64,
            reporter_id: Option<&'a str>,
            installation_fingerprint: &'a str,
            diagnostic_credential: &'a str,
        }
        let value = Secret::new(
            serde_json::to_string(&Stored {
                touch_grass_id: &self.touch_grass_id,
                generation: self.generation,
                reporter_id: self.reporter_id.as_deref(),
                installation_fingerprint: &self.installation_fingerprint,
                diagnostic_credential: self.secret.expose(),
            })
            .map_err(|_| DiagnosticError)?,
        );
        custody
            .write(SecretKind::DiagnosticReporter, &value)
            .map_err(|_| DiagnosticError)
    }
}

fn read_from(custody: &dyn SecretCustody) -> Result<Option<Credential>, DiagnosticError> {
    match custody
        .read(SecretKind::DiagnosticReporter)
        .map_err(|_| DiagnosticError)?
    {
        Some(value) => match Credential::decode(&value) {
            Ok(credential) => {
                let installation = custody
                    .read(SecretKind::InstallationCredential)
                    .map_err(|_| DiagnosticError)?;
                if installation.as_ref().is_none_or(|installation| {
                    installation_fingerprint(installation) != credential.installation_fingerprint
                }) {
                    let _ = custody.delete(SecretKind::DiagnosticReporter);
                    return Ok(None);
                }
                Ok(Some(credential))
            }
            Err(error) => {
                let _ = custody.delete(SecretKind::DiagnosticReporter);
                Err(error)
            }
        },
        None => Ok(None),
    }
}

fn installation_fingerprint(secret: &Secret) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"touchgrass-diagnostic-installation-v1:");
    hash.update(secret.expose().as_bytes());
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(target_os = "macos")]
pub(super) fn read_credential() -> Result<Option<Credential>, DiagnosticError> {
    read_from(&MacKeychain)
}

fn recovery_pending(custody: &dyn SecretCustody) -> Result<bool, DiagnosticError> {
    custody
        .read(SecretKind::RecoveryPreparation)
        .map(|prepared| prepared.is_some())
        .map_err(|_| DiagnosticError)
}

fn restore_binding(
    queue: &mut super::queue::Queue,
    custody: &dyn SecretCustody,
) -> Result<bool, DiagnosticError> {
    if queue.suspended() || recovery_pending(custody)? {
        queue.suspend()?;
        return Ok(true);
    }
    queue.bind(read_from(custody)?.and_then(|credential| credential.binding()))?;
    Ok(false)
}

#[cfg(target_os = "macos")]
pub(super) fn initialize_binding(queue: &mut super::queue::Queue) -> Result<bool, DiagnosticError> {
    restore_binding(queue, &MacKeychain)
}

#[cfg(target_os = "macos")]
pub(super) struct Client {
    http: reqwest::blocking::Client,
    registration: HttpProfileTransport,
    next_registration_at: u64,
    registration_attempts: u32,
}

#[cfg(target_os = "macos")]
impl Client {
    pub fn new() -> Self {
        Self {
            http: crate::native_https_client(),
            registration: HttpProfileTransport::from_build_configuration(),
            next_registration_at: 0,
            registration_attempts: 0,
        }
    }

    pub fn pump(&mut self, shared: &Arc<Shared>, profile: &Arc<Mutex<ProfileCoordinator>>) {
        use std::sync::atomic::Ordering;
        let now = super::now();
        let mut authority_epoch = shared.authority_epoch.load(Ordering::SeqCst);
        match recovery_pending(&MacKeychain) {
            Ok(true) => {
                super::suspend(shared);
                super::persist_suspension(shared);
                return;
            }
            Err(_) => return,
            Ok(false) => {}
        }
        let Ok(mut credential) = read_credential() else {
            return;
        };
        let current = profile
            .try_lock()
            .ok()
            .and_then(|profile| profile.cached_diagnostic_registration());
        if shared.suspended.load(Ordering::SeqCst) && current.is_none() {
            return;
        }
        if let Some((touch_grass_id, credentials)) = &current {
            if credential.as_ref().is_none_or(|value| {
                value.touch_grass_id != *touch_grass_id
                    || value.generation != credentials.active_mac_generation
            }) {
                if now < self.next_registration_at {
                    return;
                }
                // Drop all earlier authority before staging a new credential.
                super::suspend(shared);
                super::persist_suspension(shared);
                authority_epoch = shared.authority_epoch.load(Ordering::SeqCst);
                let mut random = [0u8; 32];
                if getrandom::fill(&mut random).is_err() {
                    return;
                }
                let new = Credential {
                    touch_grass_id: touch_grass_id.clone(),
                    generation: credentials.active_mac_generation,
                    reporter_id: None,
                    installation_fingerprint: installation_fingerprint(
                        &credentials.installation_credential,
                    ),
                    secret: Secret::new(random.iter().map(|byte| format!("{byte:02x}")).collect()),
                };
                use zeroize::Zeroize;
                random.zeroize();
                if new.save(&MacKeychain).is_err() {
                    return;
                }
                credential = Some(new);
            }
        }
        let Some(mut credential) = credential else {
            return;
        };
        if credential.reporter_id.is_none() || shared.suspended.load(Ordering::SeqCst) {
            let Some((_, credentials)) = current else {
                return;
            };
            if now < self.next_registration_at {
                return;
            }
            self.registration_attempts = self.registration_attempts.saturating_add(1);
            self.next_registration_at = now.saturating_add(
                30_000_u64
                    .saturating_mul(1 << self.registration_attempts.min(7))
                    .min(60 * 60_000),
            );
            let Ok(reporter_id) = self.register(&credential, &credentials) else {
                return;
            };
            credential.reporter_id = Some(reporter_id);
            if credential.save(&MacKeychain).is_err() {
                return;
            }
            self.registration_attempts = 0;
        }
        let Some(binding) = credential.binding() else {
            return;
        };
        let report = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            if shared.authority_epoch.load(Ordering::SeqCst) != authority_epoch {
                return;
            }
            if !queue.matches_binding(&binding) {
                authority_epoch = shared
                    .authority_epoch
                    .fetch_add(1, Ordering::SeqCst)
                    .saturating_add(1);
            }
            if queue.resume(binding.clone()).is_err() {
                return;
            }
            shared.suspended.store(false, Ordering::SeqCst);
            if shared.authority_epoch.load(Ordering::SeqCst) != authority_epoch {
                shared.suspended.store(true, Ordering::SeqCst);
                let _ = queue.bind(None);
                return;
            }
            let Ok(report) = queue.next(&binding, now) else {
                return;
            };
            report
        };
        let Some(report) = report else {
            return;
        };
        if shared.authority_epoch.load(Ordering::SeqCst) != authority_epoch
            || shared.suspended.load(Ordering::SeqCst)
        {
            return;
        }
        let outcome = self.submit(&credential, &report);
        if matches!(outcome, Delivery::Revoked) {
            let _ = MacKeychain.delete(SecretKind::DiagnosticReporter);
            self.next_registration_at = now.saturating_add(60 * 60_000);
        }
        if let Ok(mut queue) = shared.queue.lock() {
            let _ = queue.finish(&report.report_id, outcome, super::now());
        }
    }

    fn register(
        &self,
        credential: &Credential,
        credentials: &ActiveSyncCredentials,
    ) -> Result<String, DiagnosticError> {
        use convex::Value;
        let payload = BTreeMap::from([
            (
                "installationCredential".into(),
                Value::String(credentials.installation_credential.expose().to_owned()),
            ),
            (
                "activeMacGeneration".into(),
                Value::Float64(credentials.active_mac_generation as f64),
            ),
            (
                "diagnosticCredential".into(),
                Value::String(credential.secret.expose().to_owned()),
            ),
        ]);
        let result = self
            .registration
            .mutate_profile(
                &credentials.session,
                "diagnostics:register",
                payload,
                "diagnostic registration unavailable",
            )
            .map_err(|_| DiagnosticError)?;
        let Value::Object(fields) = result else {
            return Err(DiagnosticError);
        };
        let Some(Value::String(reporter_id)) = fields.get("reporterId") else {
            return Err(DiagnosticError);
        };
        if !matches!(fields.get("generation"), Some(Value::Float64(generation)) if *generation == credential.generation as f64)
            || reporter_id.is_empty()
            || reporter_id.len() > 128
            || !reporter_id.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(DiagnosticError);
        }
        Ok(reporter_id.clone())
    }

    fn submit(&self, credential: &Credential, report: &Report) -> Delivery {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Arguments<'a> {
            reporter_id: &'a str,
            diagnostic_credential: &'a str,
            report: &'a Report,
        }
        #[derive(Serialize)]
        struct Mutation<'a> {
            path: &'static str,
            format: &'static str,
            args: Arguments<'a>,
        }
        let Some(url) = option_env!("CONVEX_URL").filter(|url| !url.is_empty()) else {
            return Delivery::Retry(60 * 60_000);
        };
        let Some(reporter_id) = credential.reporter_id.as_deref() else {
            return Delivery::Revoked;
        };
        let response = self
            .http
            .post(format!("{}/api/mutation", url.trim_end_matches('/')))
            .timeout(Duration::from_secs(10))
            .json(&Mutation {
                path: "diagnostics:submit",
                format: "json",
                args: Arguments {
                    reporter_id,
                    diagnostic_credential: credential.secret.expose(),
                    report,
                },
            })
            .send();
        let Ok(response) = response else {
            return Delivery::Retry(30_000);
        };
        let status = response.status();
        let mut body = Vec::new();
        if response.take(8193).read_to_end(&mut body).is_err() || body.len() > 8192 {
            return Delivery::Retry(30_000);
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return Delivery::Retry(30_000);
        };
        decode_delivery(status.is_success(), &value)
    }
}

fn decode_delivery(http_success: bool, value: &serde_json::Value) -> super::Delivery {
    use super::Delivery;
    if let Some(code) = value.get("errorData").and_then(serde_json::Value::as_str) {
        return match code {
            "DIAGNOSTIC_AUTHORITY_REJECTED" => Delivery::Revoked,
            "DIAGNOSTIC_REPORT_INVALID"
            | "DIAGNOSTIC_REPORT_TOO_LARGE"
            | "DIAGNOSTIC_REPORT_CONFLICT"
            | "DIAGNOSTIC_REPORT_EXPIRED" => Delivery::Rejected,
            _ => Delivery::Retry(30_000),
        };
    }
    if !http_success || value["status"] != "success" {
        return Delivery::Retry(30_000);
    }
    match value["value"]["outcome"].as_str() {
        Some("accepted" | "duplicate") => Delivery::Accepted,
        Some("rate_limited") => {
            Delivery::Retry(value["value"]["retryAfterMs"].as_u64().unwrap_or(60_000))
        }
        _ => Delivery::Retry(30_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryCustody(std::sync::Mutex<std::collections::BTreeMap<SecretKind, Secret>>);
    impl SecretCustody for MemoryCustody {
        fn delete(&self, kind: SecretKind) -> Result<(), crate::profile::ProfileError> {
            self.0.lock().unwrap().remove(&kind);
            Ok(())
        }
        fn read(&self, kind: SecretKind) -> Result<Option<Secret>, crate::profile::ProfileError> {
            Ok(self.0.lock().unwrap().get(&kind).cloned())
        }
        fn write(
            &self,
            kind: SecretKind,
            value: &Secret,
        ) -> Result<(), crate::profile::ProfileError> {
            self.0.lock().unwrap().insert(kind, value.clone());
            Ok(())
        }
    }

    #[test]
    fn credential_is_a_single_private_keychain_item_and_rejects_bad_bindings() {
        let valid = Secret::new(format!(
            r#"{{"touchGrassId":"TG-AAAAAA","generation":1,"reporterId":"testreporter","diagnosticCredential":"{}","installationFingerprint":"{}"}}"#,
            "a".repeat(64),
            "b".repeat(64)
        ));
        let credential = Credential::decode(&valid).unwrap();
        assert_eq!(credential.binding().unwrap().generation, 1);
        assert!(!format!("{:?}", credential.secret).contains(&"a".repeat(64)));
        assert!(
            Credential::decode(&Secret::new(
                valid.expose().replace("TG-AAAAAA", "/Users/private")
            ))
            .is_err()
        );
        let policy = crate::profile::keychain_policy(SecretKind::DiagnosticReporter);
        assert!(!policy.synchronized);
        assert_eq!(
            policy.accessibility,
            crate::profile::Accessibility::AfterFirstUnlockThisDeviceOnly
        );
    }

    #[test]
    fn only_structured_authority_errors_revoke_and_retries_are_not_new_reports() {
        assert!(matches!(
            decode_delivery(
                false,
                &serde_json::json!({"errorData":"DIAGNOSTIC_AUTHORITY_REJECTED"})
            ),
            super::super::Delivery::Revoked
        ));
        assert!(matches!(
            decode_delivery(
                false,
                &serde_json::json!({"errorMessage":"DIAGNOSTIC_AUTHORITY_REJECTED"})
            ),
            super::super::Delivery::Retry(_)
        ));
        assert!(matches!(
            decode_delivery(
                true,
                &serde_json::json!({"status":"success","value":{"outcome":"duplicate"}})
            ),
            super::super::Delivery::Accepted
        ));
        assert!(matches!(
            decode_delivery(
                true,
                &serde_json::json!({"status":"success","value":{"outcome":"rate_limited","retryAfterMs":120000}})
            ),
            super::super::Delivery::Retry(120000)
        ));
    }

    #[test]
    fn enrolled_credential_can_deliver_database_failure_with_no_previous_queue_or_database() {
        use super::super::*;
        let custody = MemoryCustody::default();
        let installation = Secret::new("synthetic-installation-one".into());
        custody
            .write(SecretKind::InstallationCredential, &installation)
            .unwrap();
        let credential = Credential {
            touch_grass_id: "TG-AAAAAA".into(),
            generation: 1,
            reporter_id: Some("testreporter".into()),
            installation_fingerprint: installation_fingerprint(&installation),
            secret: Secret::new("a".repeat(64)),
        };
        credential.save(&custody).unwrap();
        // Same order as initialize: open the independent queue, bind from Keychain,
        // then collect the database startup error. No SQLite is created or opened.
        let mut queue = queue::Queue::open(
            None,
            AppContext {
                version: "0.0.34".into(),
                build: None,
                os_version: None,
                architecture: Architecture::Unknown,
            },
            1000,
        );
        let binding = read_from(&custody).unwrap().unwrap().binding().unwrap();
        assert!(!restore_binding(&mut queue, &custody).unwrap());
        assert!(queue.next(&binding, 1000).unwrap().is_none());
        queue
            .capture(
                Failure::Database {
                    code: DatabaseCode::DatabaseOpenFailed,
                    provider: None,
                    context: DatabaseContext {
                        stage: Some("open-database".into()),
                        observed_format: None,
                        expected_format: Some(7),
                        modules: vec![],
                        backup_state: BackupState::Unknown,
                    },
                },
                1000,
            )
            .unwrap();
        let report = queue.next(&binding, 1000).unwrap().unwrap();
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(serialized.contains("database_open_failed"));
        assert!(!serialized.contains(credential.secret.expose()));
        queue
            .finish(&report.report_id, Delivery::Accepted, 1000)
            .unwrap();
        assert!(queue.next(&binding, 1000).unwrap().is_none());
    }

    #[test]
    fn a_pending_recovery_prevents_old_reporter_delivery_before_sqlite_opens() {
        use super::super::*;
        let custody = MemoryCustody::default();
        let installation = Secret::new("synthetic-installation-one".into());
        custody
            .write(SecretKind::InstallationCredential, &installation)
            .unwrap();
        let credential = Credential {
            touch_grass_id: "TG-AAAAAA".into(),
            generation: 1,
            reporter_id: Some("oldreporter".into()),
            installation_fingerprint: installation_fingerprint(&installation),
            secret: Secret::new("a".repeat(64)),
        };
        credential.save(&custody).unwrap();
        let mut queue = queue::Queue::open(
            None,
            AppContext {
                version: "0.0.34".into(),
                build: None,
                os_version: None,
                architecture: Architecture::Unknown,
            },
            1000,
        );
        let old_binding = credential.binding().unwrap();
        assert!(!restore_binding(&mut queue, &custody).unwrap());
        queue
            .capture(
                Failure::Database {
                    code: DatabaseCode::DatabaseOpenFailed,
                    provider: None,
                    context: DatabaseContext {
                        stage: Some("open-database".into()),
                        observed_format: None,
                        expected_format: None,
                        modules: vec![],
                        backup_state: BackupState::Unknown,
                    },
                },
                1000,
            )
            .unwrap();
        custody
            .write(
                SecretKind::RecoveryPreparation,
                &Secret::new(r#"{"touchGrassId":"TG-BBBBBB"}"#.into()),
            )
            .unwrap();
        assert!(restore_binding(&mut queue, &custody).unwrap());
        assert!(queue.next(&old_binding, 1000).unwrap().is_none());
        custody.delete(SecretKind::RecoveryPreparation).unwrap();
        assert!(restore_binding(&mut queue, &custody).unwrap());
        assert!(queue.next(&old_binding, 1000).unwrap().is_none());
        // Only a fresh successful registration may clear the durable pause.
        queue.resume(old_binding.clone()).unwrap();
        assert!(!restore_binding(&mut queue, &custody).unwrap());
    }

    #[test]
    fn a_replaced_installation_rejects_old_reporter_even_if_the_queue_was_lost() {
        let custody = MemoryCustody::default();
        let old_installation = Secret::new("synthetic-installation-before-recovery".into());
        custody
            .write(SecretKind::InstallationCredential, &old_installation)
            .unwrap();
        let credential = Credential {
            touch_grass_id: "TG-AAAAAA".into(),
            generation: 1,
            reporter_id: Some("oldreporter".into()),
            installation_fingerprint: installation_fingerprint(&old_installation),
            secret: Secret::new("a".repeat(64)),
        };
        credential.save(&custody).unwrap();
        assert!(read_from(&custody).unwrap().is_some());
        custody
            .write(
                SecretKind::InstallationCredential,
                &Secret::new("synthetic-installation-after-recovery".into()),
            )
            .unwrap();
        assert!(read_from(&custody).unwrap().is_none());
        assert!(
            custody
                .read(SecretKind::DiagnosticReporter)
                .unwrap()
                .is_none()
        );
    }
}
