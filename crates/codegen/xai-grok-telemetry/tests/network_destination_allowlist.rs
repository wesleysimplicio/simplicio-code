//! Network-capture allowlist test (issue #13, "testar proxy que captura
//! toda saída de rede").
//!
//! This is not a full end-to-end MITM-proxy test around the built binary —
//! that would need to intercept the whole process's network stack, which is
//! a much bigger integration-test investment (tracked as follow-up in
//! `docs/privacy/network-destinations.md`). What this test *does* prove,
//! for real, over a real socket:
//!
//! 1. A single local HTTP server on loopback is the *only* destination
//!    configured for both telemetry sinks this crate owns (product events
//!    + Mixpanel `track`/`engage`).
//! 2. Driving a representative telemetry operation (`init` — which fires
//!    Mixpanel `engage` via `sync_profile` — followed by `log_event`, which
//!    fires both the product-events POST and Mixpanel `track`) causes
//!    requests to land *only* on the documented paths (`/events`,
//!    `/track`, `/engage`) and nowhere else, via a catch-all fallback route
//!    on the same server that fails the test if it's ever hit.
//! 3. The hosts these sinks are configured with in production (no
//!    override) are exactly the ones in
//!    `xai_grok_telemetry::allowlist::ALLOWED_TELEMETRY_HOSTS` — checked in
//!    `allowlist.rs`'s own unit tests, cross-referenced here so a reader
//!    doesn't have to trust that separately.
//!
//! Together: "when telemetry redirect config points at host X, nothing but
//! host X (on the documented paths) is contacted for these operations" —
//! the closest proportionate substitute for a full network-capture proxy
//! test that fits a single crate's test suite.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use xai_grok_telemetry::client;
use xai_grok_telemetry::config::{TelemetryConfig, TelemetryMode};
use xai_grok_telemetry::events::{AuthTokenKind, ManualAuth, ManualAuthReason, ManualAuthSurface};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_operation_only_contacts_configured_loopback_paths() {
    let captured_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let unexpected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let events_paths = captured_paths.clone();
    let track_paths = captured_paths.clone();
    let engage_paths = captured_paths.clone();
    let fallback_unexpected = unexpected.clone();

    let app = axum::Router::new()
        .route(
            "/events",
            axum::routing::post(move |_body: axum::body::Bytes| {
                let events_paths = events_paths.clone();
                async move {
                    events_paths.lock().unwrap().push("/events".to_string());
                    axum::http::StatusCode::OK
                }
            }),
        )
        .route(
            "/track",
            axum::routing::post(move |_body: axum::body::Bytes| {
                let track_paths = track_paths.clone();
                async move {
                    track_paths.lock().unwrap().push("/track".to_string());
                    axum::http::StatusCode::OK
                }
            }),
        )
        .route(
            "/engage",
            axum::routing::post(move |_body: axum::body::Bytes| {
                let engage_paths = engage_paths.clone();
                async move {
                    engage_paths.lock().unwrap().push("/engage".to_string());
                    axum::http::StatusCode::OK
                }
            }),
        )
        // Catch-all: any request that lands here targets an undocumented
        // path/host relative to what this test configured. This is the
        // "nothing undocumented goes out" assertion made concrete for the
        // one destination we control end-to-end in-process.
        .fallback(move |req: axum::extract::Request| {
            let fallback_unexpected = fallback_unexpected.clone();
            let uri = req.uri().to_string();
            async move {
                fallback_unexpected.lock().unwrap().push(uri);
                axum::http::StatusCode::NOT_FOUND
            }
        });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Point BOTH telemetry sinks this crate owns at the same loopback
    // server, redirecting them away from their real production hosts
    // (`cli-chat-proxy.grok.com` for events, `api.mixpanel.com` for
    // Mixpanel) using the config-level overrides
    // (`events_url` / `mixpanel_base_url`).
    client::init(
        TelemetryConfig {
            events_url: Some(format!("{base_url}/events")),
            events_api_key: Some("test-key".into()),
            mixpanel_enabled: true,
            mixpanel_token: Some("test-mixpanel-token".into()),
            mixpanel_base_url: Some(base_url.clone()),
            ..TelemetryConfig::default()
        },
        TelemetryMode::Enabled,
        Some("user-xyz".into()),
        None,
        None,
        None,
        "0.0.0-test".into(),
        None,
        reqwest::Client::new(),
    );
    // `init` fires `sync_profile` (Mixpanel `engage`) fire-and-forget.

    // A representative telemetry event: fires both the product-events POST
    // and Mixpanel `track`.
    xai_grok_telemetry::log_event(ManualAuth {
        reason: ManualAuthReason::RefreshTokenRejected,
        trigger: ManualAuthSurface::Turn,
        token_kind: AuthTokenKind::OidcSession,
        principal: Some("user-xyz".into()),
    });

    // Poll for all three expected requests (fire-and-forget async sends).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let paths = captured_paths.lock().unwrap();
        let has_events = paths.iter().any(|p| p == "/events");
        let has_track = paths.iter().any(|p| p == "/track");
        let has_engage = paths.iter().any(|p| p == "/engage");
        if has_events && has_track && has_engage {
            break;
        }
        drop(paths);
        assert!(
            Instant::now() < deadline,
            "timed out waiting for all three telemetry POSTs; got: {:?}",
            captured_paths.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Give any stray/unexpected request a moment to land before asserting
    // the fallback route was never hit.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let unexpected_hits = unexpected.lock().unwrap();
    assert!(
        unexpected_hits.is_empty(),
        "telemetry contacted undocumented path(s) on the redirected host: {:?}",
        *unexpected_hits
    );

    let mut paths = captured_paths.lock().unwrap().clone();
    paths.sort();
    paths.dedup();
    assert_eq!(
        paths,
        vec![
            "/engage".to_string(),
            "/events".to_string(),
            "/track".to_string(),
        ],
        "exactly the three documented telemetry paths, nothing else"
    );

    server.abort();
}

/// Cross-references `allowlist::ALLOWED_TELEMETRY_HOSTS` against the actual
/// production defaults these two sinks use when *not* overridden (as
/// exercised, with overrides, by the test above) — closing the loop between
/// "what the allowlist says is allowed" and "what production actually
/// targets by default".
#[test]
fn production_defaults_match_allowlisted_hosts() {
    use xai_grok_telemetry::allowlist::{ALLOWED_TELEMETRY_HOSTS, is_allowed_telemetry_host};

    let mixpanel_host = url::Url::parse(xai_mixpanel::DEFAULT_BASE_URL)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned();
    assert!(
        is_allowed_telemetry_host(&mixpanel_host),
        "production Mixpanel default host {mixpanel_host:?} must be allowlisted"
    );

    // The default product-events / first-party OTLP host is documented
    // (docs/privacy/network-destinations.md, docs/privacy/telemetry.md) as
    // `cli-chat-proxy.grok.com`; it's not a compiled-in literal in this
    // crate (comes from build-time/env config), so we assert the allowlist
    // itself carries it rather than re-deriving it from config.
    assert!(ALLOWED_TELEMETRY_HOSTS.contains(&"cli-chat-proxy.grok.com"));
}
