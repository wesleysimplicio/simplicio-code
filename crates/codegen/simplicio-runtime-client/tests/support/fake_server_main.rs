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

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
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
