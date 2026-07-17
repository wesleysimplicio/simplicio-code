//! Fail-closed MCP client used by Simplicio Code for project file reads.

use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{LazyLock, Mutex},
};

const PROTOCOL_VERSION: &str = "2024-11-05";
const EXPECTED_SERVER: &str = "simplicio";
pub const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
static MAPPED_WORKSPACES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Simplicio Runtime executable was not found; install it or set SIMPLICIO_BIN")]
    RuntimeNotFound,
    #[error("failed to start Simplicio Runtime: {0}")]
    Spawn(String),
    #[error("Simplicio Runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Simplicio Runtime protocol error: {0}")]
    Protocol(String),
    #[error("invalid Simplicio Runtime response: {0}")]
    InvalidResponse(String),
    #[error("unexpected MCP server '{0}'; expected Simplicio Runtime")]
    IdentityMismatch(String),
    #[error("Simplicio Runtime rejected the file read: {0}")]
    ReadRejected(String),
}

/// Starts the Runtime-owned repository map once per workspace and process.
///
/// Mapping runs in the background so selecting a large folder never blocks the UI.
pub fn start_workspace_map(workspace: &Path) -> Result<bool, Error> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let binary = resolve_binary()?;
    let mut mapped = MAPPED_WORKSPACES
        .lock()
        .map_err(|_| Error::Spawn("workspace map lock poisoned".into()))?;
    if !mapped.insert(workspace.clone()) {
        return Ok(false);
    }
    drop(mapped);

    let child = Command::new(&binary)
        .args(["runtime", "map", "--json", "--repo"])
        .arg(&workspace)
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            if let Ok(mut mapped) = MAPPED_WORKSPACES.lock() {
                mapped.remove(&workspace);
            }
            return Err(Error::Spawn(format!("{}: {error}", binary.display())));
        }
    };
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(true)
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct FileReadResult {
    pub schema: String,
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
}

pub struct RuntimeClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl RuntimeClient {
    pub fn spawn() -> Result<Self, Error> {
        let cwd = std::env::current_dir()?;
        Self::spawn_in(&cwd)
    }

    pub fn spawn_in(workspace: &Path) -> Result<Self, Error> {
        let binary = resolve_binary()?;
        let mut child = Command::new(&binary)
            .args(["serve", "--mcp", "--stdio", "--json"])
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| Error::Spawn(format!("{}: {error}", binary.display())))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Spawn("stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Spawn("stdout unavailable".into()))?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        let initialized = client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "simplicio-code", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        let server = initialized
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if !server.eq_ignore_ascii_case(EXPECTED_SERVER) {
            return Err(Error::IdentityMismatch(server.to_owned()));
        }
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    pub fn read_file(
        &mut self,
        repo: &Path,
        path: &Path,
        max_bytes: usize,
    ) -> Result<FileReadResult, Error> {
        let runtime_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            repo.join(path)
        };
        let result = self.request(
            "tools/call",
            json!({
                "name": "simplicio_file_read",
                "arguments": {
                    "repo": repo.to_string_lossy(),
                    "path": runtime_path.to_string_lossy(),
                    "max_bytes": max_bytes
                }
            }),
        )?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(Error::ReadRejected(tool_text(&result)));
        }
        parse_file_read(&tool_text(&result))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        if line.is_empty() {
            return Err(Error::Protocol("runtime closed stdout".into()));
        }
        let response: Value =
            serde_json::from_str(&line).map_err(|e| Error::InvalidResponse(e.to_string()))?;
        if let Some(error) = response.get("error") {
            return Err(Error::Protocol(error.to_string()));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("missing result".into()))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), Error> {
        self.send(&json!({"jsonrpc":"2.0", "method":method, "params":params}))
    }

    fn send(&mut self, message: &Value) -> Result<(), Error> {
        serde_json::to_writer(&mut self.stdin, message)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for RuntimeClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn resolve_binary() -> Result<PathBuf, Error> {
    if let Some(path) = std::env::var_os("SIMPLICIO_BIN").map(PathBuf::from) {
        return path.is_file().then_some(path).ok_or(Error::RuntimeNotFound);
    }
    let Some(path) = std::env::var_os("PATH") else {
        return Err(Error::RuntimeNotFound);
    };
    std::env::split_paths(&path)
        .map(|dir| {
            dir.join(if cfg!(windows) {
                "simplicio.exe"
            } else {
                "simplicio"
            })
        })
        .find(|candidate| candidate.is_file())
        .ok_or(Error::RuntimeNotFound)
}

fn tool_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_file_read(text: &str) -> Result<FileReadResult, Error> {
    let result: FileReadResult =
        serde_json::from_str(text).map_err(|e| Error::InvalidResponse(e.to_string()))?;
    if result.schema != "simplicio.read-result/v1" {
        return Err(Error::InvalidResponse(format!(
            "unsupported schema {}",
            result.schema
        )));
    }
    if result.truncated {
        return Err(Error::ReadRejected(
            "file exceeded the Runtime read limit".into(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_file_read_contract() {
        let result = parse_file_read(r#"{"schema":"simplicio.read-result/v1","path":"src/main.rs","content":"fn main() {}","truncated":false}"#).unwrap();
        assert_eq!(result.content, "fn main() {}");
    }

    #[test]
    fn rejects_truncated_reads_instead_of_returning_partial_source() {
        let error = parse_file_read(r#"{"schema":"simplicio.read-result/v1","path":"large","content":"partial","truncated":true}"#).unwrap_err();
        assert!(matches!(error, Error::ReadRejected(_)));
    }

    #[test]
    #[ignore = "requires an installed Simplicio Runtime"]
    fn reads_a_real_file_through_runtime_mcp() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .unwrap();
        let mut client = RuntimeClient::spawn_in(&repo).unwrap();
        let result = client
            .read_file(&repo, Path::new("SOURCE_REV"), DEFAULT_MAX_FILE_BYTES)
            .unwrap();
        assert_eq!(result.schema, "simplicio.read-result/v1");
        assert!(!result.content.trim().is_empty());
    }
}
