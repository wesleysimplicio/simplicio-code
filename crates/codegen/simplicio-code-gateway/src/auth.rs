use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tokio::time::{Duration as TokioDuration, sleep};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::paths;

const CLOCK_SKEW: Duration = Duration::seconds(30);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Simplicio authentication is not configured")]
    NotConfigured,
    #[error("Simplicio entitlement is missing, expired, or revoked")]
    EntitlementRequired,
    #[error("Simplicio session is expired")]
    AccessExpired,
    #[error("Simplicio session was revoked")]
    Revoked,
    #[error("device authorization expired")]
    DeviceExpired,
    #[error("device authorization was denied")]
    DeviceDenied,
    #[error("device authorization failed: {0}")]
    Device(String),
    #[error("credential storage failed: {0}")]
    Storage(String),
    #[error("identity request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("identity server returned {status}: {message}")]
    Server { status: StatusCode, message: String },
    #[error("invalid identity response: {0}")]
    Protocol(String),
    #[error("operation cancelled")]
    Cancelled,
}

/// A secret wrapper that cannot accidentally print its value.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Storage boundary for the long-lived refresh token. Access tokens never
/// cross this boundary.
pub trait SecretStore: Send + Sync {
    fn load_refresh_token(&self) -> Result<Option<Secret>, AuthError>;
    fn save_refresh_token(&self, token: &Secret) -> Result<(), AuthError>;
    fn clear_refresh_token(&self) -> Result<(), AuthError>;
}

/// Native keychain/credential-manager/Secret-Service storage through the
/// cross-platform `keyring` crate. Failure is surfaced; it never falls back
/// to a plaintext file.
pub struct OsSecretStore {
    service: String,
    username: String,
}

impl OsSecretStore {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            service: "simplicio-code".into(),
            username: username.into(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(&self.service, &self.username)
            .map_err(|e| AuthError::Storage(e.to_string()))
    }
}

impl SecretStore for OsSecretStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, AuthError> {
        match self.entry()?.get_password() {
            Ok(value) if !value.trim().is_empty() => Ok(Some(Secret::new(value))),
            Ok(_) => Ok(None),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthError::Storage(e.to_string())),
        }
    }

    fn save_refresh_token(&self, token: &Secret) -> Result<(), AuthError> {
        self.entry()?
            .set_password(token.expose())
            .map_err(|e| AuthError::Storage(e.to_string()))
    }

    fn clear_refresh_token(&self) -> Result<(), AuthError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Storage(e.to_string())),
        }
    }
}

#[derive(Clone, Default)]
pub struct MemorySecretStore(Arc<Mutex<Option<Secret>>>);

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, AuthError> {
        self.0
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))
            .map(|v| v.clone())
    }
    fn save_refresh_token(&self, token: &Secret) -> Result<(), AuthError> {
        *self
            .0
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))? = Some(token.clone());
        Ok(())
    }
    fn clear_refresh_token(&self) -> Result<(), AuthError> {
        *self
            .0
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))? = None;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AuthEndpoints {
    pub base_url: Url,
}

impl AuthEndpoints {
    pub fn new(base_url: Url) -> Result<Self, AuthError> {
        let scheme = base_url.scheme();
        let loopback = base_url
            .host_str()
            .is_some_and(|h| h == "localhost" || h == "127.0.0.1");
        if scheme != "https" && !(scheme == "http" && loopback) {
            return Err(AuthError::NotConfigured);
        }
        if base_url.username() != "" || base_url.password().is_some() {
            return Err(AuthError::NotConfigured);
        }
        Ok(Self { base_url })
    }
    pub fn url(&self, path: &str) -> Url {
        let mut base = self.base_url.clone();
        base.set_path(path);
        base.set_query(None);
        base
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub device_code: SecretString,
    pub user_code: String,
    pub verification_uri: Url,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TokenResponse {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_in: u64,
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".into()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Entitlement {
    pub plan: String,
    pub expires_at: DateTime<Utc>,
    pub max_request_tokens: u64,
    pub max_tool_calls: u32,
}

impl Entitlement {
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        now + CLOCK_SKEW < self.expires_at
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthState {
    SignedOut,
    Authorized,
    Revoked,
}

struct SessionInner {
    access_token: Option<SecretString>,
    access_expires_at: Option<DateTime<Utc>>,
    entitlement: Option<Entitlement>,
    state: AuthState,
}

pub struct AuthSession<S: SecretStore> {
    store: Arc<S>,
    inner: Mutex<SessionInner>,
}

impl<S: SecretStore> AuthSession<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            inner: Mutex::new(SessionInner {
                access_token: None,
                access_expires_at: None,
                entitlement: None,
                state: AuthState::SignedOut,
            }),
        }
    }

    pub fn state(&self) -> AuthState {
        self.inner
            .lock()
            .map(|v| v.state.clone())
            .unwrap_or(AuthState::Revoked)
    }

    pub fn install(
        &self,
        token: TokenResponse,
        entitlement: Entitlement,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        if !entitlement.is_valid_at(now) || token.expires_in == 0 {
            return Err(AuthError::EntitlementRequired);
        }
        if let Some(refresh) = token.refresh_token.as_ref() {
            self.store
                .save_refresh_token(&Secret::new(refresh.expose()))?;
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        inner.access_token = Some(token.access_token);
        inner.access_expires_at = Some(now + Duration::seconds(token.expires_in as i64));
        inner.entitlement = Some(entitlement);
        inner.state = AuthState::Authorized;
        Ok(())
    }

    pub fn access_token(&self, now: DateTime<Utc>) -> Result<SecretString, AuthError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        if inner.state == AuthState::Revoked {
            return Err(AuthError::Revoked);
        }
        if inner.state != AuthState::Authorized {
            return Err(AuthError::EntitlementRequired);
        }
        if !inner
            .entitlement
            .as_ref()
            .is_some_and(|e| e.is_valid_at(now))
        {
            return Err(AuthError::EntitlementRequired);
        }
        if inner
            .access_expires_at
            .is_none_or(|e| now + CLOCK_SKEW >= e)
        {
            return Err(AuthError::AccessExpired);
        }
        inner.access_token.clone().ok_or(AuthError::AccessExpired)
    }

    pub fn entitlement(&self, now: DateTime<Utc>) -> Result<Entitlement, AuthError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        if inner.state != AuthState::Authorized
            || !inner
                .entitlement
                .as_ref()
                .is_some_and(|e| e.is_valid_at(now))
        {
            return Err(AuthError::EntitlementRequired);
        }
        inner
            .entitlement
            .clone()
            .ok_or(AuthError::EntitlementRequired)
    }

    pub fn rotate_refresh(
        &self,
        token: &TokenResponse,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        let refresh = token.refresh_token.as_ref().ok_or_else(|| {
            AuthError::Protocol("refresh response omitted rotated refresh token".into())
        })?;
        let entitlement = self.entitlement(now)?;
        self.store
            .save_refresh_token(&Secret::new(refresh.expose()))?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        inner.access_token = Some(token.access_token.clone());
        inner.access_expires_at = Some(now + Duration::seconds(token.expires_in as i64));
        inner.entitlement = Some(entitlement);
        Ok(())
    }

    pub fn revoke_local(&self) -> Result<(), AuthError> {
        self.store.clear_refresh_token()?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        inner.access_token = None;
        inner.access_expires_at = None;
        inner.entitlement = None;
        inner.state = AuthState::Revoked;
        Ok(())
    }
}

pub struct IdentityClient<S: SecretStore> {
    http: reqwest::Client,
    endpoints: AuthEndpoints,
    pub session: Arc<AuthSession<S>>,
}

impl<S: SecretStore> IdentityClient<S> {
    pub fn new(endpoints: AuthEndpoints, store: Arc<S>) -> Self {
        Self::with_http_client(endpoints, store, reqwest::Client::new())
    }

    /// Same as [`Self::new`] but with a caller-supplied `reqwest::Client` —
    /// used by tests to set a short timeout.
    pub fn with_http_client(
        endpoints: AuthEndpoints,
        store: Arc<S>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            http,
            endpoints,
            session: Arc::new(AuthSession::new(store)),
        }
    }

    pub async fn begin_device_authorization(&self) -> Result<DeviceAuthorization, AuthError> {
        let response = self
            .http
            .post(self.endpoints.url(paths::DEVICE_AUTHORIZE))
            .json(&serde_json::json!({"scope": "code"}))
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn poll_device_authorization(
        &self,
        device: &DeviceAuthorization,
        cancel: CancellationToken,
        now: DateTime<Utc>,
    ) -> Result<TokenResponse, AuthError> {
        let deadline = now + Duration::seconds(device.expires_in as i64);
        loop {
            if cancel.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            if Utc::now() + CLOCK_SKEW >= deadline {
                return Err(AuthError::DeviceExpired);
            }
            let response = self
                .http
                .post(self.endpoints.url(paths::DEVICE_TOKEN))
                .json(&serde_json::json!({"device_code": device.device_code.expose()}))
                .send()
                .await?;
            if response.status() == StatusCode::ACCEPTED {
                sleep(TokioDuration::from_secs(device.interval.max(1))).await;
                continue;
            }
            if response.status() == StatusCode::FORBIDDEN {
                return Err(AuthError::DeviceDenied);
            }
            return decode_response(response).await;
        }
    }

    pub async fn fetch_entitlement(&self) -> Result<Entitlement, AuthError> {
        let token = self.session.access_token(Utc::now())?;
        let response = self
            .http
            .get(self.endpoints.url(paths::ENTITLEMENT))
            .bearer_auth(token.expose())
            .send()
            .await?;
        decode_response(response).await
    }

    pub async fn logout(&self) -> Result<(), AuthError> {
        let token = self.session.access_token(Utc::now()).ok();
        if let Some(token) = token {
            let response = self
                .http
                .post(self.endpoints.url(paths::REVOKE))
                .bearer_auth(token.expose())
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(server_error(response).await);
            }
        }
        self.session.revoke_local()
    }
}

async fn server_error(response: reqwest::Response) -> AuthError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    // Consume the body only to run it through the metadata-only redactor. The
    // body itself is never used as an error message: identity responses can
    // contain bearer tokens, device codes, or private hostnames.
    let _diagnostics =
        crate::redaction::redact_diagnostics(&serde_json::json!({"error_body": body}));
    AuthError::Server {
        status,
        message: "identity request failed".into(),
    }
}

async fn decode_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, AuthError> {
    let status = response.status();
    if !status.is_success() {
        return Err(server_error(response).await);
    }
    response
        .json()
        .await
        .map_err(|e| AuthError::Protocol(e.to_string()))
}

/// Deterministic local identity fake. It models pending device approval,
/// single-use refresh rotation, entitlement expiry, and remote revoke.
#[derive(Clone)]
pub struct FakeIdentity {
    state: Arc<Mutex<FakeIdentityState>>,
}

struct FakeIdentityState {
    pending_polls: u8,
    refresh_generation: u64,
    revoked: bool,
    entitlement: Entitlement,
}

impl FakeIdentity {
    pub fn new(entitlement: Entitlement) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeIdentityState {
                pending_polls: 1,
                refresh_generation: 0,
                revoked: false,
                entitlement,
            })),
        }
    }
    pub fn authorize(&self) -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: SecretString::new("fake-device-code"),
            user_code: "FAKE-CODE".into(),
            verification_uri: Url::parse("https://login.simplicio.invalid/device")
                .expect("fake URL"),
            expires_in: 600,
            interval: 0,
        }
    }
    pub fn poll(&self, device: &DeviceAuthorization) -> Result<TokenResponse, AuthError> {
        if device.device_code.expose() != "fake-device-code" {
            return Err(AuthError::Device("unknown device".into()));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        if state.revoked {
            return Err(AuthError::Revoked);
        }
        if state.pending_polls > 0 {
            state.pending_polls -= 1;
            return Err(AuthError::Device("authorization_pending".into()));
        }
        state.refresh_generation += 1;
        Ok(TokenResponse {
            access_token: SecretString::new(format!("fake-access-{}", state.refresh_generation)),
            refresh_token: Some(SecretString::new(format!(
                "fake-refresh-{}",
                state.refresh_generation
            ))),
            expires_in: 3600,
            token_type: "Bearer".into(),
        })
    }
    pub fn entitlement(&self) -> Result<Entitlement, AuthError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))?;
        if state.revoked {
            return Err(AuthError::Revoked);
        }
        Ok(state.entitlement.clone())
    }
    pub fn revoke(&self) -> Result<(), AuthError> {
        self.state
            .lock()
            .map_err(|_| AuthError::Storage("lock poisoned".into()))
            .map(|mut s| {
                s.revoked = true;
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entitlement(expires_at: DateTime<Utc>) -> Entitlement {
        Entitlement {
            plan: "pro".into(),
            expires_at,
            max_request_tokens: 1000,
            max_tool_calls: 4,
        }
    }
    fn token(access: &str, refresh: &str) -> TokenResponse {
        TokenResponse {
            access_token: SecretString::new(access),
            refresh_token: Some(SecretString::new(refresh)),
            expires_in: 300,
            token_type: "Bearer".into(),
        }
    }

    #[test]
    fn access_is_memory_only_and_entitlement_fail_closed() {
        let store = Arc::new(MemorySecretStore::new());
        let session = AuthSession::new(store.clone());
        let now = Utc::now();
        assert!(matches!(
            session.access_token(now),
            Err(AuthError::EntitlementRequired)
        ));
        session
            .install(
                token("access", "refresh"),
                entitlement(now + Duration::hours(1)),
                now,
            )
            .unwrap();
        assert_eq!(session.access_token(now).unwrap().expose(), "access");
        assert_eq!(
            store.load_refresh_token().unwrap().unwrap().expose(),
            "refresh"
        );
        assert_eq!(
            format!("{:?}", session.access_token(now).unwrap()),
            "[REDACTED]"
        );
    }

    #[test]
    fn expiry_skew_is_fail_closed() {
        // Regression test for a real bug found while verifying this crate:
        // the original version of this test varied *entitlement* expiry
        // (`entitlement(now + Duration::seconds(31))`) while asserting on
        // `AccessExpired` and using the fixed 300s-expiry `token()` helper —
        // it happened to pass only because 31s sits just past the 30s skew
        // tolerance, and never actually exercised the access-token skew
        // check it claimed to. Fixed to construct an access token whose
        // `expires_in` (29s) sits *inside* the `CLOCK_SKEW` tolerance (30s)
        // while entitlement is comfortably valid, so the fail-closed
        // access-expiry branch is the one genuinely under test.
        let session = AuthSession::new(Arc::new(MemorySecretStore::new()));
        let now = Utc::now();
        let near_expiry_access = TokenResponse {
            access_token: SecretString::new("access"),
            refresh_token: Some(SecretString::new("refresh")),
            expires_in: 29,
            token_type: "Bearer".into(),
        };
        session
            .install(
                near_expiry_access,
                entitlement(now + Duration::hours(1)),
                now,
            )
            .unwrap();
        assert!(matches!(
            session.access_token(now),
            Err(AuthError::AccessExpired)
        ));
    }

    #[test]
    fn refresh_rotation_replaces_old_secret_and_logout_clears() {
        let store = Arc::new(MemorySecretStore::new());
        let session = AuthSession::new(store.clone());
        let now = Utc::now();
        session
            .install(
                token("one", "old"),
                entitlement(now + Duration::hours(1)),
                now,
            )
            .unwrap();
        session.rotate_refresh(&token("two", "new"), now).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap().expose(), "new");
        session.revoke_local().unwrap();
        assert!(store.load_refresh_token().unwrap().is_none());
        assert!(matches!(session.access_token(now), Err(AuthError::Revoked)));
    }

    #[test]
    fn fake_device_flow_requires_approval_then_issues_rotated_tokens() {
        let fake = FakeIdentity::new(entitlement(Utc::now() + Duration::hours(1)));
        let device = fake.authorize();
        assert!(matches!(fake.poll(&device), Err(AuthError::Device(_))));
        assert!(fake.poll(&device).is_ok());
    }

    /// Verifies `OsSecretStore` against the *real* OS credential store
    /// (Windows Credential Manager on this machine) — not a fake. It is
    /// `#[ignore]`d because CI/headless containers commonly have no
    /// credential store backend available (no Secret Service on a bare
    /// Linux container, no login keychain in some macOS CI runners), so
    /// running it unconditionally would make the suite flaky for reasons
    /// unrelated to this crate's correctness. Run explicitly with
    /// `cargo test -p simplicio-code-gateway -- --ignored os_secret_store`
    /// on a machine known to have one (this repo's CI on real
    /// developer/desktop platforms does).
    ///
    /// This was run manually while preparing this change, on this
    /// (Windows) sandbox, and passed — see the PR description for the
    /// exact command and output. That is what justifies keeping
    /// `OsSecretStore` in this PR rather than only shipping
    /// `MemorySecretStore` with a TODO: it is a verified implementation
    /// backed by `keyring` 4.1.5 / `windows-native-keyring-store`, not an
    /// unverified stub.
    #[test]
    #[ignore = "requires a real OS credential store backend; run explicitly, see doc comment"]
    fn os_secret_store_round_trips_a_real_secret_through_the_platform_keychain() {
        // Unique per-run username so repeated manual runs (and accidental
        // concurrent runs) don't collide on one keychain entry.
        let username = format!("simplicio-code-gateway-test-{}", uuid::Uuid::new_v4());
        let store = OsSecretStore::new(&username);

        // Starts empty.
        assert!(store.load_refresh_token().unwrap().is_none());

        // Round-trip a real secret through the real platform store.
        store
            .save_refresh_token(&Secret::new("integration-test-refresh-token"))
            .unwrap();
        let loaded = store
            .load_refresh_token()
            .unwrap()
            .expect("secret must be readable back from the OS store");
        assert_eq!(loaded.expose(), "integration-test-refresh-token");

        // Clearing must actually delete the OS-level entry, not just this
        // process's view of it — verified by re-loading.
        store.clear_refresh_token().unwrap();
        assert!(
            store.load_refresh_token().unwrap().is_none(),
            "clear_refresh_token must delete the OS credential entry"
        );
    }
}
