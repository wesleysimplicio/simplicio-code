//! Lightweight Mixpanel HTTP tracking client.
//!
//! This is a minimal replacement for `mixpanel-rs` that uses `reqwest 0.12`
//! instead of `reqwest 0.11`, avoiding a duplicate HTTP stack in the binary.
//!
//! Only the `track` API is implemented since that's all we use.

use base64::Engine;
use std::collections::HashMap;

/// Production Mixpanel ingestion host. This is the only host this crate
/// contacts unless a caller explicitly overrides it via
/// [`Mixpanel::with_client_and_base_url`] (used for tests and, in
/// principle, an enterprise on-prem proxy).
pub const DEFAULT_BASE_URL: &str = "https://api.mixpanel.com";

/// Mixpanel client for sending track events.
#[derive(Clone)]
pub struct Mixpanel {
    token: String,
    client: reqwest::Client,
    base_url: String,
}

/// Error type for Mixpanel operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl Mixpanel {
    /// Create a new Mixpanel client with the given project token. Posts to
    /// the production [`DEFAULT_BASE_URL`].
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_client(token, reqwest::Client::new())
    }

    /// Create a new Mixpanel client with a shared reqwest client. Posts to
    /// the production [`DEFAULT_BASE_URL`].
    pub fn with_client(token: impl Into<String>, client: reqwest::Client) -> Self {
        Self::with_client_and_base_url(token, client, DEFAULT_BASE_URL)
    }

    /// Create a new Mixpanel client that posts to `base_url` instead of the
    /// production Mixpanel host. This exists so tests (and, if ever needed,
    /// an enterprise on-prem ingestion proxy) can redirect outbound calls
    /// without touching `api.mixpanel.com` — it is how
    /// `xai-grok-telemetry`'s network-capture test proves the Mixpanel path
    /// only ever contacts the host it was configured with.
    pub fn with_client_and_base_url(
        token: impl Into<String>,
        client: reqwest::Client,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            client,
            base_url: base_url.into(),
        }
    }

    /// Scrub property string values in place, then inject the project
    /// token. Split out from [`Self::track`] so the scrub-then-inject
    /// ordering is testable.
    fn prepare_properties(
        &self,
        mut properties: HashMap<String, serde_json::Value>,
    ) -> HashMap<String, serde_json::Value> {
        for v in properties.values_mut() {
            xai_grok_secrets::redact_json_string_values(v);
        }
        properties.insert("token".to_owned(), serde_json::json!(self.token));
        properties
    }

    /// Track an event. Properties should include `distinct_id`. The
    /// project `token` is injected after scrubbing, so it isn't redacted.
    pub async fn track(
        &self,
        event: &str,
        properties: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<(), Error> {
        let props = self.prepare_properties(properties.unwrap_or_default());

        let payload = serde_json::json!([{
            "event": event,
            "properties": props,
        }]);

        let json_bytes = serde_json::to_vec(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json_bytes);

        self.client
            .post(format!("{}/track", self.base_url))
            .form(&[("data", &encoded)])
            .send()
            .await?;

        Ok(())
    }

    /// Create or update a user profile via Mixpanel's Engage API.
    /// String values in `set` are scrubbed for secrets before sending.
    /// The project `token` is injected automatically.
    pub async fn engage(
        &self,
        distinct_id: &str,
        set: HashMap<String, serde_json::Value>,
    ) -> Result<(), Error> {
        let mut scrubbed = set;
        for v in scrubbed.values_mut() {
            xai_grok_secrets::redact_json_string_values(v);
        }

        let payload = serde_json::json!([{
            "$token": self.token,
            "$distinct_id": distinct_id,
            "$set": scrubbed,
        }]);

        let json_bytes = serde_json::to_vec(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json_bytes);

        self.client
            .post(format!("{}/engage", self.base_url))
            .form(&[("data", &encoded)])
            .send()
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project token is deliberately Bearer-shaped: it would be redacted
    /// if `prepare_properties` ran the scrubber after token injection.
    /// The `error` value catches the inverse regression: if the scrub
    /// loop is dropped, the user-supplied Bearer leaks.
    #[test]
    fn prepare_properties_scrubs_then_injects_token() {
        let project_token = "Bearer fake-project-token-abcdef0123456789";
        let mp = Mixpanel::new(project_token);

        let mut props = HashMap::new();
        props.insert("error".into(), "Bearer abcdef0123456789abcdef".into());

        let prepared = mp.prepare_properties(props);

        assert_eq!(prepared["token"], project_token, "project token redacted");
        let error = prepared["error"].as_str().unwrap();
        assert!(
            !error.contains("abcdef0123456789abcdef"),
            "secret leaked: {error}"
        );
    }

    /// `new()`/`with_client()` must default to the production Mixpanel host
    /// — nothing else contacts a different endpoint unless a caller opts in
    /// via `with_client_and_base_url`.
    #[test]
    fn default_constructors_use_production_base_url() {
        assert_eq!(Mixpanel::new("tok").base_url, DEFAULT_BASE_URL);
        assert_eq!(
            Mixpanel::with_client("tok", reqwest::Client::new()).base_url,
            DEFAULT_BASE_URL
        );
    }

    /// The override constructor actually overrides — this is the mechanism
    /// the `xai-grok-telemetry` network-capture test relies on to redirect
    /// outbound Mixpanel calls to a loopback test server instead of
    /// `api.mixpanel.com`.
    #[test]
    fn override_constructor_replaces_base_url() {
        let mp =
            Mixpanel::with_client_and_base_url("tok", reqwest::Client::new(), "http://127.0.0.1:9");
        assert_eq!(mp.base_url, "http://127.0.0.1:9");
    }
}
