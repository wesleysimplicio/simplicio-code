use chrono::{Duration, Utc};
use simplicio_code_gateway::{
    ChatRequest, Entitlement, FakeGateway, FakeIdentity, GatewayLimits, MemorySecretStore,
    AuthSession,
};
use std::sync::Arc;

#[test]
fn fake_identity_and_gateway_contract_is_local_and_fail_closed() {
    let now = Utc::now();
    let identity = FakeIdentity::new(Entitlement {
        plan: "pro".into(),
        expires_at: now + Duration::hours(1),
        max_request_tokens: 128,
        max_tool_calls: 1,
    });
    let device = identity.authorize();
    let _ = identity.poll(&device);
    let token = identity.poll(&device).expect("fake approval");
    let store = Arc::new(MemorySecretStore::new());
    let session = Arc::new(AuthSession::new(store));
    session
        .install(token, identity.entitlement().unwrap(), now)
        .unwrap();

    let gateway = FakeGateway::new(session.clone());
    let response = gateway
        .stream(&ChatRequest::new(Vec::new(), 3), GatewayLimits { max_request_tokens: 128, max_tool_calls: 1 })
        .unwrap();
    assert!(response.iter().any(|event| event.text_delta.is_some()));

    identity.revoke().unwrap();
    session.revoke_local().unwrap();
    assert!(gateway.models().is_err());
}
