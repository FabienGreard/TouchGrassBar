//! Explicit, process-local consent for one fixed remote report operation.
use crate::{
    profile::{ActiveSyncCredentials, HttpProfileTransport, ProfileCoordinator},
    support::SupportReports,
};
use convex::Value;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionState {
    expires_at: Option<u64>,
}
struct Session {
    id: String,
    touch_grass_id: String,
    generation: u64,
    expires_at: u64,
    pending: Option<(String, Value)>,
}
pub(crate) struct RemoteSupport {
    session: Mutex<Option<Session>>,
    profile: Arc<Mutex<ProfileCoordinator>>,
    reports: SupportReports,
    version: String,
    enabled: bool,
}
fn now() -> u64 {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}
fn authority_args(credentials: &ActiveSyncCredentials) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "installationCredential".into(),
            Value::String(credentials.installation_credential.expose().to_owned()),
        ),
        (
            "activeMacGeneration".into(),
            Value::Float64(credentials.active_mac_generation as f64),
        ),
    ])
}
fn session_args(credentials: &ActiveSyncCredentials, id: &str) -> BTreeMap<String, Value> {
    let mut args = authority_args(credentials);
    args.insert("sessionId".into(), Value::String(id.to_owned()));
    args
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
impl Session {
    fn matches(&self, identity: &str, generation: u64, at: u64) -> bool {
        self.expires_at > at && self.touch_grass_id == identity && self.generation == generation
    }
}
impl RemoteSupport {
    pub(crate) fn new(
        profile: Arc<Mutex<ProfileCoordinator>>,
        reports: SupportReports,
        version: String,
        enabled: bool,
    ) -> Self {
        Self {
            session: Mutex::new(None),
            profile,
            reports,
            version,
            enabled,
        }
    }
    fn credentials(&self) -> Result<Option<(String, ActiveSyncCredentials)>, ()> {
        // A busy Profile coordinator is transient, not withdrawal of consent.
        Ok(self
            .profile
            .try_lock()
            .map_err(|_| ())?
            .cached_diagnostic_registration())
    }
    pub(crate) fn state(&self) -> Result<SessionState, ()> {
        let mut stored = self.session.lock().map_err(|_| ())?;
        if let Some(session) = stored.as_ref() {
            let current = self.credentials()?;
            if current.as_ref().is_none_or(|(identity, credentials)| {
                !session.matches(identity, credentials.active_mac_generation, now())
            }) {
                *stored = None;
            }
        }
        Ok(SessionState {
            expires_at: stored.as_ref().map(|session| session.expires_at),
        })
    }
    pub(crate) fn set_enabled(&self, enabled: bool) -> Result<SessionState, ()> {
        if !self.enabled {
            return Err(());
        }
        let mut stored = self.session.lock().map_err(|_| ())?;
        let transport = HttpProfileTransport::from_build_configuration();
        if !enabled {
            // Clear local consent even when cancellation cannot reach the server.
            if let Some(session) = stored.take() {
                if let Ok(Some((identity, credentials))) = self.credentials() {
                    if session.matches(&identity, credentials.active_mac_generation, now()) {
                        let _ = transport.mutate_profile(
                            &credentials.session,
                            "support:cancel",
                            session_args(&credentials, &session.id),
                            "support unavailable",
                        );
                    }
                }
            }
            return Ok(SessionState { expires_at: None });
        }
        let (identity, credentials) = self.credentials()?.ok_or(())?;
        let result = transport
            .mutate_profile(
                &credentials.session,
                "support:start",
                authority_args(&credentials),
                "support unavailable",
            )
            .map_err(|_| ())?;
        let Value::Object(fields) = result else {
            return Err(());
        };
        let (Some(Value::String(id)), Some(Value::Float64(expires_at))) =
            (fields.get("sessionId"), fields.get("expiresAt"))
        else {
            return Err(());
        };
        let at = now();
        if !valid_id(id)
            || !expires_at.is_finite()
            || expires_at.fract() != 0.0
            || *expires_at <= at as f64
            || *expires_at > at.saturating_add(30 * 60_000) as f64
        {
            return Err(());
        }
        *stored = Some(Session {
            id: id.clone(),
            touch_grass_id: identity,
            generation: credentials.active_mac_generation,
            expires_at: *expires_at as u64,
            pending: None,
        });
        Ok(SessionState {
            expires_at: Some(*expires_at as u64),
        })
    }
    pub(crate) fn pump(&self) {
        // Serializes read/submit with consent changes. This never holds the Profile lock over IO.
        let Ok(mut stored) = self.session.lock() else {
            return;
        };
        let Some(session) = stored.as_mut() else {
            return;
        };
        let Ok(current) = self.credentials() else {
            return;
        };
        let Some((identity, credentials)) = current else {
            *stored = None;
            return;
        };
        if !session.matches(&identity, credentials.active_mac_generation, now()) {
            *stored = None;
            return;
        }
        let transport = HttpProfileTransport::from_build_configuration();
        let response = transport.mutate_profile(
            &credentials.session,
            "support:poll",
            session_args(&credentials, &session.id),
            "support unavailable",
        );
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if error.is_authority_rejected() {
                    *stored = None;
                }
                return;
            }
        };
        let Value::Object(fields) = response else {
            return;
        };
        if !matches!(fields.get("active"), Some(Value::Boolean(true))) {
            *stored = None;
            return;
        }
        let Some(Value::String(request_id)) = fields.get("requestId") else {
            session.pending = None;
            return;
        };
        if !valid_id(request_id) {
            return;
        }
        if session
            .pending
            .as_ref()
            .is_none_or(|(id, _)| id != request_id)
        {
            let report = self
                .reports
                .read(&self.version, time::OffsetDateTime::now_utc())
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| Value::try_from(value).ok())
                .unwrap_or(Value::Null);
            session.pending = Some((request_id.clone(), report));
        }
        // Recheck recovery and expiry after the report read, before transmission.
        let Ok(current) = self.credentials() else {
            return;
        };
        let Some((identity, credentials)) = current else {
            *stored = None;
            return;
        };
        if !session.matches(&identity, credentials.active_mac_generation, now()) {
            *stored = None;
            return;
        }
        let mut args = session_args(&credentials, &session.id);
        let Some((request_id, report)) = &session.pending else {
            return;
        };
        args.insert("requestId".into(), Value::String(request_id.clone()));
        args.insert("report".into(), report.clone());
        if transport
            .mutate_profile(
                &credentials.session,
                "support:complete",
                args,
                "support unavailable",
            )
            .is_ok()
        {
            session.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consent_is_bound_to_identity_generation_and_expiry() {
        let session = Session {
            id: "testsession".into(),
            touch_grass_id: "TG-AAAAAA".into(),
            generation: 2,
            expires_at: 100,
            pending: None,
        };
        assert!(session.matches("TG-AAAAAA", 2, 99));
        assert!(!session.matches("TG-BBBBBB", 2, 99));
        assert!(!session.matches("TG-AAAAAA", 1, 99));
        assert!(!session.matches("TG-AAAAAA", 2, 100));
        assert!(!valid_id("/private/path"));
    }
}
