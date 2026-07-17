//! Regression test for issue #38: a Runtime that misbehaves during the
//! `initialize` handshake must fail fast with an actionable diagnostic,
//! instead of hanging for tens of seconds and returning a bare parse error.
//!
//! Uses the fake stdio server's `SIMPLICIO_FAKE_SERVER_MODE` switch (see
//! `tests/support/fake_server_main.rs`) so both failure modes are reproduced
//! deterministically, without depending on a real broken Runtime binary.
//!
//! Both scenarios live in one `#[test]` function for the same reason as
//! `fake_server_search.rs`: `SIMPLICIO_BIN`/`SIMPLICIO_FAKE_SERVER_MODE` are
//! process-wide, and `cargo test` runs `#[test]` functions in the same
//! binary concurrently by default, so splitting these into separate test
//! functions would race on those env vars.

use std::time::Instant;

use simplicio_runtime_client::{DEFAULT_HANDSHAKE_TIMEOUT_MS, Error, RuntimeClient};

#[test]
fn misbehaving_handshake_fails_fast_with_actionable_diagnostics() {
    let fake_server = env!("CARGO_BIN_EXE_simplicio-fake-runtime-server");
    let repo = tempfile::tempdir().unwrap();

    // --- scenario 1: non-JSON-RPC banner in place of the `initialize` reply ---
    // SAFETY: this file's only test function; no concurrent reader/writer of
    // these env vars exists within this process (see module doc comment).
    unsafe {
        std::env::set_var("SIMPLICIO_BIN", fake_server);
        std::env::set_var("SIMPLICIO_FAKE_SERVER_MODE", "banner_then_json");
    }
    let started = Instant::now();
    let result = RuntimeClient::spawn_in(repo.path());
    let elapsed = started.elapsed();

    let error = result
        .err()
        .expect("a non-JSON-RPC handshake response must not succeed");
    assert!(
        elapsed.as_millis() < 2_000,
        "malformed handshake must fail well under the old multi-second-to-30s range, took {elapsed:?}"
    );
    match &error {
        Error::InvalidResponse(message) => {
            assert!(
                message.contains("first bytes:"),
                "diagnostic must include a raw-bytes snippet, got: {message}"
            );
            assert!(
                message.contains("Simplicio Runtime booting"),
                "diagnostic must surface what was actually received, got: {message}"
            );
        }
        other => panic!("expected Error::InvalidResponse with a raw snippet, got: {other:?}"),
    }

    // --- scenario 2: Runtime never answers `initialize` at all ---
    unsafe {
        std::env::set_var("SIMPLICIO_FAKE_SERVER_MODE", "hang_handshake");
    }
    let started = Instant::now();
    let result = RuntimeClient::spawn_in(repo.path());
    let elapsed = started.elapsed();

    unsafe {
        std::env::remove_var("SIMPLICIO_FAKE_SERVER_MODE");
        std::env::remove_var("SIMPLICIO_BIN");
    }

    let error = result
        .err()
        .expect("a Runtime that never answers initialize must not succeed");
    assert!(
        elapsed.as_millis() < DEFAULT_HANDSHAKE_TIMEOUT_MS as u128 + 1_500,
        "handshake timeout must fire close to DEFAULT_HANDSHAKE_TIMEOUT_MS, took {elapsed:?}"
    );
    assert!(
        matches!(error, Error::HandshakeTimeout { .. }),
        "expected Error::HandshakeTimeout, got: {error:?}"
    );
}
