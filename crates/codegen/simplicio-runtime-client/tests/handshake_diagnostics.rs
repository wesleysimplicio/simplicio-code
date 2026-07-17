//! Regression tests for issue #38: `RuntimeClient` must fail fast (not
//! multi-second-to-30s) with an actionable, redacted diagnostic when the
//! Runtime MCP handshake breaks, instead of the previous bare
//! `serde_json` parse error after a long hang.
//!
//! Both tests drive `RuntimeClient::spawn_in` against the fake stdio server
//! in `tests/support/fake_server_main.rs`, switched into a misbehaving mode
//! via `FAKE_RUNTIME_MODE` (see that file's doc comment) — no real,
//! installed Simplicio Runtime binary or network access is required, so
//! this reproduces both failure shapes deterministically in CI.
//!
//! Like `tests/fake_server_search.rs`, both assertions live in one
//! `#[test]` function: `SIMPLICIO_BIN`/`FAKE_RUNTIME_MODE` are process-wide
//! env vars and `cargo test` runs tests in the same binary on multiple
//! threads by default, so spreading these across separate `#[test]`
//! functions in this file would race on them. Each integration test *file*
//! is still its own process, so this doesn't affect (or get affected by)
//! tests in other files/crates.

use std::time::{Duration, Instant};

use simplicio_runtime_client::{Error, RuntimeClient};

#[test]
fn fails_fast_with_redacted_snippet_on_malformed_handshake_and_on_a_hung_handshake() {
    let fake_server = env!("CARGO_BIN_EXE_simplicio-fake-runtime-server");
    let repo = tempfile::tempdir().unwrap();

    // --- malformed handshake response: fails fast with a redacted snippet ---
    // SAFETY: this file's only test function; no concurrent reader/writer of
    // these env vars exists within this process (see module doc comment).
    unsafe {
        std::env::set_var("SIMPLICIO_BIN", fake_server);
        std::env::set_var("FAKE_RUNTIME_MODE", "malformed_handshake");
    }
    let started = Instant::now();
    let result = RuntimeClient::spawn_in(repo.path());
    let elapsed = started.elapsed();
    unsafe {
        std::env::remove_var("FAKE_RUNTIME_MODE");
    }

    assert!(
        elapsed < Duration::from_secs(3),
        "malformed handshake response should fail fast (well under the previous \
         multi-second-to-30s hang), took {elapsed:?}"
    );
    let error = match result {
        Ok(_) => panic!("a non-JSON-RPC handshake response must be rejected"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::InvalidResponse(_)),
        "expected Error::InvalidResponse for a malformed handshake response, got: {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("Simplicio Runtime starting up"),
        "diagnostic should include a snippet of what was actually received, got: {message}"
    );
    assert!(
        !message.contains("testuser"),
        "diagnostic must redact the absolute path (and the username in it), got: {message}"
    );
    assert!(
        message.contains("<REDACTED>"),
        "diagnostic should mark the redacted path, got: {message}"
    );

    // --- handshake that never responds: bounded HANDSHAKE_TIMEOUT fires ---
    unsafe {
        std::env::set_var("FAKE_RUNTIME_MODE", "hangs_forever");
    }
    let started = Instant::now();
    let result = RuntimeClient::spawn_in(repo.path());
    let elapsed = started.elapsed();
    unsafe {
        std::env::remove_var("SIMPLICIO_BIN");
        std::env::remove_var("FAKE_RUNTIME_MODE");
    }

    assert!(
        elapsed < Duration::from_secs(3),
        "a hung handshake should time out in ~2s (HANDSHAKE_TIMEOUT), not the previous \
         multi-second-to-30s range, took {elapsed:?}"
    );
    let error = match result {
        Ok(_) => panic!("a handshake that never responds must be rejected"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::HandshakeTimeout { .. }),
        "expected Error::HandshakeTimeout for a hung handshake, got: {error:?}"
    );
}
