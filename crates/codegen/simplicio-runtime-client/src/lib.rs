//! Fail-closed MCP client used by Simplicio Code for project operations.
//!
//! The client deliberately contains no local filesystem or shell fallback. A
//! Runtime connection is negotiated with `initialize` and `tools/list`; each
//! operation checks its required capability immediately before sending the
//! request. This lets older Runtimes keep the already-supported read path
//! while rejecting newer operations with an actionable incompatibility error.

pub mod map_cache;

pub use map_cache::{
    MAP_RESULT_SCHEMA_V1, MapCache, MapResult, MapState, budgeted_summary, compute_repo_hash,
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    io::{BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{LazyLock, Mutex},
};

pub const MCP_CONTRACT_SCHEMA: &str = "simplicio.code-mcp/v1";
const PROTOCOL_VERSION: &str = "2024-11-05";
const EXPECTED_SERVER: &str = "simplicio";
pub const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_EXEC_TIMEOUT_MS: u64 = 120_000;
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
    #[error("Simplicio Runtime rejected the operation: {0}")]
    OperationRejected(String),
    #[error(
        "Simplicio Runtime capability '{operation}' is unavailable; required tool '{required}', available: {available}"
    )]
    CapabilityMismatch {
        operation: String,
        required: String,
        available: String,
    },
    #[error("workspace path rejected: {0}")]
    PathRejected(String),
    #[error("unsafe command rejected: {0}")]
    ExecRejected(String),
    #[error("Simplicio Runtime rejected the file read: {0}")]
    ReadRejected(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub schema: String,
    pub protocol_version: String,
    pub server_name: String,
    #[serde(default)]
    pub server_version: Option<String>,
    pub tools: BTreeSet<String>,
}

impl RuntimeCapabilities {
    pub fn supports(&self, tool: &str) -> bool {
        self.tools.contains(tool)
    }

    fn available(&self) -> String {
        self.tools.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Starts the Runtime-owned repository map once per workspace and process.
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
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub bytes_base64: Option<String>,
}

impl FileReadResult {
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        if self.encoding.as_deref() == Some("base64") {
            return base64::engine::general_purpose::STANDARD
                .decode(self.bytes_base64.as_deref().unwrap_or(&self.content))
                .map_err(|e| Error::InvalidResponse(format!("invalid base64 read: {e}")));
        }
        Ok(self.content.as_bytes().to_vec())
    }
}

pub struct RuntimeClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    capabilities: RuntimeCapabilities,
}

impl RuntimeClient {
    pub fn spawn() -> Result<Self, Error> {
        let cwd = std::env::current_dir()?;
        Self::spawn_in(&cwd)
    }

    pub fn spawn_in(workspace: &Path) -> Result<Self, Error> {
        let workspace = canonical_repo(workspace)?;
        let binary = resolve_binary()?;
        let mut child = Command::new(&binary)
            .args(["serve", "--mcp", "--stdio", "--json"])
            .current_dir(&workspace)
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
            capabilities: RuntimeCapabilities {
                schema: MCP_CONTRACT_SCHEMA.into(),
                protocol_version: String::new(),
                server_name: String::new(),
                server_version: None,
                tools: BTreeSet::new(),
            },
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
        client.capabilities =
            parse_capabilities(&initialized, &client.request("tools/list", json!({}))?)?;
        Ok(client)
    }

    pub fn capabilities(&self) -> &RuntimeCapabilities {
        &self.capabilities
    }

    pub fn read_file(
        &mut self,
        repo: &Path,
        path: &Path,
        max_bytes: usize,
    ) -> Result<FileReadResult, Error> {
        self.read_file_range(repo, path, None, None, max_bytes)
    }

    pub fn read_file_range(
        &mut self,
        repo: &Path,
        path: &Path,
        start: Option<u64>,
        end: Option<u64>,
        max_bytes: usize,
    ) -> Result<FileReadResult, Error> {
        let repo = canonical_repo(repo)?;
        let relative = secure_relative_path(&repo, path)?;
        let mut args = json!({
            "repo": repo,
            "path": relative,
            "max_bytes": max_bytes.min(DEFAULT_MAX_FILE_BYTES),
        });
        if let Some(start) = start {
            args["start"] = json!(start);
        }
        if let Some(end) = end {
            args["end"] = json!(end);
        }
        let result = self.call_tool("read", "simplicio_file_read", args)?;
        parse_file_read(&tool_text_or_json(&result))
    }

    pub fn search(
        &mut self,
        repo: &Path,
        pattern: &str,
        globs: &[String],
        case_insensitive: bool,
        literal: bool,
        max_files: usize,
        max_matches: usize,
    ) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        self.call_tool(
            "search",
            "simplicio_search",
            json!({
                "repo": repo,
                "query": pattern,
                "pattern": pattern,
                "globs": globs,
                "case_insensitive": case_insensitive,
                "literal": literal,
                "max_files": max_files,
                "max_matches": max_matches,
            }),
        )
    }

    pub fn list(&mut self, repo: &Path, path: &Path, options: Value) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        let path = secure_relative_path(&repo, path)?;
        self.call_tool(
            "list",
            "simplicio_fs_list",
            json!({ "repo": repo, "path": path, "options": options }),
        )
    }

    pub fn stat(&mut self, repo: &Path, path: &Path) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        let path = secure_relative_path(&repo, path)?;
        self.call_tool(
            "stat",
            "simplicio_fs_stat",
            json!({ "repo": repo, "path": path }),
        )
    }

    pub fn edit(&mut self, repo: &Path, plan: Value) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        validate_plan_paths(&repo, &plan)?;
        self.call_tool(
            "edit",
            "simplicio_edit",
            json!({ "repo": repo, "plan": serde_json::to_string(&plan).map_err(|e| Error::InvalidResponse(e.to_string()))?, "atomic": true, "rollback": true }),
        )
    }

    pub fn write_file(&mut self, repo: &Path, path: &Path, data: &[u8]) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        let path = secure_relative_path(&repo, path)?;
        self.call_tool(
            "write",
            "simplicio_fs_write",
            json!({
                "repo": repo,
                "path": path,
                "content_base64": base64::engine::general_purpose::STANDARD.encode(data),
                "encoding": "base64",
                "atomic": true,
                "rollback": true,
            }),
        )
    }

    pub fn delete_file(&mut self, repo: &Path, path: &Path) -> Result<Value, Error> {
        let repo = canonical_repo(repo)?;
        let path = secure_relative_path(&repo, path)?;
        self.call_tool(
            "delete",
            "simplicio_fs_delete",
            json!({ "repo": repo, "path": path, "atomic": true, "rollback": true }),
        )
    }

    pub fn exec(
        &mut self,
        repo: &Path,
        cwd: &Path,
        argv: &[String],
        timeout_ms: u64,
        max_output_bytes: usize,
    ) -> Result<Value, Error> {
        if argv.is_empty() {
            return Err(Error::ExecRejected("argv must not be empty".into()));
        }
        if argv.iter().any(|arg| contains_shell_metacharacters(arg)) {
            return Err(Error::ExecRejected(
                "shell metacharacters are not allowed".into(),
            ));
        }
        let repo = canonical_repo(repo)?;
        let cwd = secure_relative_path(&repo, cwd)?;
        self.call_tool(
            "exec",
            "simplicio_exec",
            json!({
                "repo": repo,
                "argv": argv,
                "cwd": cwd,
                "timeout_ms": timeout_ms.min(DEFAULT_EXEC_TIMEOUT_MS),
                "max_output_bytes": max_output_bytes.min(DEFAULT_MAX_OUTPUT_BYTES),
                "shell": false,
            }),
        )
    }

    fn call_tool(&mut self, operation: &str, tool: &str, args: Value) -> Result<Value, Error> {
        if !self.capabilities.supports(tool) {
            return Err(Error::CapabilityMismatch {
                operation: operation.into(),
                required: tool.into(),
                available: self.capabilities.available(),
            });
        }
        let result = self.request("tools/call", json!({ "name": tool, "arguments": args }))?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(if operation == "read" {
                Error::ReadRejected(tool_text(&result))
            } else {
                Error::OperationRejected(tool_text(&result))
            });
        }
        Ok(result)
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

fn parse_capabilities(initialized: &Value, listed: &Value) -> Result<RuntimeCapabilities, Error> {
    let tools = listed
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidResponse("tools/list missing tools array".into()))?
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    Ok(RuntimeCapabilities {
        schema: MCP_CONTRACT_SCHEMA.into(),
        protocol_version: initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION)
            .into(),
        server_name: initialized
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .unwrap_or(EXPECTED_SERVER)
            .into(),
        server_version: initialized
            .pointer("/serverInfo/version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools,
    })
}

fn canonical_repo(repo: &Path) -> Result<PathBuf, Error> {
    repo.canonicalize()
        .map_err(|e| Error::PathRejected(format!("repository {}: {e}", repo.display())))
}

fn secure_relative_path(repo: &Path, path: &Path) -> Result<String, Error> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(Error::PathRejected(format!(
            "parent traversal: {}",
            path.display()
        )));
    }
    // Canonicalize the repo root itself: callers that skip `canonical_repo`
    // (or pass a root whose on-disk representation differs from its
    // canonical form, e.g. a symlinked temp dir) would otherwise compare a
    // canonicalized candidate against a non-canonical root and reject
    // legitimate in-repo paths. Falls back to the given root when it can't
    // be canonicalized (e.g. it doesn't exist yet); containment is checked
    // against whichever form both sides agree on below.
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    };
    let canonical = if candidate.exists() {
        candidate.canonicalize()
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| Error::PathRejected(format!("missing parent: {}", candidate.display())))?
            .canonicalize();
        parent.map(|parent| parent.join(candidate.file_name().unwrap_or_default()))
    }
    .map_err(|e| Error::PathRejected(format!("{}: {e}", path.display())))?;
    if !canonical.starts_with(&repo) {
        return Err(Error::PathRejected(format!(
            "path escapes repository: {}",
            path.display()
        )));
    }
    canonical
        .strip_prefix(repo)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            Error::PathRejected(format!(
                "path is not repository-relative: {}",
                path.display()
            ))
        })
}

fn validate_plan_paths(repo: &Path, plan: &Value) -> Result<(), Error> {
    let files = plan
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            plan.as_object()
                .into_iter()
                .filter_map(|object| object.get("file")),
        )
        .collect::<Vec<_>>();
    for file in files.into_iter().filter_map(Value::as_str) {
        secure_relative_path(repo, Path::new(file))?;
    }
    Ok(())
}

fn contains_shell_metacharacters(arg: &str) -> bool {
    arg.chars()
        .any(|c| matches!(c, '|' | ';' | '&' | '`' | '$' | '>' | '<' | '\n' | '\r'))
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

fn tool_text_or_json(result: &Value) -> String {
    let text = tool_text(result);
    if text.is_empty() {
        result.to_string()
    } else {
        text
    }
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
    use std::fs;

    #[test]
    fn parses_runtime_capabilities_contract() {
        let capabilities = parse_capabilities(
            &json!({"protocolVersion": PROTOCOL_VERSION, "serverInfo": {"name": "simplicio", "version": "3.5.2"}}),
            &json!({"tools": [{"name": "simplicio_file_read"}, {"name": "simplicio_exec"}]}),
        )
        .unwrap();
        assert_eq!(capabilities.schema, MCP_CONTRACT_SCHEMA);
        assert!(capabilities.supports("simplicio_file_read"));
        assert!(!capabilities.supports("simplicio_fs_write"));
    }

    #[test]
    fn secure_path_rejects_parent_traversal_and_external_absolute_paths() {
        let repo = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("inside.txt"), "ok").unwrap();
        assert!(secure_relative_path(repo.path(), Path::new("../outside.txt")).is_err());
        let outside = repo.path().parent().unwrap().join("outside.txt");
        assert!(secure_relative_path(repo.path(), &outside).is_err());
        assert_eq!(
            secure_relative_path(repo.path(), Path::new("inside.txt")).unwrap(),
            "inside.txt"
        );
    }

    #[test]
    fn rejects_shell_injection_before_mcp_request() {
        assert!(contains_shell_metacharacters("cargo test; whoami"));
        assert!(contains_shell_metacharacters("echo $HOME"));
        assert!(!contains_shell_metacharacters("cargo test -p crate"));
    }

    #[test]
    fn parses_runtime_file_read_contract_and_binary_payload() {
        let result = parse_file_read(r#"{"schema":"simplicio.read-result/v1","path":"bin","content":"aGk=","encoding":"base64","bytes_base64":"aGk=","truncated":false}"#).unwrap();
        assert_eq!(result.bytes().unwrap(), b"hi");
    }

    #[test]
    fn rejects_truncated_reads_instead_of_returning_partial_source() {
        let error = parse_file_read(r#"{"schema":"simplicio.read-result/v1","path":"large","content":"partial","truncated":true}"#).unwrap_err();
        assert!(matches!(error, Error::ReadRejected(_)));
    }

    #[test]
    #[ignore = "requires an installed Simplicio Runtime with MCP tools/list"]
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
