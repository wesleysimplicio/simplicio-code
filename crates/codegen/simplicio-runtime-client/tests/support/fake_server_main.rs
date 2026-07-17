//! Minimal stdio JSON-RPC server standing in for a real Simplicio Runtime.
//!
//! Speaks just enough of the MCP subset `RuntimeClient` uses (`initialize`,
//! `notifications/initialized`, `tools/list`, `tools/call`) to exercise
//! `RuntimeClient::search`/`read_file`/`write_file`/`delete_file` end to end
//! without depending on a real Runtime binary being installed. Pointed to via
//! `SIMPLICIO_BIN` in `tests/fake_server_search.rs`.
//!
//! Deliberately tiny: one line of JSON in, one line of JSON out, matching
//! `RuntimeClient::request`'s framing (`serde_json::to_writer` + `\n`).
//!
//! `FAKE_RUNTIME_MODE` switches this into one of two misbehaving modes used
//! by `tests/handshake_diagnostics.rs` (issue #38) to reproduce, without any
//! real Runtime binary, the two failure shapes simplicio-runtime#3319
//! exposed: a malformed (non-JSON-RPC) handshake response, and a handshake
//! that never responds at all.
//!
//! - `malformed_handshake`: writes one line of non-JSON-RPC text (as if the
//!   Runtime process printed a startup banner directly to the MCP stdout
//!   channel, or emitted corrupted output) in reply to `initialize`, then
//!   exits. `RuntimeClient::request_timed`'s `serde_json::from_slice` fails
//!   to parse it, exercising the redacted-snippet diagnostic.
//! - `hangs_forever`: never writes anything back and never exits on its own,
//!   simulating a Runtime process stuck mid-handshake. Exercises
//!   `HANDSHAKE_TIMEOUT`/`Error::HandshakeTimeout`. The test that uses this
//!   mode kills the child itself (via `RuntimeClient`'s `Drop`); this mode
//!   never terminates by itself.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    match std::env::var("FAKE_RUNTIME_MODE").as_deref() {
        Ok("malformed_handshake") => {
            // Simulates a corrupted/misbehaving Runtime response: a single
            // line of plain text (not JSON-RPC) containing something that
            // looks like an absolute path, so the test can also assert the
            // diagnostic redacts it. Written promptly (no sleep) since the
            // point of this mode is the *parse* failure, not a timeout.
            let mut stdout = io::stdout();
            let _ = stdout.write_all(
                b"Simplicio Runtime starting up at /home/testuser/.simplicio/cache banner\n",
            );
            let _ = stdout.flush();
            return;
        }
        Ok("hangs_forever") => {
            // Simulates a Runtime process stuck mid-handshake: never writes
            // a response and never exits on its own. `RuntimeClient` kills
            // this process on `Drop`, so parking here is safe for the test.
            loop {
                std::thread::park();
            }
        }
        _ => {}
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();

        match method {
            "initialize" => write_response(
                &mut stdout,
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": { "name": "simplicio", "version": "fake-test-server" }
                }),
            ),
            "notifications/initialized" => {
                // Notifications carry no `id` and expect no response.
            }
            "tools/list" => write_response(
                &mut stdout,
                id,
                json!({
                    "tools": [
                        { "name": "simplicio_file_read" },
                        { "name": "simplicio_fs_write" },
                        { "name": "simplicio_fs_delete" },
                        { "name": "simplicio_search" },
                    ]
                }),
            ),
            "tools/call" => {
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                write_response(&mut stdout, id, handle_tool_call(name, &arguments));
            }
            _ => {}
        }
    }
}

/// Builds the `tools/call` `result` object (`isError` + `content[].text`,
/// matching the shape `RuntimeClient::call_tool`/`tool_text` expect).
fn handle_tool_call(name: &str, arguments: &Value) -> Value {
    match name {
        "simplicio_search" => {
            let path = arguments.get("path").and_then(Value::as_str).unwrap_or("");
            let scope = if path.is_empty() { "." } else { path };
            let body = json!({
                "schema": "simplicio.search-result/v1",
                "matches": [
                    { "path": format!("{scope}/fake_match.rs"), "line": 1, "text": "fn fake_match() {}" }
                ],
                "truncated": false,
            });
            tool_ok(body.to_string())
        }
        "simplicio_fs_write" | "simplicio_fs_delete" => tool_ok("{}".to_owned()),
        "simplicio_file_read" => {
            let path = arguments.get("path").and_then(Value::as_str).unwrap_or("");
            let body = json!({
                "schema": "simplicio.read-result/v1",
                "path": path,
                "content": "fake content",
                "truncated": false,
            });
            tool_ok(body.to_string())
        }
        other => json!({
            "isError": true,
            "content": [{ "type": "text", "text": format!("fake server: unknown tool '{other}'") }],
        }),
    }
}

fn tool_ok(text: String) -> Value {
    json!({
        "isError": false,
        "content": [{ "type": "text", "text": text }],
    })
}

fn write_response(stdout: &mut impl Write, id: Option<Value>, result: Value) {
    let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    if serde_json::to_writer(&mut *stdout, &response).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}
