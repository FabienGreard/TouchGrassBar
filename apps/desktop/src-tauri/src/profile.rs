use std::{
    collections::BTreeMap,
    fmt,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use convex::{AuthenticationToken, ConvexClient, FunctionResult, Value};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::lifecycle::DesktopLifecycle;
use crate::sanitized::SanitizedProfileOutcome;

const KEYCHAIN_SERVICE: &str = "app.touchgrass.bar.profile";
const PROFILE_MUTATION: &str = "tokenmaxxers:ensureProfile";
const PREPARE_PATH: &str = "/api/auth/touchgrass/prepare";
const SIGN_UP_PATH: &str = "/api/auth/sign-up/email";
const SIGN_IN_PATH: &str = "/api/auth/sign-in/username";
const CONVEX_TOKEN_PATH: &str = "/api/auth/convex/token";
const SIGNUP_PROOF_HEADER: &str = "x-touchgrass-signup-proof";
const ID_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const SECRET_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

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

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileError(&'static str);

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
        let policy = keychain_policy(kind);
        let mut options = PasswordOptions::new_generic_password(KEYCHAIN_SERVICE, policy.account);
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
    fn ensure_profile(&self, session: &Secret, display_name: &str) -> Result<(), ProfileError>;
}

fn profile_mutation_payload(display_name: String) -> BTreeMap<String, Value> {
    BTreeMap::from([("displayName".to_owned(), Value::String(display_name))])
}

#[derive(Clone, Debug)]
pub(crate) struct RecoveryPresentation {
    pub touch_grass_id: String,
    pub recovery_key: Secret,
}

pub(crate) trait RecoverySheetPresenter: Send + Sync {
    fn present(&self, presentation: RecoveryPresentation) -> bool;
}

pub(crate) struct ProfileCoordinator {
    lifecycle: DesktopLifecycle,
    custody: Arc<dyn SecretCustody>,
    transport: Arc<dyn ProfileTransport>,
    presenter: Arc<dyn RecoverySheetPresenter>,
}

impl ProfileCoordinator {
    fn new(
        lifecycle: DesktopLifecycle,
        custody: Arc<dyn SecretCustody>,
        transport: Arc<dyn ProfileTransport>,
        presenter: Arc<dyn RecoverySheetPresenter>,
    ) -> Self {
        Self {
            lifecycle,
            custody,
            transport,
            presenter,
        }
    }

    pub(crate) fn retry_pending(&self) -> Result<Option<SanitizedProfileOutcome>, ProfileError> {
        let Some(request) = self.lifecycle.profile_request() else {
            self.present_pending_disclosure()?;
            return Ok(None);
        };

        self.ensure_secret(SecretKind::InstallationCredential, 52)?;
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
        self.transport
            .ensure_profile(&session, &request.display_name)?;
        self.lifecycle
            .mark_profile_ready(&prepared.touch_grass_id)
            .map_err(ProfileError)?;
        let _ = self.custody.delete(SecretKind::SignupPreparation);
        let profile = SanitizedProfileOutcome::Ready {
            display_name: request.display_name,
            touch_grass_id: prepared.touch_grass_id,
        };
        let _ = self.present_pending_disclosure();
        Ok(Some(profile))
    }

    pub(crate) fn present_pending_disclosure(&self) -> Result<(), ProfileError> {
        if !self.lifecycle.pending_recovery_disclosure() {
            return Ok(());
        }
        let touch_grass_id = self
            .lifecycle
            .ready_touch_grass_id()
            .ok_or(ProfileError("Profile disclosure pending"))?;
        let recovery_key = self
            .custody
            .read(SecretKind::RecoveryKey)?
            .ok_or(ProfileError("Profile disclosure pending"))?;
        if self.presenter.present(RecoveryPresentation {
            touch_grass_id,
            recovery_key,
        }) {
            self.lifecycle
                .mark_recovery_disclosed()
                .map_err(ProfileError)?;
        }
        Ok(())
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
pub(crate) fn production_coordinator(
    lifecycle: DesktopLifecycle,
    presenter: Arc<dyn RecoverySheetPresenter>,
) -> ProfileCoordinator {
    ProfileCoordinator::new(
        lifecycle,
        Arc::new(MacKeychain),
        Arc::new(HttpProfileTransport::from_build_configuration()),
        presenter,
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
    pub(crate) fn from_build_configuration() -> Self {
        Self {
            auth_site_url: option_env!("TOUCHGRASS_AUTH_SITE_URL")
                .filter(|value| !value.is_empty()),
            convex_url: option_env!("TOUCHGRASS_CONVEX_URL").filter(|value| !value.is_empty()),
            client: reqwest::blocking::Client::new(),
        }
    }

    fn endpoint(&self, path: &str) -> Result<String, ProfileError> {
        let base = self
            .auth_site_url
            .ok_or(ProfileError("profile service unavailable"))?;
        Ok(format!("{}{path}", base.trim_end_matches('/')))
    }
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

    fn ensure_profile(&self, session: &Secret, display_name: &str) -> Result<(), ProfileError> {
        let auth_site_url = self
            .auth_site_url
            .ok_or(ProfileError("profile service unavailable"))?
            .trim_end_matches('/')
            .to_owned();
        let convex_url = self
            .convex_url
            .ok_or(ProfileError("profile service unavailable"))?
            .to_owned();
        let session = Arc::new(Zeroizing::new(session.expose().to_owned()));
        let display_name = display_name.to_owned();
        tokio::runtime::Runtime::new()
            .map_err(|_| ProfileError("Profile creation pending"))?
            .block_on(async move {
                let mut client = ConvexClient::new(&convex_url)
                    .await
                    .map_err(|_| ProfileError("Profile creation pending"))?;
                let fetcher: convex::AuthTokenFetcher = Box::new(move |_force_refresh| {
                    let auth_site_url = auth_site_url.clone();
                    let session = Arc::clone(&session);
                    Box::pin(async move {
                        let response = reqwest::Client::new()
                            .get(format!("{auth_site_url}{CONVEX_TOKEN_PATH}"))
                            .bearer_auth(session.as_str())
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<ConvexTokenResponse>()
                            .await?;
                        Ok(AuthenticationToken::User(response.token))
                    })
                });
                client.set_auth_callback(Some(fetcher)).await;
                let result = client
                    .mutation(PROFILE_MUTATION, profile_mutation_payload(display_name))
                    .await
                    .map_err(|_| ProfileError("Profile creation pending"))?;
                client.set_auth_callback(None).await;
                match result {
                    FunctionResult::Value(_) => Ok(()),
                    FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
                        Err(ProfileError("Profile creation pending"))
                    }
                }
            })
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
        ) -> Result<(), ProfileError> {
            self.exchange_count.fetch_add(1, Ordering::SeqCst);
            let jwt = Secret::new(generate_secret(44)?);
            *self.last_jwt.lock().unwrap() = Some(jwt.expose().to_owned());
            self.fixed_profile_mutation.store(true, Ordering::SeqCst);
            drop(jwt);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePresenter {
        available: AtomicBool,
        last: Mutex<Option<RecoveryPresentation>>,
    }

    impl FakePresenter {
        fn set_available(&self, available: bool) {
            self.available.store(available, Ordering::SeqCst);
        }

        fn take_last(&self) -> Option<RecoveryPresentation> {
            self.last.lock().unwrap().take()
        }
    }

    impl RecoverySheetPresenter for FakePresenter {
        fn present(&self, presentation: RecoveryPresentation) -> bool {
            if !self.available.load(Ordering::SeqCst) {
                return false;
            }
            *self.last.lock().unwrap() = Some(presentation);
            true
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
        presenter: Arc<FakePresenter>,
        coordinator: ProfileCoordinator,
    }

    impl ProfileFixture {
        fn new() -> Self {
            let database = TestDatabase::new();
            let lifecycle = DesktopLifecycle::open(&database.0).unwrap();
            let custody = Arc::new(FakeCustody::default());
            let transport = Arc::new(FakeTransport::new());
            let presenter = Arc::new(FakePresenter::default());
            let coordinator = ProfileCoordinator::new(
                lifecycle.clone(),
                custody.clone(),
                transport.clone(),
                presenter.clone(),
            );
            Self {
                _database: database,
                lifecycle,
                custody,
                transport,
                presenter,
                coordinator,
            }
        }

        fn complete_bootstrap(&self) {
            self.lifecycle.complete_bootstrap("Fabien").unwrap();
        }

        fn public_boundaries(&self) -> Vec<String> {
            let core = NativeCore::unavailable();
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
                "recoverykey",
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
            client: reqwest::blocking::Client::new(),
        };

        let prepared = transport.prepare().expect("prepare JSON request");
        server.join().unwrap();

        assert_eq!(prepared.touch_grass_id, "TG-234567");
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
    fn disclosure_uses_only_the_native_presenter_and_sanitized_state() {
        let fixture = ProfileFixture::new();
        fixture.complete_bootstrap();
        fixture.presenter.set_available(false);
        let outcome = fixture.coordinator.retry_pending().unwrap();
        assert!(matches!(
            outcome,
            Some(SanitizedProfileOutcome::Ready { .. })
        ));
        assert!(fixture.presenter.take_last().is_none());
        assert!(fixture.lifecycle.pending_recovery_disclosure());

        fixture.presenter.set_available(true);
        fixture.coordinator.present_pending_disclosure().unwrap();
        let presentation = fixture.presenter.take_last().expect("native disclosure");
        assert_eq!(
            presentation.touch_grass_id,
            fixture.transport.touch_grass_id()
        );
        assert!(!presentation.recovery_key.expose().is_empty());

        let state = fixture
            .lifecycle
            .settings_state(LaunchAtLoginState::Unavailable);
        let serialized = serde_json::to_string(&state).unwrap();
        assert_eq!(
            state.touch_grass_id.as_deref(),
            Some(fixture.transport.touch_grass_id())
        );
        assert!(!fixture.lifecycle.pending_recovery_disclosure());
        assert!(!serialized.contains(presentation.recovery_key.expose()));
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
            pending.providers.map(|provider| provider.status),
            providers_before.map(|provider| provider.status)
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
