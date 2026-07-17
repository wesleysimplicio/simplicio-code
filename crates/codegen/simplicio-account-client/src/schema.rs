//! Typed mirrors of `docs/contracts/site-simpleti-openapi.yaml`.
//!
//! Field names and shapes here must stay in lockstep with that YAML file;
//! the round-trip tests below parse literal example payloads copied from the
//! spec's schemas to catch drift between the two.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceAuthorizeRequest {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceAuthorizeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u32,
    pub interval: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceTokenRequest {
    pub device_code: String,
    pub client_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTokenErrorCode {
    AuthorizationPending,
    SlowDown,
    ExpiredToken,
    AccessDenied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceTokenError {
    pub error: DeviceTokenErrorCode,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u32,
}

/// Manual `Debug` impl: `access_token`/`refresh_token` are raw OAuth
/// secrets returned by the device-auth flow. A derived `Debug` would print
/// both verbatim on any `{:?}` of the parsed response (e.g. diagnostic
/// logging of the auth exchange).
impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Manual `Debug` impl: `refresh_token` is a raw OAuth secret.
impl std::fmt::Debug for RefreshRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshRequest")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenTypeHint {
    RefreshToken,
    AccessToken,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokeRequest {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type_hint: Option<TokenTypeHint>,
}

/// Manual `Debug` impl: `token` is a raw OAuth access/refresh token being
/// revoked.
impl std::fmt::Debug for RevokeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevokeRequest")
            .field("token", &"<redacted>")
            .field("token_type_hint", &self.token_type_hint)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementStatus {
    Active,
    PastDue,
    Canceled,
    Trialing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitlementResponse {
    pub plan: String,
    pub status: EntitlementStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renews_at: Option<String>,
    pub profiles: Vec<String>,
    #[serde(default)]
    pub limits: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSummary {
    pub device_id: String,
    pub label: String,
    pub last_seen_at: String,
    #[serde(default)]
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WebhookEventType {
    #[serde(rename = "subscription.created")]
    SubscriptionCreated,
    #[serde(rename = "subscription.updated")]
    SubscriptionUpdated,
    #[serde(rename = "subscription.canceled")]
    SubscriptionCanceled,
    #[serde(rename = "subscription.payment_failed")]
    SubscriptionPaymentFailed,
}

/// Mirrors `WebhookEvent` in the OpenAPI contract. `event_id` is the
/// idempotency key consumed by [`crate::idempotency`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebhookEvent {
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: WebhookEventType,
    pub created_at: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each of these payloads is a literal instance of a schema from
    // docs/contracts/site-simpleti-openapi.yaml. If a required field is
    // renamed or removed there without updating this crate, these round
    // trips catch it.

    #[test]
    fn device_authorize_response_matches_openapi_example() {
        let json = r#"{
            "device_code": "dc_abc123",
            "user_code": "ABCD-1234",
            "verification_uri": "https://simplicio.dev/device",
            "verification_uri_complete": "https://simplicio.dev/device?user_code=ABCD-1234",
            "expires_in": 900,
            "interval": 5
        }"#;
        let parsed: DeviceAuthorizeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.user_code, "ABCD-1234");
        assert!(parsed.expires_in <= 900, "device codes must be short-lived");
    }

    #[test]
    fn device_token_error_uses_rfc8628_error_codes() {
        let json = r#"{"error": "expired_token"}"#;
        let parsed: DeviceTokenError = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.error, DeviceTokenErrorCode::ExpiredToken);
    }

    #[test]
    fn entitlement_response_never_requires_upstream_provider_field() {
        let json = r#"{
            "plan": "beta",
            "status": "active",
            "profiles": ["Simplicio-1"],
            "limits": {"requests_per_day": 500}
        }"#;
        let parsed: EntitlementResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.profiles, vec!["Simplicio-1".to_string()]);
        // Contract-level guard: reject accidental leakage of upstream/model
        // vendor identifiers into a client-visible entitlement profile name.
        let leaked_vendor_terms = ["grok", "openrouter", "opencode", "xai"];
        for profile in &parsed.profiles {
            let lower = profile.to_lowercase();
            for term in leaked_vendor_terms {
                assert!(
                    !lower.contains(term),
                    "entitlement profile {profile:?} must not name an upstream provider"
                );
            }
        }
    }

    #[test]
    fn webhook_event_round_trips() {
        let json = r#"{
            "event_id": "evt_1",
            "type": "subscription.updated",
            "created_at": "2026-07-17T00:00:00Z",
            "data": {"plan": "pro"}
        }"#;
        let parsed: WebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.event_id, "evt_1");
        assert_eq!(parsed.event_type, WebhookEventType::SubscriptionUpdated);
    }

    /// `TokenResponse`/`RefreshRequest`/`RevokeRequest` all carry raw OAuth
    /// secrets (access/refresh tokens). None may appear in `{:?}` output.
    #[test]
    fn oauth_secret_bearing_types_never_print_raw_tokens_in_debug() {
        const CANARY_ACCESS_TOKEN: &str = "canary-access-token-00000000";
        const CANARY_REFRESH_TOKEN: &str = "canary-refresh-token-11111111";

        let token_response = TokenResponse {
            access_token: CANARY_ACCESS_TOKEN.to_string(),
            refresh_token: CANARY_REFRESH_TOKEN.to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        };
        let debug_output = format!("{token_response:?}");
        assert!(
            !debug_output.contains(CANARY_ACCESS_TOKEN),
            "{debug_output}"
        );
        assert!(
            !debug_output.contains(CANARY_REFRESH_TOKEN),
            "{debug_output}"
        );
        assert!(debug_output.contains("<redacted>"));

        let refresh_request = RefreshRequest {
            refresh_token: CANARY_REFRESH_TOKEN.to_string(),
        };
        let debug_output = format!("{refresh_request:?}");
        assert!(
            !debug_output.contains(CANARY_REFRESH_TOKEN),
            "{debug_output}"
        );
        assert!(debug_output.contains("<redacted>"));

        let revoke_request = RevokeRequest {
            token: CANARY_ACCESS_TOKEN.to_string(),
            token_type_hint: Some(TokenTypeHint::AccessToken),
        };
        let debug_output = format!("{revoke_request:?}");
        assert!(
            !debug_output.contains(CANARY_ACCESS_TOKEN),
            "{debug_output}"
        );
        assert!(debug_output.contains("<redacted>"));

        // Serialization must still carry the real values on the wire.
        let json = serde_json::to_string(&token_response).unwrap();
        assert!(json.contains(CANARY_ACCESS_TOKEN));
        assert!(json.contains(CANARY_REFRESH_TOKEN));
    }
}
