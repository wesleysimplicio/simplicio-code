//! Integration tests against a local `wiremock` server standing in for the
//! Simplicio identity + private-gateway backend (issue #3: device login,
//! refresh, revoke; issue #4: models/usage/chat-completions streaming,
//! cancellation, and 401/429/5xx/timeout). There is no real backend to
//! test against yet, so this is the strongest verification available:
//! `IdentityClient`/`PrivateGateway` driving real HTTP over a real
//! (loopback) socket against a server that plays the documented contract.
//!
//! Note: `Secret`/`SecretString::expose()` are `pub(crate)` by design (the
//! whole point is that nothing outside this crate can read a live secret
//! back out), so — correctly — these external tests can never assert on
//! raw token contents. They assert on state transitions and `is_some()` /
//! `is_none()` instead; content-level rotation checks already live in
//! `src/auth.rs`'s in-crate unit tests, which do have `pub(crate)` access.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use simplicio_code_gateway::{
    AuthEndpoints, AuthError, AuthSession, ChatRequest, DeviceAuthorization, Entitlement, GatewayError, GatewayLimits,
    IdentityClient, MemorySecretStore, PrivateGateway, SecretStore, SecretString, TokenResponse, paths,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_url(server: &MockServer) -> Url {
    Url::parse(&server.uri()).unwrap()
}

fn authorized_session() -> Arc<AuthSession<MemorySecretStore>> {
    let store = Arc::new(MemorySecretStore::new());
    let session = Arc::new(AuthSession::new(store));
    let now = Utc::now();
    session
        .install(
            TokenResponse {
                access_token: SecretString::new("test-access-token"),
                refresh_token: Some(SecretString::new("test-refresh-token")),
                expires_in: 3600,
                token_type: "Bearer".into(),
            },
            Entitlement { plan: "pro".into(), expires_at: now + chrono::Duration::hours(1), max_request_tokens: 10_000, max_tool_calls: 8 },
            now,
        )
        .unwrap();
    session
}

#[tokio::test]
async fn models_success_over_real_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::MODELS))
        .and(header("authorization", "Bearer test-access-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": "simplicio-1", "display_name": "Simplicio-1", "context_window": 262144}
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let models = gateway.models().await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "simplicio-1");
}

#[tokio::test]
async fn usage_success_over_real_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::USAGE))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "request_tokens": 10, "response_tokens": 20, "remaining_tokens": 9970, "remaining_tool_calls": 7
        })))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let usage = gateway.usage().await.unwrap();
    assert_eq!(usage.remaining_tool_calls, 7);
}

#[tokio::test]
async fn chat_stream_success_over_real_sse_http() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"id\":\"r1\",\"text_delta\":\"He\",\"tool_call\":null,\"usage\":null,\"done\":false}\n\n",
        "data: {\"id\":\"r1\",\"text_delta\":\"llo\",\"tool_call\":null,\"usage\":null,\"done\":false}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path(paths::CHAT_COMPLETIONS))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let request = ChatRequest::new(Vec::new(), 5);
    let limits = GatewayLimits { max_request_tokens: 10_000, max_tool_calls: 8 };

    use futures_util::StreamExt;
    let mut stream = gateway.chat_stream(request, limits, CancellationToken::new()).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.text_delta.as_deref(), Some("He"));
    let second = stream.next().await.unwrap().unwrap();
    assert_eq!(second.text_delta.as_deref(), Some("llo"));
    let done = stream.next().await.unwrap().unwrap();
    assert!(done.done);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn chat_stream_accepts_fragmented_crlf_frames_over_real_http() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 8192];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();

        async fn chunk(socket: &mut tokio::net::TcpStream, data: &[u8]) {
            let header = format!("{:X}\r\n", data.len());
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(data).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // The first event's CRLF delimiter is deliberately split at the
        // network chunk boundary: the parser must retain the trailing CR.
        chunk(&mut socket, b"data: {\"id\":\"r1\",\"text_delta\":\"He\",\"tool_call\":null,\"usage\":null,\"done\":false}\r").await;
        chunk(&mut socket, b"\n\r\ndata: {\"id\":\"r1\",\"text_delta\":\"llo\",\"tool_call\":null,\"usage\":null,\"done\":false}\r\n\r\n").await;
        chunk(&mut socket, b"data: [DONE]\r\n\r\n").await;
        socket.write_all(b"0\r\n\r\n").await.unwrap();
    });

    let gateway = PrivateGateway::new(Url::parse(&endpoint).unwrap(), authorized_session()).unwrap();
    let request = ChatRequest::new(Vec::new(), 5);
    let limits = GatewayLimits { max_request_tokens: 10_000, max_tool_calls: 8 };
    use futures_util::StreamExt;
    let mut stream = gateway.chat_stream(request, limits, CancellationToken::new()).await.unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().text_delta.as_deref(), Some("He"));
    assert_eq!(stream.next().await.unwrap().unwrap().text_delta.as_deref(), Some("llo"));
    assert!(stream.next().await.unwrap().unwrap().done);
    assert!(stream.next().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn chat_stream_is_cancellable_mid_stream() {
    // A slow/long stream: cancelling the token must surface
    // `GatewayError::Cancelled` promptly rather than the caller having to
    // wait for the whole body or drop the stream and hope for the best.
    let server = MockServer::start().await;
    let mut body = String::new();
    for i in 0..500 {
        body.push_str(&format!(
            "data: {{\"id\":\"r\",\"text_delta\":\"{i}\",\"tool_call\":null,\"usage\":null,\"done\":false}}\n\n"
        ));
    }
    Mock::given(method("POST"))
        .and(path(paths::CHAT_COMPLETIONS))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let request = ChatRequest::new(Vec::new(), 5);
    let limits = GatewayLimits { max_request_tokens: 10_000, max_tool_calls: 8 };
    let cancel = CancellationToken::new();

    use futures_util::StreamExt;
    let mut stream = gateway.chat_stream(request, limits, cancel.clone()).await.unwrap();
    let _first = stream.next().await.unwrap().unwrap();
    cancel.cancel();
    let mut saw_cancelled = false;
    while let Some(item) = stream.next().await {
        if matches!(item, Err(GatewayError::Cancelled)) {
            saw_cancelled = true;
            break;
        }
    }
    assert!(saw_cancelled, "expected a Cancelled error after cancel() during an in-flight stream");
}

#[tokio::test]
async fn unauthorized_401_is_a_stable_server_error_without_leaking_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::MODELS))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"message":"token abc123 rejected by upstream provider host provider.internal"}"#))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let err = gateway.models().await.unwrap_err();
    match err {
        GatewayError::Server { status, message, retry_after } => {
            assert_eq!(status, 401);
            assert!(!message.to_lowercase().contains("token"), "redacted message must not echo the raw token field: {message}");
            assert_eq!(retry_after, None, "401 response carried no Retry-After header");
        }
        other => panic!("expected Server{{401}}, got {other:?}"),
    }
}

#[tokio::test]
async fn identity_error_redacts_secret_and_hostname_from_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_AUTHORIZE))
        .respond_with(ResponseTemplate::new(502).set_body_string(
            r#"{"message":"device secret super-secret-123 rejected by identity.private.internal"}"#,
        ))
        .mount(&server)
        .await;

    let client = IdentityClient::new(
        AuthEndpoints::new(base_url(&server)).unwrap(),
        Arc::new(MemorySecretStore::new()),
    );
    let err = client.begin_device_authorization().await.unwrap_err();
    match err {
        AuthError::Server { status, message } => {
            assert_eq!(status, 502);
            assert!(!message.contains("super-secret-123"));
            assert!(!message.contains("identity.private.internal"));
            assert_eq!(message, "identity request failed");
        }
        other => panic!("expected AuthError::Server{{502}}, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limited_429_is_a_stable_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::USAGE))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "12").set_body_string(r#"{"message":"rate limited"}"#))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let err = gateway.usage().await.unwrap_err();
    match err {
        GatewayError::Server { status, retry_after, .. } => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(12)), "Retry-After: 12 must surface as a 12s backoff hint");
        }
        other => panic!("expected Server{{429}}, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limited_429_without_retry_after_header_yields_no_hint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::MODELS))
        .respond_with(ResponseTemplate::new(429).set_body_string(r#"{"message":"rate limited"}"#))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let err = gateway.models().await.unwrap_err();
    match err {
        GatewayError::Server { status, retry_after, .. } => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, None, "missing header must yield None, not a fabricated default");
        }
        other => panic!("expected Server{{429}}, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limited_429_with_unparseable_retry_after_yields_no_hint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::USAGE))
        // HTTP-date form is legal HTTP but intentionally not parsed by this
        // client (see `parse_retry_after` doc comment) — must degrade to
        // `None`, never panic or misparse into a bogus duration.
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "Wed, 21 Oct 2099 07:28:00 GMT").set_body_string(r#"{"message":"rate limited"}"#))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let err = gateway.usage().await.unwrap_err();
    match err {
        GatewayError::Server { status, retry_after, .. } => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, None);
        }
        other => panic!("expected Server{{429}}, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_stream_429_surfaces_retry_after_before_failing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::CHAT_COMPLETIONS))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7").set_body_string(r#"{"message":"rate limited"}"#))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let request = ChatRequest::new(Vec::new(), 5);
    let limits = GatewayLimits { max_request_tokens: 10_000, max_tool_calls: 8 };
    match gateway.chat_stream(request, limits, CancellationToken::new()).await {
        Err(GatewayError::Server { status, retry_after, .. }) => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(7)));
        }
        Err(other) => panic!("expected Server{{429}}, got {other:?}"),
        Ok(_) => panic!("expected an error, request should have failed with 429"),
    }
}

#[tokio::test]
async fn upstream_5xx_is_a_stable_server_error_without_naming_the_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::CHAT_COMPLETIONS))
        .respond_with(ResponseTemplate::new(502).set_body_string("upstream provider XYZ at host llm-backend.internal:443 returned a bad gateway"))
        .mount(&server)
        .await;

    let gateway = PrivateGateway::new(base_url(&server), authorized_session()).unwrap();
    let request = ChatRequest::new(Vec::new(), 5);
    let limits = GatewayLimits { max_request_tokens: 10_000, max_tool_calls: 8 };
    let result = gateway.chat_stream(request, limits, CancellationToken::new()).await;
    match result {
        Err(GatewayError::Server { status, .. }) => assert_eq!(status, 502),
        Err(other) => panic!("expected Server{{502}}, got {other:?}"),
        Ok(_) => panic!("expected an error, request should have failed with 502"),
    }
}

#[tokio::test]
async fn request_timeout_surfaces_as_a_transport_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(paths::MODELS))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let short_timeout_client = reqwest::Client::builder().timeout(Duration::from_millis(50)).build().unwrap();
    let gateway = PrivateGateway::with_http_client(base_url(&server), authorized_session(), short_timeout_client).unwrap();
    let err = gateway.models().await.unwrap_err();
    match err {
        GatewayError::Http(e) => assert!(e.is_timeout(), "expected a timeout reqwest error, got {e:?}"),
        other => panic!("expected Http(timeout), got {other:?}"),
    }
}

#[tokio::test]
async fn device_login_flow_pending_then_granted_over_real_http() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_AUTHORIZE))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "dc-1", "user_code": "ABCD-1234",
            "verification_uri": "https://auth.simplicio.dev/device", "expires_in": 600, "interval": 0
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_TOKEN))
        .respond_with(ResponseTemplate::new(202)) // still pending, once
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "granted-access", "refresh_token": "granted-refresh", "expires_in": 3600, "token_type": "Bearer"
        })))
        .mount(&server)
        .await;

    let endpoints = AuthEndpoints::new(base_url(&server)).unwrap();
    let store = Arc::new(MemorySecretStore::new());
    let client = IdentityClient::new(endpoints, store.clone());

    let device = client.begin_device_authorization().await.unwrap();
    assert_eq!(device.user_code, "ABCD-1234");

    let token = client
        .poll_device_authorization(&device, CancellationToken::new(), Utc::now())
        .await
        .expect("device flow should eventually grant a token (after one pending poll)");

    client
        .session
        .install(token, Entitlement { plan: "pro".into(), expires_at: Utc::now() + chrono::Duration::hours(1), max_request_tokens: 100, max_tool_calls: 1 }, Utc::now())
        .unwrap();
    assert!(store.load_refresh_token().unwrap().is_some(), "a granted device flow must persist a refresh token");
}

#[tokio::test]
async fn device_login_denied_maps_to_device_denied() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::DEVICE_TOKEN))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let endpoints = AuthEndpoints::new(base_url(&server)).unwrap();
    let store = Arc::new(MemorySecretStore::new());
    let client = IdentityClient::new(endpoints, store);
    let device = DeviceAuthorization {
        device_code: SecretString::new("dc-1"),
        user_code: "ABCD-1234".into(),
        verification_uri: Url::parse("https://auth.simplicio.dev/device").unwrap(),
        expires_in: 600,
        interval: 0,
    };
    let err = client.poll_device_authorization(&device, CancellationToken::new(), Utc::now()).await.unwrap_err();
    assert!(matches!(err, AuthError::DeviceDenied));
}

#[tokio::test]
async fn logout_revokes_remotely_and_clears_local_secret() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(paths::REVOKE))
        .and(header("authorization", "Bearer test-access-token"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let endpoints = AuthEndpoints::new(base_url(&server)).unwrap();
    let store = Arc::new(MemorySecretStore::new());
    let client = IdentityClient::with_http_client(endpoints, store.clone(), reqwest::Client::new());
    let now = Utc::now();
    client
        .session
        .install(
            TokenResponse {
                access_token: SecretString::new("test-access-token"),
                refresh_token: Some(SecretString::new("test-refresh-token")),
                expires_in: 3600,
                token_type: "Bearer".into(),
            },
            Entitlement { plan: "pro".into(), expires_at: now + chrono::Duration::hours(1), max_request_tokens: 10, max_tool_calls: 1 },
            now,
        )
        .unwrap();

    client.logout().await.expect("logout should succeed");
    assert!(store.load_refresh_token().unwrap().is_none(), "logout must clear the local secret");
    assert!(matches!(client.session.access_token(Utc::now()), Err(AuthError::Revoked)));
}
