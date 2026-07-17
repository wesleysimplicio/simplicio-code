//! Golden-payload contract tests: each fixture under `tests/fixtures/`
//! matches a schema in `openapi/simplicio-gateway.yaml` and must
//! round-trip through this crate's real (non-fake) response types.

use simplicio_code_gateway::{DeviceAuthorization, Entitlement, GatewayModel, GatewayUsage, PUBLIC_MODEL_ID};

#[test]
fn models_response_fixture_matches_gateway_model_schema() {
    let raw = include_str!("fixtures/models_response.json");
    let parsed: Vec<GatewayModel> = serde_json::from_str(raw).expect("fixture must deserialize into Vec<GatewayModel>");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, PUBLIC_MODEL_ID, "simplicio-1 must be the only publicly-named model");
}

#[test]
fn usage_response_fixture_matches_gateway_usage_schema() {
    let raw = include_str!("fixtures/usage_response.json");
    let parsed: GatewayUsage = serde_json::from_str(raw).expect("fixture must deserialize into GatewayUsage");
    assert!(parsed.remaining_tokens <= 10_000);

    // Regression guard: the usage contract must never grow a
    // provider-identifying field. Assert the exact known field set.
    let value: serde_json::Value = serde_json::from_str(raw).unwrap();
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["remaining_tokens", "remaining_tool_calls", "request_tokens", "response_tokens"]);
}

#[test]
fn entitlement_response_fixture_matches_entitlement_schema_and_hides_provider() {
    let raw = include_str!("fixtures/entitlement_response.json");
    let parsed: Entitlement = serde_json::from_str(raw).expect("fixture must deserialize into Entitlement");
    assert_eq!(parsed.plan, "simplicio-pro");

    let value: serde_json::Value = serde_json::from_str(raw).unwrap();
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["expires_at", "max_request_tokens", "max_tool_calls", "plan"],
        "entitlement must reveal plan/limits/validity only — never a provider identity"
    );
}

#[test]
fn device_authorization_fixture_matches_schema() {
    let raw = include_str!("fixtures/device_authorization.json");
    let parsed: DeviceAuthorization = serde_json::from_str(raw).expect("fixture must deserialize into DeviceAuthorization");
    assert_eq!(parsed.user_code, "GOLD-EN01");
}

#[test]
fn openapi_schema_declares_every_path_this_crate_actually_calls() {
    let raw = include_str!("../openapi/simplicio-gateway.yaml");
    use simplicio_code_gateway::paths;
    for path in [
        paths::DEVICE_AUTHORIZE,
        paths::DEVICE_TOKEN,
        paths::REFRESH,
        paths::REVOKE,
        paths::ENTITLEMENT,
        paths::MODELS,
        paths::CHAT_COMPLETIONS,
        paths::USAGE,
    ] {
        assert!(raw.contains(path), "openapi schema missing path used by the client: {path}");
    }
    assert!(raw.contains(PUBLIC_MODEL_ID));
    assert!(!raw.to_lowercase().contains("openrouter"), "schema must never name an upstream provider");
    assert!(!raw.to_lowercase().contains("x.ai"), "schema must never name an upstream provider host");
}

#[test]
fn openapi_bootstrap_device_endpoints_override_global_bearer_security() {
    let raw = include_str!("../openapi/simplicio-gateway.yaml");
    for path in ["/v1/code/auth/device/authorize:", "/v1/code/auth/device/token:"] {
        let section = raw.split_once(path).expect("device endpoint must be declared").1;
        let operation = section.split_once("\n  /").map_or(section, |(operation, _)| operation);
        assert!(operation.contains("security: []"), "bootstrap endpoint must be anonymous: {path}");
    }
}
