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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenTypeHint {
    RefreshToken,
    AccessToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokeRequest {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type_hint: Option<TokenTypeHint>,
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
}
