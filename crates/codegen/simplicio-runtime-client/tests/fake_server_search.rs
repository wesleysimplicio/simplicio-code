//! End-to-end test of `RuntimeClient` against the fake stdio MCP server in
//! `tests/fake_server/main.rs`, run in place of a real Simplicio Runtime
//! binary. No real Runtime install exists in CI/dev sandboxes, so this is the
//! only way to exercise the full `initialize` -> `tools/list` ->
//! `tools/call` round trip (capability negotiation included) for `search`,
//! and to regression-test that `read`/`write`/`delete` still work the same
//! way through the identical `RuntimeClient` after the `search` hardening
//! added alongside it.
//!
//! All assertions live in one `#[test]` function: `SIMPLICIO_BIN` is a
//! process-wide environment variable, and `cargo test` runs tests in the same
//! binary on multiple threads by default, so spreading these across several
//! `#[test]` functions in this file would race on that env var. Each
//! integration test *file* is still its own process, so this does not affect
//! (or get affected by) tests in other files/crates.

use std::{collections::BTreeMap, path::Path};

use simplicio_runtime_client::{Error, RuntimeClient};

fn tool_payload(result: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn fake_runtime_round_trips_search_and_keeps_read_write_delete_fail_closed_contract() {
    let fake_server = env!("CARGO_BIN_EXE_simplicio-fake-runtime-server");
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("existing.txt"), "hello").unwrap();

    // SAFETY: this file's only test function; no concurrent reader/writer of
    // `SIMPLICIO_BIN` exists within this process (see module doc comment).
    unsafe {
        std::env::set_var("SIMPLICIO_BIN", fake_server);
    }

    let mut client = RuntimeClient::spawn_in(repo.path()).expect("fake runtime should spawn");

    // --- search: capability negotiation + typed contract parsing ---
    let result = client
        .search(
            repo.path(),
            "fake_match",
            Some(Path::new("src")),
            &["*.rs".to_owned()],
            false,
            false,
            100,
            100,
        )
        .expect("search should round-trip through the fake runtime");
    assert_eq!(result.schema, "simplicio.search-result/v1");
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].path, "src/fake_match.rs");
    assert!(!result.truncated);

    let list = client
        .list(repo.path(), Path::new("."), serde_json::json!({"depth": 1}))
        .unwrap();
    assert_eq!(tool_payload(&list)["schema"], "simplicio.fs-list-result/v1");
    let stat = client.stat(repo.path(), Path::new("existing.txt")).unwrap();
    assert_eq!(tool_payload(&stat)["path"], "existing.txt");

    let argv = vec!["printf".to_owned(), "hello".to_owned()];
    let first = client
        .exec(
            repo.path(),
            Path::new("."),
            &argv,
            &BTreeMap::new(),
            1_000,
            4096,
            "issue-108-replay",
        )
        .unwrap();
    let replay = client
        .exec(
            repo.path(),
            Path::new("."),
            &argv,
            &BTreeMap::new(),
            1_000,
            4096,
            "issue-108-replay",
        )
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(tool_payload(&first)["effect"], "committed");
    let shell = client.exec(
        repo.path(),
        Path::new("."),
        &["printf hello | cat".to_owned()],
        &BTreeMap::new(),
        1_000,
        4096,
        "unsafe-shell",
    );
    assert!(matches!(shell, Err(Error::ExecRejected(_))));
    let failure = client.exec(
        repo.path(),
        Path::new("."),
        &["__fail__".to_owned()],
        &BTreeMap::new(),
        1_000,
        4096,
        "injected-failure",
    );
    assert!(
        matches!(failure, Err(Error::OperationRejected(message)) if message.contains("injected exec failure"))
    );

    // --- search fails closed on a path-escape attempt, same as read/write/delete ---
    let escape = client.search(
        repo.path(),
        "pattern",
        Some(Path::new("../outside")),
        &[],
        false,
        false,
        10,
        10,
    );
    assert!(
        matches!(escape, Err(Error::PathRejected(_))),
        "search must reject a path scope that escapes the repo, got: {escape:?}"
    );

    // --- search fails closed on a glob-escape attempt ---
    let bad_glob = client.search(
        repo.path(),
        "pattern",
        None,
        &["../../etc/*".to_owned()],
        false,
        false,
        10,
        10,
    );
    assert!(
        matches!(bad_glob, Err(Error::GlobRejected(_))),
        "search must reject a glob that attempts parent traversal, got: {bad_glob:?}"
    );

    // --- regression: read/write/delete still work through the same client/session ---
    let read = client
        .read_file(repo.path(), Path::new("existing.txt"), 4096)
        .expect("read should still round-trip");
    assert_eq!(read.schema, "simplicio.read-result/v1");

    client
        .write_file(repo.path(), Path::new("new.txt"), b"data")
        .expect("write should still round-trip");

    client
        .delete_file(repo.path(), Path::new("existing.txt"))
        .expect("delete should still round-trip");

    // --- edit: the atomic patch consumer uses the existing Runtime contract ---
    let edit = client
        .edit(
            repo.path(),
            serde_json::json!({
                "files": [{
                    "file": "new.txt",
                    "operation": "update",
                    "content": "updated"
                }]
            }),
        )
        .expect("edit should round-trip through the fake Runtime");
    let edit = tool_payload(&edit);
    assert_eq!(edit["schema"], "simplicio.edit-result/v1");
    assert_eq!(edit["plan"]["files"][0]["file"], "new.txt");

    // --- regression: read/write/delete still fail closed on path escape ---
    let read_escape = client.read_file(repo.path(), Path::new("../outside.txt"), 4096);
    assert!(matches!(read_escape, Err(Error::PathRejected(_))));
    let write_escape = client.write_file(repo.path(), Path::new("../outside.txt"), b"x");
    assert!(matches!(write_escape, Err(Error::PathRejected(_))));
    let delete_escape = client.delete_file(repo.path(), Path::new("../outside.txt"));
    assert!(matches!(delete_escape, Err(Error::PathRejected(_))));
    let edit_escape = client.edit(
        repo.path(),
        serde_json::json!({
            "files": [{
                "file": "inside.txt",
                "operation": "move",
                "move_to": "../outside.txt"
            }]
        }),
    );
    assert!(matches!(edit_escape, Err(Error::PathRejected(_))));

    drop(client);
    // SAFETY: same single-test-function invariant as the `set_var` above.
    unsafe {
        std::env::remove_var("SIMPLICIO_BIN");
    }
}
