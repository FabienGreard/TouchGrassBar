#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use std::{
    collections::BTreeMap,
    fmt,
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use convex::{ConvexClient, FunctionResult, Value};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::lifecycle::{DesktopLifecycle, SettingsProfileAuthorization};
use crate::sanitized::SanitizedProfileOutcome;

const KEYCHAIN_SERVICE: &str = "app.touchgrass.bar.profile";
const ENSURE_PROFILE_MUTATION: &str = "tokenmaxxers:ensureProfile";
const UPDATE_DISPLAY_NAME_MUTATION: &str = "tokenmaxxers:updateDisplayName";
const PREPARE_PATH: &str = "/api/auth/touchgrass/prepare";
const SIGN_UP_PATH: &str = "/api/auth/sign-up/email";
const SIGN_IN_PATH: &str = "/api/auth/sign-in/username";
const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const SIGNUP_PROOF_HEADER: &str = "x-touchgrass-signup-proof";
const PROFILE_MUTATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROFILE_TOKEN_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_PROFILE_JWT_BYTES: usize = 8 * 1_024;
const AUTHORITY_REJECTED_MESSAGE: &str = "Active Mac authority rejected";
const ID_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const SECRET_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct KeychainConfiguration {
    service: &'static str,
}

fn keychain_configuration(development_service: Option<&'static str>) -> KeychainConfiguration {
    KeychainConfiguration {
        service: development_service
            .filter(|service| !service.is_empty())
            .unwrap_or(KEYCHAIN_SERVICE),
    }
}

#[cfg(target_os = "macos")]
fn build_keychain_configuration() -> KeychainConfiguration {
    keychain_configuration(option_env!("TOUCHGRASS_DEV_KEYCHAIN_SERVICE"))
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum SecretKind {
    RecoveryKey,
    BetterAuthSession,
    InstallationCredential,
    SignupPreparation,
    ConvexJwt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Accessibility {
    WhenUnlockedThisDeviceOnly,
    AfterFirstUnlockThisDeviceOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KeychainPolicy {
    pub account: &'static str,
    pub synchronized: bool,
    pub accessibility: Accessibility,
}

pub(crate) const fn keychain_policy(kind: SecretKind) -> KeychainPolicy {
    match kind {
        SecretKind::RecoveryKey => KeychainPolicy {
            account: "recovery-key",
            synchronized: false,
            accessibility: Accessibility::WhenUnlockedThisDeviceOnly,
        },
        SecretKind::BetterAuthSession => KeychainPolicy {
            account: "better-auth-session",
            synchronized: false,
            accessibility: Accessibility::AfterFirstUnlockThisDeviceOnly,
        },
        SecretKind::InstallationCredential => KeychainPolicy {
            account: "installation-credential",
            synchronized: false,
            accessibility: Accessibility::AfterFirstUnlockThisDeviceOnly,
        },
        SecretKind::SignupPreparation => KeychainPolicy {
            account: "signup-preparation",
            synchronized: false,
            accessibility: Accessibility::AfterFirstUnlockThisDeviceOnly,
        },
        SecretKind::ConvexJwt => KeychainPolicy {
            account: "convex-jwt-memory-only",
            synchronized: false,
            accessibility: Accessibility::WhenUnlockedThisDeviceOnly,
        },
    }
}

pub(crate) struct Secret(Zeroizing<String>);

impl Secret {
    fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    #[cfg(test)]
    pub(crate) fn test_only() -> Self {
        Self::new("test-only".to_owned())
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for Secret {
    fn clone(&self) -> Self {
        Self::new(self.expose().to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

fn recovery_key_suffix(key: &Secret) -> String {
    key.expose()
        .chars()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileError(&'static str);

impl ProfileError {
    fn authority_rejected() -> Self {
        Self(AUTHORITY_REJECTED_MESSAGE)
    }

    pub(crate) fn is_authority_rejected(self) -> bool {
        self.0 == AUTHORITY_REJECTED_MESSAGE
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

pub(crate) trait SecretCustody: Send + Sync {
    fn delete(&self, kind: SecretKind) -> Result<(), ProfileError>;
    fn read(&self, kind: SecretKind) -> Result<Option<Secret>, ProfileError>;
    fn write(&self, kind: SecretKind, value: &Secret) -> Result<(), ProfileError>;
}

#[cfg(target_os = "macos")]
pub(crate) struct MacKeychain;

#[cfg(target_os = "macos")]
impl MacKeychain {
    fn options(
        kind: SecretKind,
    ) -> Result<security_framework::passwords::PasswordOptions, ProfileError> {
        use security_framework::passwords::PasswordOptions;

        if kind == SecretKind::ConvexJwt {
            return Err(ProfileError("memory-only credential cannot be stored"));
        }
        let configuration = build_keychain_configuration();
        let policy = keychain_policy(kind);
        let mut options =
            PasswordOptions::new_generic_password(configuration.service, policy.account);
        options.set_access_synchronized(Some(policy.synchronized));
        options.use_protected_keychain();
        Ok(options)
    }

    fn write_options(
        kind: SecretKind,
    ) -> Result<security_framework::passwords::PasswordOptions, ProfileError> {
        use security_framework::{
            access_control::{ProtectionMode, SecAccessControl},
            passwords::AccessControlOptions,
        };

        let policy = keychain_policy(kind);
        let protection = match policy.accessibility {
            Accessibility::WhenUnlockedThisDeviceOnly => {
                ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly
            }
            Accessibility::AfterFirstUnlockThisDeviceOnly => {
                ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly
            }
        };
        let access_control = SecAccessControl::create_with_protection(
            Some(protection),
            AccessControlOptions::empty().bits(),
        )
        .map_err(|_| ProfileError("secure custody unavailable"))?;
        let mut options = Self::options(kind)?;
        options.set_access_control(access_control);
        Ok(options)
    }
}

#[cfg(target_os = "macos")]
impl SecretCustody for MacKeychain {
    fn delete(&self, kind: SecretKind) -> Result<(), ProfileError> {
        use security_framework::passwords::delete_generic_password_options;
        use security_framework_sys::base::errSecItemNotFound;

        match delete_generic_password_options(Self::options(kind)?) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(_) => Err(ProfileError("secure custody unavailable")),
        }
    }

    fn read(&self, kind: SecretKind) -> Result<Option<Secret>, ProfileError> {
        use security_framework::passwords::generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        match generic_password(Self::options(kind)?) {
            Ok(value) => String::from_utf8(value)
                .map(Secret::new)
                .map(Some)
                .map_err(|_| ProfileError("secure custody unavailable")),
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(_) => Err(ProfileError("secure custody unavailable")),
        }
    }

    fn write(&self, kind: SecretKind, value: &Secret) -> Result<(), ProfileError> {
        security_framework::passwords::set_generic_password_options(
            value.expose().as_bytes(),
            Self::write_options(kind)?,
        )
        .map_err(|_| ProfileError("secure custody unavailable"))
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn production_recovery_key_suffix(lifecycle: &DesktopLifecycle) -> Option<String> {
    lifecycle.ready_touch_grass_id()?;
    MacKeychain
        .read(SecretKind::RecoveryKey)
        .ok()
        .flatten()
        .map(|key| recovery_key_suffix(&key))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn production_recovery_key_suffix(_lifecycle: &DesktopLifecycle) -> Option<String> {
    None
}

#[derive(Clone, Debug)]
struct PreparedProfile {
    expires_at_ms: u64,
    touch_grass_id: String,
    signup_proof: Secret,
}

impl PreparedProfile {
    fn encode(&self) -> Secret {
        Secret::new(format!(
            "{}\n{}\n{}",
            self.touch_grass_id,
            self.expires_at_ms,
            self.signup_proof.expose()
        ))
    }

    fn decode(value: &Secret) -> Result<Self, ProfileError> {
        let mut fields = value.expose().splitn(3, '\n');
        let (Some(touch_grass_id), Some(expires_at), Some(signup_proof)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(ProfileError("profile preparation unavailable"));
        };
        let expires_at_ms = expires_at
            .parse::<u64>()
            .map_err(|_| ProfileError("profile preparation unavailable"))?;
        if !valid_touch_grass_id(touch_grass_id) || signup_proof.is_empty() {
            return Err(ProfileError("profile preparation unavailable"));
        }
        Ok(Self {
            expires_at_ms,
            touch_grass_id: touch_grass_id.to_owned(),
            signup_proof: Secret::new(signup_proof.to_owned()),
        })
    }
}

enum SignInOutcome {
    Authenticated(Secret),
    NoAccount,
}

trait ProfileTransport: Send + Sync {
    fn prepare(&self) -> Result<PreparedProfile, ProfileError>;
    fn sign_up(
        &self,
        prepared: &PreparedProfile,
        recovery_key: &Secret,
        display_name: &str,
    ) -> Result<(), ProfileError>;
    fn sign_in(
        &self,
        touch_grass_id: &str,
        recovery_key: &Secret,
    ) -> Result<SignInOutcome, ProfileError>;
    fn ensure_profile(
        &self,
        session: &Secret,
        display_name: &str,
        expected_touch_grass_id: &str,
        installation_credential: &Secret,
    ) -> Result<EnsuredProfileAuthority, ProfileError>;
    fn update_display_name(&self, session: &Secret, display_name: &str)
    -> Result<(), ProfileError>;
}

fn profile_mutation_payload(display_name: String) -> BTreeMap<String, Value> {
    BTreeMap::from([("displayName".to_owned(), Value::String(display_name))])
}

fn ensure_profile_mutation_payload(
    display_name: String,
    expected_touch_grass_id: String,
    installation_credential: &Secret,
) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("displayName".to_owned(), Value::String(display_name)),
        (
            "expectedTouchGrassId".to_owned(),
            Value::String(expected_touch_grass_id),
        ),
        (
            "installationCredential".to_owned(),
            Value::String(installation_credential.expose().to_owned()),
        ),
    ])
}

struct EnsuredProfileAuthority {
    active_mac_generation: u64,
    touch_grass_id: String,
}

fn ensured_profile_authority(value: &Value) -> Option<EnsuredProfileAuthority> {
    let Value::Object(object) = value else {
        return None;
    };
    let active_mac_generation = match object.get("activeMacGeneration") {
        Some(Value::Int64(value)) => u64::try_from(*value).ok().filter(|value| *value > 0),
        Some(Value::Float64(value))
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 1.0
                && *value <= 9_007_199_254_740_991.0 =>
        {
            Some(*value as u64)
        }
        _ => None,
    }?;
    let touch_grass_id = match object.get("touchGrassId") {
        Some(Value::String(value)) if valid_touch_grass_id(value) => value.clone(),
        _ => return None,
    };
    Some(EnsuredProfileAuthority {
        active_mac_generation,
        touch_grass_id,
    })
}

fn matching_active_mac_generation(
    authority: EnsuredProfileAuthority,
    expected_touch_grass_id: &str,
) -> Result<u64, ProfileError> {
    (authority.touch_grass_id == expected_touch_grass_id)
        .then_some(authority.active_mac_generation)
        .ok_or_else(ProfileError::authority_rejected)
}

pub(crate) struct ActiveSyncCredentials {
    pub(crate) active_mac_generation: u64,
    pub(crate) installation_credential: Secret,
    pub(crate) session: Secret,
}

pub(crate) struct ProfileCoordinator {
    active_mac_generation: Mutex<Option<u64>>,
    lifecycle: DesktopLifecycle,
    custody: Arc<dyn SecretCustody>,
    transport: Arc<dyn ProfileTransport>,
}

impl ProfileCoordinator {
    fn new(
        lifecycle: DesktopLifecycle,
        custody: Arc<dyn SecretCustody>,
        transport: Arc<dyn ProfileTransport>,
    ) -> Self {
        Self {
            active_mac_generation: Mutex::new(None),
            lifecycle,
            custody,
            transport,
        }
    }

    pub(crate) fn retry_pending(&self) -> Result<Option<SanitizedProfileOutcome>, ProfileError> {
        let Some(request) = self.lifecycle.profile_request() else {
            return Ok(None);
        };

        let installation_credential = self.ensure_secret(SecretKind::InstallationCredential, 52)?;
        let recovery_key = self.ensure_secret(SecretKind::RecoveryKey, 48)?;
        let mut prepared = match self.custody.read(SecretKind::SignupPreparation)? {
            Some(value) => PreparedProfile::decode(&value)?,
            None => {
                let prepared = self.transport.prepare()?;
                if !valid_touch_grass_id(&prepared.touch_grass_id) {
                    return Err(ProfileError("profile preparation unavailable"));
                }
                self.custody
                    .write(SecretKind::SignupPreparation, &prepared.encode())?;
                prepared
            }
        };

        let session = match self
            .transport
            .sign_in(&prepared.touch_grass_id, &recovery_key)?
        {
            SignInOutcome::Authenticated(session) => session,
            SignInOutcome::NoAccount => {
                if prepared.expires_at_ms <= unix_time_ms()? {
                    prepared = self.transport.prepare()?;
                    self.custody
                        .write(SecretKind::SignupPreparation, &prepared.encode())?;
                }
                self.transport
                    .sign_up(&prepared, &recovery_key, &request.display_name)?;
                match self
                    .transport
                    .sign_in(&prepared.touch_grass_id, &recovery_key)?
                {
                    SignInOutcome::Authenticated(session) => session,
                    SignInOutcome::NoAccount => {
                        return Err(ProfileError("Profile creation pending"));
                    }
                }
            }
        };
        self.custody
            .write(SecretKind::BetterAuthSession, &session)?;
        let authority = self.transport.ensure_profile(
            &session,
            &request.display_name,
            &prepared.touch_grass_id,
            &installation_credential,
        )?;
        let generation = match matching_active_mac_generation(authority, &prepared.touch_grass_id) {
            Ok(generation) => generation,
            Err(error) => {
                let _ = self.custody.delete(SecretKind::BetterAuthSession);
                return Err(error);
            }
        };
        *self
            .active_mac_generation
            .lock()
            .map_err(|_| ProfileError("Active Mac authority unavailable"))? = Some(generation);
        self.lifecycle
            .mark_profile_ready(&prepared.touch_grass_id)
            .map_err(ProfileError)?;
        let _ = self.custody.delete(SecretKind::SignupPreparation);
        let profile = SanitizedProfileOutcome::Ready {
            display_name: request.display_name,
            touch_grass_id: prepared.touch_grass_id,
        };
        Ok(Some(profile))
    }

    pub(crate) fn active_sync_credentials(
        &self,
    ) -> Result<Option<ActiveSyncCredentials>, ProfileError> {
        let SanitizedProfileOutcome::Ready {
            display_name,
            touch_grass_id,
        } = self.lifecycle.sanitized_profile_outcome()
        else {
            return Ok(None);
        };
        let installation_credential = self
            .custody
            .read(SecretKind::InstallationCredential)?
            .ok_or(ProfileError("Active Mac authority unavailable"))?;
        let mut session = match self.custody.read(SecretKind::BetterAuthSession)? {
            Some(session) => session,
            None => {
                let recovery_key = self
                    .custody
                    .read(SecretKind::RecoveryKey)?
                    .ok_or(ProfileError("Active Mac authority unavailable"))?;
                let SignInOutcome::Authenticated(session) =
                    self.transport.sign_in(&touch_grass_id, &recovery_key)?
                else {
                    return Err(ProfileError::authority_rejected());
                };
                self.custody
                    .write(SecretKind::BetterAuthSession, &session)?;
                session
            }
        };
        let cached_generation = *self
            .active_mac_generation
            .lock()
            .map_err(|_| ProfileError("Active Mac authority unavailable"))?;
        let active_mac_generation = if let Some(generation) = cached_generation {
            generation
        } else {
            let first_attempt = self
                .transport
                .ensure_profile(
                    &session,
                    &display_name,
                    &touch_grass_id,
                    &installation_credential,
                )
                .and_then(|authority| matching_active_mac_generation(authority, &touch_grass_id));
            let generation = match first_attempt {
                Ok(generation) => generation,
                Err(_) => {
                    let recovery_key = self
                        .custody
                        .read(SecretKind::RecoveryKey)?
                        .ok_or(ProfileError("Active Mac authority unavailable"))?;
                    let SignInOutcome::Authenticated(fresh_session) =
                        self.transport.sign_in(&touch_grass_id, &recovery_key)?
                    else {
                        return Err(ProfileError::authority_rejected());
                    };
                    self.custody
                        .write(SecretKind::BetterAuthSession, &fresh_session)?;
                    session = fresh_session;
                    let authority = self.transport.ensure_profile(
                        &session,
                        &display_name,
                        &touch_grass_id,
                        &installation_credential,
                    )?;
                    match matching_active_mac_generation(authority, &touch_grass_id) {
                        Ok(generation) => generation,
                        Err(error) => {
                            let _ = self.custody.delete(SecretKind::BetterAuthSession);
                            return Err(error);
                        }
                    }
                }
            };
            *self
                .active_mac_generation
                .lock()
                .map_err(|_| ProfileError("Active Mac authority unavailable"))? = Some(generation);
            generation
        };

        Ok(Some(ActiveSyncCredentials {
            active_mac_generation,
            installation_credential,
            session,
        }))
    }

    pub(crate) fn recovery_key(
        &self,
        authorization: SettingsProfileAuthorization,
    ) -> Result<Secret, ProfileError> {
        if !self.lifecycle.is_current_profile_settings(authorization) {
            return Err(ProfileError("Recovery Key unavailable"));
        }
        self.lifecycle
            .ready_touch_grass_id()
            .ok_or(ProfileError("Recovery Key unavailable"))?;
        self.custody
            .read(SecretKind::RecoveryKey)?
            .ok_or(ProfileError("Recovery Key unavailable"))
    }

    pub(crate) fn update_display_name(
        &self,
        authorization: SettingsProfileAuthorization,
        display_name: &str,
    ) -> Result<SanitizedProfileOutcome, ProfileError> {
        if !self.lifecycle.is_current_profile_settings(authorization) {
            return Err(ProfileError("Display Name update unavailable"));
        }
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 40 {
            return Err(ProfileError("Display Name invalid"));
        }
        let touch_grass_id = self
            .lifecycle
            .ready_touch_grass_id()
            .ok_or(ProfileError("Display Name update unavailable"))?;
        let recovery_key = self
            .custody
            .read(SecretKind::RecoveryKey)?
            .ok_or(ProfileError("Display Name update unavailable"))?;
        let SignInOutcome::Authenticated(session) =
            self.transport.sign_in(&touch_grass_id, &recovery_key)?
        else {
            return Err(ProfileError("Display Name update unavailable"));
        };
        self.custody
            .write(SecretKind::BetterAuthSession, &session)?;
        self.transport.update_display_name(&session, display_name)?;
        self.lifecycle
            .update_display_name(display_name)
            .map_err(ProfileError)?;
        Ok(SanitizedProfileOutcome::Ready {
            display_name: display_name.to_owned(),
            touch_grass_id,
        })
    }

    fn ensure_secret(&self, kind: SecretKind, length: usize) -> Result<Secret, ProfileError> {
        if let Some(value) = self.custody.read(kind)? {
            return Ok(value);
        }
        let value = Secret::new(generate_secret(length)?);
        self.custody.write(kind, &value)?;
        Ok(value)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn production_coordinator(lifecycle: DesktopLifecycle) -> ProfileCoordinator {
    ProfileCoordinator::new(
        lifecycle,
        Arc::new(MacKeychain),
        Arc::new(HttpProfileTransport::from_build_configuration()),
    )
}

fn generate_secret(length: usize) -> Result<String, ProfileError> {
    let mut random = vec![0_u8; length];
    getrandom::fill(&mut random).map_err(|_| ProfileError("secure random unavailable"))?;
    Ok(random
        .into_iter()
        .map(|byte| SECRET_ALPHABET[usize::from(byte) % SECRET_ALPHABET.len()] as char)
        .collect())
}

fn unix_time_ms() -> Result<u64, ProfileError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|_| ProfileError("system clock unavailable"))
}

fn valid_touch_grass_id(value: &str) -> bool {
    value.strip_prefix("TG-").is_some_and(|suffix| {
        suffix.len() == 6
            && suffix
                .bytes()
                .all(|character| ID_ALPHABET.contains(&character))
    })
}

#[derive(Clone)]
pub(crate) struct HttpProfileTransport {
    auth_site_url: Option<&'static str>,
    convex_url: Option<&'static str>,
    client: reqwest::blocking::Client,
}

impl HttpProfileTransport {
    #[cfg(target_os = "macos")]
    pub(crate) fn from_build_configuration() -> Self {
        Self {
            auth_site_url: option_env!("CONVEX_SITE_URL").filter(|value| !value.is_empty()),
            convex_url: option_env!("CONVEX_URL").filter(|value| !value.is_empty()),
            client: crate::native_https_client(),
        }
    }

    fn endpoint(&self, path: &str) -> Result<String, ProfileError> {
        let base = self
            .auth_site_url
            .ok_or(ProfileError("profile service unavailable"))?;
        Ok(format!("{}{path}", base.trim_end_matches('/')))
    }

    fn fetch_convex_token(
        &self,
        session: &Secret,
        failure: &'static str,
    ) -> Result<Zeroizing<String>, ProfileError> {
        let response = self
            .client
            .get(self.endpoint(CONVEX_TOKEN_PATH)?)
            .bearer_auth(session.expose())
            .send()
            .map_err(|_| ProfileError(failure))?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(ProfileError::authority_rejected());
        }
        let response = response
            .error_for_status()
            .map_err(|_| ProfileError(failure))?;
        let mut body = Zeroizing::new(Vec::with_capacity(MAX_PROFILE_TOKEN_RESPONSE_BYTES));
        response
            .take((MAX_PROFILE_TOKEN_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|_| ProfileError(failure))?;
        if body.len() > MAX_PROFILE_TOKEN_RESPONSE_BYTES {
            return Err(ProfileError(failure));
        }
        let response: ConvexTokenResponse =
            serde_json::from_slice(body.as_slice()).map_err(|_| ProfileError(failure))?;
        if response.token.is_empty() || response.token.len() > MAX_PROFILE_JWT_BYTES {
            return Err(ProfileError(failure));
        }
        Ok(Zeroizing::new(response.token))
    }

    fn mutate_profile(
        &self,
        session: &Secret,
        mutation: &'static str,
        payload: BTreeMap<String, Value>,
        failure: &'static str,
    ) -> Result<Value, ProfileError> {
        let convex_url = self
            .convex_url
            .ok_or(ProfileError("profile service unavailable"))?
            .to_owned();
        let jwt = self.fetch_convex_token(session, failure)?;
        let attempt = tokio::runtime::Runtime::new()
            .map_err(|_| ProfileError(failure))?
            .block_on(async move {
                tokio::time::timeout(PROFILE_MUTATION_TIMEOUT, async move {
                    let mut client = ConvexClient::new(&convex_url)
                        .await
                        .map_err(|_| ProfileError(failure))?;
                    client.set_auth(Some(jwt.as_str().to_owned())).await;
                    let result = client.mutation(mutation, payload).await;
                    client.set_auth(None).await;
                    match result.map_err(|_| ProfileError(failure))? {
                        FunctionResult::Value(value) => Ok(value),
                        FunctionResult::ConvexError(error)
                            if is_exact_authority_rejection(&error.data) =>
                        {
                            Err(ProfileError::authority_rejected())
                        }
                        FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
                            Err(ProfileError(failure))
                        }
                    }
                })
                .await
            });
        match attempt {
            Ok(result) => result,
            Err(_) => Err(ProfileError(failure)),
        }
    }
}

fn is_exact_authority_rejection(data: &Value) -> bool {
    let Value::Object(fields) = data else {
        return false;
    };
    fields.len() == 1
        && matches!(
            fields.get("code"),
            Some(Value::String(code)) if code == "authority-rejected"
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareResponse {
    expires_at: u64,
    touch_grass_id: String,
    signup_proof: String,
}

#[derive(Deserialize)]
struct SignInResponse {
    token: String,
}

#[derive(Deserialize)]
struct ConvexTokenResponse {
    token: String,
}

impl ProfileTransport for HttpProfileTransport {
    fn prepare(&self) -> Result<PreparedProfile, ProfileError> {
        let response = self
            .client
            .post(self.endpoint(PREPARE_PATH)?)
            .json(&serde_json::json!({}))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|_| ProfileError("Profile creation pending"))?
            .json::<PrepareResponse>()
            .map_err(|_| ProfileError("Profile creation pending"))?;
        Ok(PreparedProfile {
            expires_at_ms: response.expires_at,
            touch_grass_id: response.touch_grass_id,
            signup_proof: Secret::new(response.signup_proof),
        })
    }

    fn sign_up(
        &self,
        prepared: &PreparedProfile,
        recovery_key: &Secret,
        display_name: &str,
    ) -> Result<(), ProfileError> {
        let email = format!(
            "{}@profile.touchgrass.invalid",
            prepared.touch_grass_id.to_lowercase()
        );
        self.client
            .post(self.endpoint(SIGN_UP_PATH)?)
            .header(SIGNUP_PROOF_HEADER, prepared.signup_proof.expose())
            .json(&serde_json::json!({
                "email": email,
                "name": display_name,
                "password": recovery_key.expose(),
                "username": prepared.touch_grass_id,
            }))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|_| ProfileError("Profile creation pending"))?;
        Ok(())
    }

    fn sign_in(
        &self,
        touch_grass_id: &str,
        recovery_key: &Secret,
    ) -> Result<SignInOutcome, ProfileError> {
        let response = self
            .client
            .post(self.endpoint(SIGN_IN_PATH)?)
            .json(&serde_json::json!({
                "password": recovery_key.expose(),
                "username": touch_grass_id,
            }))
            .send()
            .map_err(|_| ProfileError("Profile creation pending"))?;
        if matches!(response.status().as_u16(), 400 | 401 | 403 | 404) {
            return Ok(SignInOutcome::NoAccount);
        }
        let response = response
            .error_for_status()
            .map_err(|_| ProfileError("Profile creation pending"))?
            .json::<SignInResponse>()
            .map_err(|_| ProfileError("Profile creation pending"))?;
        Ok(SignInOutcome::Authenticated(Secret::new(response.token)))
    }

    fn ensure_profile(
        &self,
        session: &Secret,
        display_name: &str,
        expected_touch_grass_id: &str,
        installation_credential: &Secret,
    ) -> Result<EnsuredProfileAuthority, ProfileError> {
        let result = self.mutate_profile(
            session,
            ENSURE_PROFILE_MUTATION,
            ensure_profile_mutation_payload(
                display_name.to_owned(),
                expected_touch_grass_id.to_owned(),
                installation_credential,
            ),
            "Profile creation pending",
        )?;
        ensured_profile_authority(&result).ok_or(ProfileError("Profile creation pending"))
    }

    fn update_display_name(
        &self,
        session: &Secret,
        display_name: &str,
    ) -> Result<(), ProfileError> {
        self.mutate_profile(
            session,
            UPDATE_DISPLAY_NAME_MUTATION,
            profile_mutation_payload(display_name.to_owned()),
            "Display Name update unavailable",
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        process,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        lifecycle::{LaunchAtLoginState, ProfileProvisioningStatus, ProviderPresenceStatus},
        sanitized::NativeCore,
    };

    use super::*;

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

    #[derive(Default)]
    struct FakeCustody(Mutex<BTreeMap<SecretKind, Secret>>);

    impl FakeCustody {
        fn contains(&self, kind: SecretKind) -> bool {
            self.0.lock().unwrap().contains_key(&kind)
        }

        fn private_values(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .values()
                .map(|value| value.expose().to_owned())
                .collect()
        }
    }

    impl SecretCustody for FakeCustody {
        fn delete(&self, kind: SecretKind) -> Result<(), ProfileError> {
            self.0.lock().unwrap().remove(&kind);
            Ok(())
        }

        fn read(&self, kind: SecretKind) -> Result<Option<Secret>, ProfileError> {
            Ok(self.0.lock().unwrap().get(&kind).cloned())
        }

        fn write(&self, kind: SecretKind, value: &Secret) -> Result<(), ProfileError> {
            if kind == SecretKind::ConvexJwt {
                return Err(ProfileError("memory-only credential cannot be stored"));
            }
            self.0.lock().unwrap().insert(kind, value.clone());
            Ok(())
        }
    }

    struct FakeTransport {
        touch_grass_id: String,
        signup_proof: Secret,
        account_exists: AtomicBool,
        fail_next: AtomicBool,
        exchange_count: AtomicUsize,
        fixed_profile_mutation: AtomicBool,
        last_jwt: Mutex<Option<String>>,
        authority_touch_grass_id: Mutex<Option<String>>,
        private_sentinels: [Secret; 3],
    }

    impl FakeTransport {
        fn new() -> Self {
            let suffix = generate_secret(6)
                .unwrap()
                .chars()
                .map(|character| {
                    let index = character as usize % ID_ALPHABET.len();
                    ID_ALPHABET[index] as char
                })
                .collect::<String>();
            Self {
                touch_grass_id: format!("TG-{suffix}"),
                signup_proof: Secret::new(generate_secret(36).unwrap()),
                account_exists: AtomicBool::new(false),
                fail_next: AtomicBool::new(false),
                exchange_count: AtomicUsize::new(0),
                fixed_profile_mutation: AtomicBool::new(false),
                last_jwt: Mutex::new(None),
                authority_touch_grass_id: Mutex::new(None),
                private_sentinels: [
                    Secret::new(format!("COOKIE_SENTINEL_{}", generate_secret(18).unwrap())),
                    Secret::new(format!(
                        "PRIVATE_PATH_SENTINEL_{}",
                        generate_secret(18).unwrap()
                    )),
                    Secret::new(format!(
                        "RAW_RESPONSE_SENTINEL_{}",
                        generate_secret(18).unwrap()
                    )),
                ],
            }
        }

        fn touch_grass_id(&self) -> &str {
            &self.touch_grass_id
        }

        fn fail_next_attempt(&self) {
            self.fail_next.store(true, Ordering::SeqCst);
        }

        fn exchange_count(&self) -> usize {
            self.exchange_count.load(Ordering::SeqCst)
        }

        fn used_fixed_profile_mutation(&self) -> bool {
            self.fixed_profile_mutation.load(Ordering::SeqCst)
        }

        fn return_authority_for(&self, touch_grass_id: &str) {
            *self.authority_touch_grass_id.lock().unwrap() = Some(touch_grass_id.to_owned());
        }

        fn private_values(&self) -> Vec<String> {
            self.private_sentinels
                .iter()
                .map(|value| value.expose().to_owned())
                .collect()
        }
    }

    impl ProfileTransport for FakeTransport {
        fn prepare(&self) -> Result<PreparedProfile, ProfileError> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(ProfileError("Profile creation pending"));
            }
            Ok(PreparedProfile {
                expires_at_ms: u64::MAX,
                touch_grass_id: self.touch_grass_id.clone(),
                signup_proof: self.signup_proof.clone(),
            })
        }

        fn sign_up(
            &self,
            _prepared: &PreparedProfile,
            _recovery_key: &Secret,
            _display_name: &str,
        ) -> Result<(), ProfileError> {
            self.account_exists.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn sign_in(
            &self,
            _touch_grass_id: &str,
            _recovery_key: &Secret,
        ) -> Result<SignInOutcome, ProfileError> {
            Ok(if self.account_exists.load(Ordering::SeqCst) {
                SignInOutcome::Authenticated(Secret::new(generate_secret(42)?))
            } else {
                SignInOutcome::NoAccount
            })
        }

        fn ensure_profile(
            &self,
            _session: &Secret,
            _display_name: &str,
            expected_touch_grass_id: &str,
            installation_credential: &Secret,
        ) -> Result<EnsuredProfileAuthority, ProfileError> {
            if installation_credential.expose().len() != 52 {
                return Err(ProfileError("Active Mac authority unavailable"));
            }
            self.exchange_count.fetch_add(1, Ordering::SeqCst);
            let jwt = Secret::new(generate_secret(44)?);
            *self.last_jwt.lock().unwrap() = Some(jwt.expose().to_owned());
            self.fixed_profile_mutation.store(true, Ordering::SeqCst);
            drop(jwt);
            let touch_grass_id = self
                .authority_touch_grass_id
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| self.touch_grass_id.clone());
            if touch_grass_id != expected_touch_grass_id {
                return Err(ProfileError::authority_rejected());
            }
            Ok(EnsuredProfileAuthority {
                active_mac_generation: 1,
                touch_grass_id,
            })
        }

        fn update_display_name(
            &self,
            _session: &Secret,
            _display_name: &str,
        ) -> Result<(), ProfileError> {
            self.exchange_count.fetch_add(1, Ordering::SeqCst);
            let jwt = Secret::new(generate_secret(44)?);
            *self.last_jwt.lock().unwrap() = Some(jwt.expose().to_owned());
            drop(jwt);
            Ok(())
        }
    }

    struct TestDatabase(PathBuf);

    impl TestDatabase {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "touchgrassbar-profile-{}-{timestamp}-{}.sqlite3",
                process::id(),
                NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(self.0.with_extension("sqlite3-shm"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3-wal"));
        }
    }

    struct ProfileFixture {
        _database: TestDatabase,
        lifecycle: DesktopLifecycle,
        custody: Arc<FakeCustody>,
        transport: Arc<FakeTransport>,
        coordinator: ProfileCoordinator,
    }

    impl ProfileFixture {
        fn new() -> Self {
            let database = TestDatabase::new();
            let lifecycle = DesktopLifecycle::open(&database.0).unwrap();
            let custody = Arc::new(FakeCustody::default());
            let transport = Arc::new(FakeTransport::new());
            let coordinator =
                ProfileCoordinator::new(lifecycle.clone(), custody.clone(), transport.clone());
            Self {
                _database: database,
                lifecycle,
                custody,
                transport,
                coordinator,
            }
        }

        fn complete_bootstrap(&self) {
            self.lifecycle.complete_bootstrap("Fabien").unwrap();
        }

        fn public_boundaries(&self) -> Vec<String> {
            let core = NativeCore::no_io_unavailable();
            core.set_profile_outcome(self.lifecycle.sanitized_profile_outcome())
                .unwrap();
            vec![
                serde_json::to_string(&core.panel_state().unwrap()).unwrap(),
                serde_json::to_string(&self.lifecycle.bootstrap_state()).unwrap(),
                serde_json::to_string(
                    &self
                        .lifecycle
                        .settings_state(LaunchAtLoginState::Unavailable),
                )
                .unwrap(),
                serde_json::Value::from(Value::Object(profile_mutation_payload(
                    "Fabien".to_owned(),
                )))
                .to_string(),
                crate::profile_attempt_metric(&Result::<(), ProfileError>::Err(ProfileError(
                    "cookie credential private path raw response",
                )))
                .to_owned(),
            ]
        }

        fn assert_public_boundary_is_sanitized(&self, boundary: &str) {
            let normalized = boundary.to_lowercase();
            for prohibited in [
                "\"recoverykey\":",
                "password",
                "cookie",
                "credential",
                "session",
                "signup_proof",
                "rawresponse",
                "privatepath",
            ] {
                assert!(!normalized.contains(prohibited), "public {prohibited}");
            }
            let mut private_values = self.custody.private_values();
            private_values.push(self.transport.signup_proof.expose().to_owned());
            private_values.extend(self.transport.private_values());
            if let Some(jwt) = self.transport.last_jwt.lock().unwrap().clone() {
                private_values.push(jwt);
            }
            for value in private_values {
                assert!(!boundary.contains(&value), "public secret value");
            }
        }
    }

    #[test]
    fn recovery_key_suffix_is_limited_to_the_real_final_characters() {
        let key = Secret::new("not-a-secret".to_owned());

        let suffix = recovery_key_suffix(&key);

        assert_eq!(suffix, "ret");
        assert_eq!(suffix.chars().count(), 3);
        assert!(key.expose().ends_with(&suffix));
    }

    #[test]
    fn ensure_profile_payload_proves_the_expected_profile() {
        let installation_credential = Secret::new("A".repeat(52));

        let payload = ensure_profile_mutation_payload(
            "Fabien".to_owned(),
            "TG-234567".to_owned(),
            &installation_credential,
        );

        assert_eq!(
            payload.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "displayName",
                "expectedTouchGrassId",
                "installationCredential"
            ]
        );
        assert_eq!(
            payload.get("expectedTouchGrassId"),
            Some(&Value::String("TG-234567".to_owned()))
        );
    }

    #[test]
    fn prepare_sends_json_to_the_better_auth_http_route() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let request_length = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..request_length]);
            let sends_json = request
                .to_ascii_lowercase()
                .contains("content-type: application/json");
            let (status, body) = if sends_json {
                (
                    "200 OK",
                    r#"{"expiresAt":4102444800000,"touchGrassId":"TG-234567","signupProof":"proof"}"#,
                )
            } else {
                ("415 Unsupported Media Type", r#"{"error":"json required"}"#)
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let auth_site_url = Box::leak(format!("http://{address}").into_boxed_str());
        let transport = HttpProfileTransport {
            auth_site_url: Some(auth_site_url),
            convex_url: None,
            client: crate::native_https_client(),
        };

        let prepared = transport.prepare().expect("prepare JSON request");
        server.join().unwrap();

        assert_eq!(prepared.touch_grass_id, "TG-234567");
    }

    #[test]
    fn rejected_live_session_is_a_typed_authority_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let auth_site_url = Box::leak(format!("http://{address}").into_boxed_str());
        let transport = HttpProfileTransport {
            auth_site_url: Some(auth_site_url),
            convex_url: Some("http://127.0.0.1:1"),
            client: crate::native_https_client(),
        };

        let result =
            transport.fetch_convex_token(&Secret::new("rejected-session".to_owned()), "pending");
        server.join().unwrap();
        let Err(error) = result else {
            panic!("a rejected session must not return a token");
        };

        assert!(error.is_authority_rejected());
    }

    #[test]
    fn only_exact_structured_profile_rejection_is_authoritative() {
        let exact = Value::Object(BTreeMap::from([(
            "code".to_owned(),
            Value::String("authority-rejected".to_owned()),
        )]));
        let with_detail = Value::Object(BTreeMap::from([
            (
                "code".to_owned(),
                Value::String("authority-rejected".to_owned()),
            ),
            (
                "detail".to_owned(),
                Value::String("private-response".to_owned()),
            ),
        ]));

        assert!(is_exact_authority_rejection(&exact));
        assert!(!is_exact_authority_rejection(&with_detail));
        assert!(!is_exact_authority_rejection(&Value::String(
            "authority-rejected".to_owned()
        )));
    }

    #[test]
    fn custody_keeps_profile_secrets_in_separate_non_sync_items() {
        let policies = [
            keychain_policy(SecretKind::RecoveryKey),
            keychain_policy(SecretKind::BetterAuthSession),
            keychain_policy(SecretKind::InstallationCredential),
        ];

        assert_eq!(policies.map(|policy| policy.synchronized), [false; 3]);
        assert_ne!(policies[0].account, policies[1].account);
        assert_ne!(policies[1].account, policies[2].account);
        assert_eq!(
            policies[0].accessibility,
            Accessibility::WhenUnlockedThisDeviceOnly
        );
        assert_eq!(
            policies[1].accessibility,
            Accessibility::AfterFirstUnlockThisDeviceOnly
        );
    }

    #[test]
    fn development_custody_uses_one_isolated_data_protection_service() {
        let development = keychain_configuration(Some("app.touchgrass.bar.dev.wexample"));
        let production = keychain_configuration(None);

        assert_eq!(development.service, "app.touchgrass.bar.dev.wexample");
        assert_eq!(production.service, KEYCHAIN_SERVICE);
    }

    #[test]
    fn creation_does_not_disclose_and_settings_reveal_is_authorized() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        let outcome = fixture.coordinator.retry_pending().unwrap();
        assert!(matches!(
            outcome,
            Some(SanitizedProfileOutcome::Ready { .. })
        ));

        let state = fixture
            .lifecycle
            .settings_state(LaunchAtLoginState::Unavailable);
        let serialized = serde_json::to_string(&state).unwrap();
        assert_eq!(
            state.touch_grass_id.as_deref(),
            Some(fixture.transport.touch_grass_id())
        );

        fixture
            .lifecycle
            .request_settings_section(crate::lifecycle::SettingsSection::Profile);
        let authorization = fixture
            .lifecycle
            .authorize_profile_settings()
            .expect("Profile Settings authorization");

        let revealed = fixture.coordinator.recovery_key(authorization).unwrap();
        assert!(!revealed.expose().is_empty());
        assert!(!serialized.contains(revealed.expose()));

        fixture
            .lifecycle
            .request_settings_section(crate::lifecycle::SettingsSection::General);
        assert!(fixture.coordinator.recovery_key(authorization).is_err());
    }

    #[test]
    fn session_exchange_keeps_the_convex_jwt_in_memory() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();

        assert_eq!(fixture.transport.exchange_count(), 1);
        assert!(!fixture.custody.contains(SecretKind::ConvexJwt));
        assert!(fixture.transport.used_fixed_profile_mutation());
    }

    #[test]
    fn ready_profile_exposes_only_current_in_memory_sync_authority() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();

        let first = fixture
            .coordinator
            .active_sync_credentials()
            .unwrap()
            .unwrap();
        let second = fixture
            .coordinator
            .active_sync_credentials()
            .unwrap()
            .unwrap();

        assert_eq!(first.active_mac_generation, 1);
        assert_eq!(second.active_mac_generation, 1);
        assert_eq!(first.installation_credential.expose().len(), 52);
        assert!(!first.session.expose().is_empty());
        assert_eq!(fixture.transport.exchange_count(), 1);
        assert!(!fixture.custody.contains(SecretKind::ConvexJwt));
    }

    #[test]
    fn mismatched_authenticated_profile_never_caches_active_mac_authority() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        let wrong_touch_grass_id = if fixture.transport.touch_grass_id() == "TG-234567" {
            "TG-234568"
        } else {
            "TG-234567"
        };
        fixture.transport.return_authority_for(wrong_touch_grass_id);

        let error = fixture.coordinator.retry_pending().unwrap_err();
        assert!(error.is_authority_rejected());
        assert_eq!(
            fixture.lifecycle.bootstrap_state().profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
        assert!(
            fixture
                .coordinator
                .active_sync_credentials()
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .coordinator
                .active_mac_generation
                .lock()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn restart_propagates_rejected_active_mac_authority() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();
        let restarted = ProfileCoordinator::new(
            fixture.lifecycle.clone(),
            fixture.custody.clone(),
            fixture.transport.clone(),
        );
        let wrong_touch_grass_id = if fixture.transport.touch_grass_id() == "TG-234567" {
            "TG-234568"
        } else {
            "TG-234567"
        };
        fixture.transport.return_authority_for(wrong_touch_grass_id);

        let Err(error) = restarted.active_sync_credentials() else {
            panic!("rejected Active Mac authority must fail");
        };

        assert!(error.is_authority_rejected());
        assert!(restarted.active_mac_generation.lock().unwrap().is_none());
    }

    #[test]
    fn ready_profile_treats_rejected_recovery_sign_in_as_authority_rejection() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();
        fixture
            .custody
            .delete(SecretKind::BetterAuthSession)
            .unwrap();
        fixture
            .transport
            .account_exists
            .store(false, Ordering::SeqCst);
        let restarted = ProfileCoordinator::new(
            fixture.lifecycle.clone(),
            fixture.custody.clone(),
            fixture.transport.clone(),
        );

        let Err(error) = restarted.active_sync_credentials() else {
            panic!("rejected recovery sign-in must fail");
        };

        assert!(error.is_authority_rejected());
        assert!(restarted.active_mac_generation.lock().unwrap().is_none());
    }

    #[test]
    fn display_name_update_requires_profile_settings_and_commits_after_transport() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();
        fixture
            .lifecycle
            .request_settings_section(crate::lifecycle::SettingsSection::Profile);
        let authorization = fixture
            .lifecycle
            .authorize_profile_settings()
            .expect("Profile Settings authorization");

        let outcome = fixture
            .coordinator
            .update_display_name(authorization, "  New name  ")
            .unwrap();

        assert_eq!(
            outcome,
            SanitizedProfileOutcome::Ready {
                display_name: "New name".to_owned(),
                touch_grass_id: fixture.transport.touch_grass_id().to_owned(),
            }
        );
        assert_eq!(
            fixture.lifecycle.bootstrap_state().display_name.as_deref(),
            Some("New name")
        );
        assert_eq!(fixture.transport.exchange_count(), 2);

        fixture
            .lifecycle
            .request_settings_section(crate::lifecycle::SettingsSection::General);
        assert!(
            fixture
                .coordinator
                .update_display_name(authorization, "Other name")
                .is_err()
        );
        assert_eq!(
            fixture.lifecycle.bootstrap_state().display_name.as_deref(),
            Some("New name")
        );
    }

    #[test]
    fn retry_keeps_profile_pending_without_blocking_provider_utility() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        let providers_before = fixture.lifecycle.bootstrap_state().providers;
        fixture.transport.fail_next_attempt();

        assert!(fixture.coordinator.retry_pending().is_err());
        let pending = fixture.lifecycle.bootstrap_state();
        assert_eq!(
            pending.profile_provisioning,
            ProfileProvisioningStatus::ProfilePending
        );
        assert_eq!(
            pending
                .providers
                .iter()
                .map(|provider| provider.status)
                .collect::<Vec<_>>(),
            providers_before
                .iter()
                .map(|provider| provider.status)
                .collect::<Vec<_>>()
        );
        assert!(pending.providers.iter().all(|provider| matches!(
            provider.status,
            ProviderPresenceStatus::Detected
                | ProviderPresenceStatus::NotDetected
                | ProviderPresenceStatus::Unavailable
        )));

        fixture.coordinator.retry_pending().unwrap();
        assert_eq!(
            fixture.lifecycle.bootstrap_state().profile_provisioning,
            ProfileProvisioningStatus::Ready
        );
    }

    #[test]
    fn secret_sentinel_rejects_private_material_from_public_boundaries() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.coordinator.retry_pending().unwrap();

        for boundary in fixture.public_boundaries() {
            fixture.assert_public_boundary_is_sanitized(&boundary);
        }
    }
}
