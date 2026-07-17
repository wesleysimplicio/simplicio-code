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
    io::{self, BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        LazyLock, Mutex,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

pub const MCP_CONTRACT_SCHEMA: &str = "simplicio.code-mcp/v1";
const PROTOCOL_VERSION: &str = "2024-11-05";
const EXPECTED_SERVER: &str = "simplicio";
pub const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_EXEC_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MAX_SEARCH_FILES: usize = 2_000;
pub const DEFAULT_MAX_SEARCH_MATCHES: usize = 10_000;
/// Bounded timeout for the `initialize`/`tools/list` startup handshake,
/// distinct from any later per-call timeout (e.g. `exec`'s `timeout_ms`).
/// A healthy local Runtime process answers this in milliseconds; a broken
/// or hung handshake (simplicio-runtime#3319) previously surfaced as a
/// multi-second-to-30s hang before a low-signal parse error. Bounding it
/// here makes that failure mode fail fast and diagnosably instead.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum number of raw bytes from a malformed handshake response echoed
/// back in [`Error::InvalidResponse`]/[`Error::Protocol`] diagnostics.
/// Bounded so a huge or binary response can't blow up an error message.
const DIAGNOSTIC_SNIPPET_BYTES: usize = 200;
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
    #[error(
        "Simplicio Runtime handshake ('{method}') timed out after {timeout:?}; the process may be hung, stuck negotiating, or emitting a malformed/never-terminated response instead of replying (see simplicio-runtime#3319 for one known server-side cause)"
    )]
    HandshakeTimeout { method: String, timeout: Duration },
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
    #[error("search glob rejected: {0}")]
    GlobRejected(String),
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

/// A single match returned by [`RuntimeClient::search`].
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SearchMatch {
    /// Repo-relative path (forward-slash separated), as returned by the Runtime.
    pub path: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub text: String,
}

/// Typed response contract for `simplicio_search` (`simplicio.search-result/v1`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub schema: String,
    #[serde(default)]
    pub matches: Vec<SearchMatch>,
    #[serde(default)]
    pub truncated: bool,
}

pub struct RuntimeClient {
    child: Child,
    stdin: ChildStdin,
    /// Response lines from the Runtime's stdout, produced by a dedicated
    /// reader thread (see [`spawn_stdout_reader`]). Routing reads through a
    /// channel lets [`RuntimeClient::request_timed`] bound how long it
    /// waits for a reply with `Receiver::recv_timeout`, which a raw
    /// blocking `BufRead::read_line` on `ChildStdout` cannot do portably.
    stdout_rx: Receiver<io::Result<Vec<u8>>>,
    next_id: u64,
    capabilities: RuntimeCapabilities,
}

/// Reads newline-delimited responses off `stdout` on a background thread and
/// forwards each raw line (including a trailing `\n` if present) to the
/// returned channel. An `Ok(vec![])` marks a clean EOF (mirrors the previous
/// `line.is_empty()` "runtime closed stdout" check); an `Err` forwards the
/// underlying I/O error. Reading raw bytes (`read_until`) rather than
/// `String`-based `read_line` means a non-UTF-8 or binary response is still
/// captured for diagnostics instead of being silently dropped by a UTF-8
/// validation error inside `read_line`.
fn spawn_stdout_reader(stdout: ChildStdout) -> Receiver<io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut buf = Vec::new();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => {
                    let _ = tx.send(Ok(Vec::new()));
                    break;
                }
                Ok(_) => {
                    if tx.send(Ok(buf)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    });
    rx
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
            stdout_rx: spawn_stdout_reader(stdout),
            next_id: 1,
            capabilities: RuntimeCapabilities {
                schema: MCP_CONTRACT_SCHEMA.into(),
                protocol_version: String::new(),
                server_name: String::new(),
                server_version: None,
                tools: BTreeSet::new(),
            },
        };
        // Both the `initialize` request and the immediately-following
        // `tools/list` capability probe are part of the startup handshake,
        // not a regular operation: bound both with `HANDSHAKE_TIMEOUT` so a
        // hung or malformed-response Runtime fails fast (see
        // simplicio-runtime#3319) instead of blocking for the multi-second
        // to 30s range observed before this fix.
        let initialized = client.request_timed(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "simplicio-code", "version": env!("CARGO_PKG_VERSION") }
            }),
            Some(HANDSHAKE_TIMEOUT),
        )?;
        let server = initialized
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if !server.eq_ignore_ascii_case(EXPECTED_SERVER) {
            return Err(Error::IdentityMismatch(server.to_owned()));
        }
        client.notify("notifications/initialized", json!({}))?;
        let tools_list =
            client.request_timed("tools/list", json!({}), Some(HANDSHAKE_TIMEOUT))?;
        client.capabilities = parse_capabilities(&initialized, &tools_list)?;
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

    /// Searches file contents under `repo`, optionally scoped to a
    /// repo-relative `path` subdirectory and filtered by `globs`.
    ///
    /// Fails closed on any path-escape attempt: `path` (when given) is
    /// resolved through [`secure_relative_path`] exactly like
    /// `read`/`write`/`delete`/`list`/`stat`, and every entry in `globs` is
    /// validated by [`secure_glob`] to reject parent-traversal (`../`) and
    /// absolute-path segments *before* the request ever reaches the Runtime.
    /// Without this, a caller-supplied glob like `../../etc/*` would be
    /// forwarded to the Runtime unchecked — the same class of sandbox bypass
    /// PR #26 fixed for symlinked read/write/delete targets.
    pub fn search(
        &mut self,
        repo: &Path,
        pattern: &str,
        path: Option<&Path>,
        globs: &[String],
        case_insensitive: bool,
        literal: bool,
        max_files: usize,
        max_matches: usize,
    ) -> Result<SearchResult, Error> {
        let repo = canonical_repo(repo)?;
        let relative_path = path.map(|p| secure_relative_path(&repo, p)).transpose()?;
        let safe_globs = globs
            .iter()
            .map(|glob| secure_glob(glob))
            .collect::<Result<Vec<_>, _>>()?;
        let mut args = json!({
            "repo": repo,
            "query": pattern,
            "pattern": pattern,
            "globs": safe_globs,
            "case_insensitive": case_insensitive,
            "literal": literal,
            "max_files": max_files.min(DEFAULT_MAX_SEARCH_FILES),
            "max_matches": max_matches.min(DEFAULT_MAX_SEARCH_MATCHES),
        });
        if let Some(relative_path) = relative_path {
            args["path"] = json!(relative_path);
        }
        let result = self.call_tool("search", "simplicio_search", args)?;
        parse_search_result(&tool_text_or_json(&result))
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

    /// Sends a request and waits indefinitely for its response line. Used
    /// for regular post-handshake operations (`tools/call`), which are
    /// already bounded by their own request-level timeout (e.g. `exec`'s
    /// `timeout_ms`, enforced Runtime-side).
    fn request(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.request_timed(method, params, None)
    }

    /// Sends a request and waits for its response line, optionally bounded
    /// by `timeout`. When `timeout` elapses before a line arrives, returns
    /// [`Error::HandshakeTimeout`] rather than blocking further — used by
    /// [`RuntimeClient::spawn_in`] for the `initialize`/`tools/list`
    /// handshake so a hung or slow-to-misbehave Runtime fails fast (see
    /// simplicio-runtime#3319) instead of hanging for seconds-to-30s.
    ///
    /// When the response line fails to parse as JSON-RPC, the resulting
    /// [`Error::InvalidResponse`] includes a bounded, redacted snippet of
    /// the raw bytes actually received, so callers see e.g. "got non-JSON
    /// output, first bytes: ..." instead of a bare parse error.
    fn request_timed(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        let raw = match timeout {
            Some(timeout) => {
                self.stdout_rx
                    .recv_timeout(timeout)
                    .map_err(|_| Error::HandshakeTimeout {
                        method: method.to_owned(),
                        timeout,
                    })?
            }
            None => self.stdout_rx.recv().map_err(|_| {
                Error::Protocol("Runtime reader thread stopped unexpectedly".into())
            })?,
        }?;
        if raw.is_empty() {
            return Err(Error::Protocol("runtime closed stdout".into()));
        }
        let response: Value = serde_json::from_slice(&raw).map_err(|e| {
            Error::InvalidResponse(format!(
                "{e}; got non-JSON-RPC output from Runtime, first bytes (redacted, max {DIAGNOSTIC_SNIPPET_BYTES}): {}",
                redact_snippet(&raw, DIAGNOSTIC_SNIPPET_BYTES)
            ))
        })?;
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

/// Rejects a search glob that could escape the repository: absolute paths
/// (Unix `/...`, Windows `\...` or `C:...`) and any `..` path segment.
/// Globs are otherwise passed through unmodified (wildcards like `*`/`**`
/// are not path components and are left for the Runtime to interpret).
fn secure_glob(glob: &str) -> Result<String, Error> {
    if glob.is_empty() {
        return Err(Error::GlobRejected("empty glob".into()));
    }
    let looks_like_windows_drive =
        glob.len() >= 2 && glob.as_bytes()[1] == b':' && glob.as_bytes()[0].is_ascii_alphabetic();
    if glob.starts_with('/') || glob.starts_with('\\') || looks_like_windows_drive {
        return Err(Error::GlobRejected(format!(
            "absolute glob rejected: {glob}"
        )));
    }
    if glob.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(Error::GlobRejected(format!(
            "parent traversal in glob: {glob}"
        )));
    }
    Ok(glob.to_owned())
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

/// Bounded, redacted snippet of raw bytes received from the Runtime, used in
/// diagnostics when a handshake response fails to parse as JSON-RPC (see
/// [`RuntimeClient::request_timed`]).
///
/// This is a small, purpose-built redactor rather than a reuse of
/// `xai-crash-handler`'s `redact_report`: that crate isn't a dependency of
/// `simplicio-runtime-client`, and pulling in `regex` plus its full
/// crash-report redaction surface (env-var assignments, vendor secret
/// prefixes, `Bearer` tokens, `Args:`-labelled lines) just to redact a
/// 200-byte diagnostic snippet isn't worth the dependency weight. It covers
/// the same core risk that matters here — an absolute filesystem path
/// (Windows drive-letter/UNC or Unix `/...`) leaking a home directory or
/// username into an error message — without the extra dependency.
fn redact_snippet(raw: &[u8], max_bytes: usize) -> String {
    let bytes = &raw[..raw.len().min(max_bytes)];
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end_matches(['\r', '\n']);
    text.split_inclusive(char::is_whitespace)
        .map(redact_token_if_path)
        .collect()
}

/// Redacts `token` in place if it looks like an absolute filesystem path,
/// preserving any trailing whitespace captured by `split_inclusive` so
/// re-joining the tokens reproduces the original spacing.
fn redact_token_if_path(token: &str) -> String {
    let trimmed = token.trim_end_matches(char::is_whitespace);
    let trailing = &token[trimmed.len()..];
    if looks_like_absolute_path(trimmed) {
        format!("<REDACTED>{trailing}")
    } else {
        token.to_owned()
    }
}

/// Heuristic (non-regex) check for an absolute filesystem path: a Windows
/// UNC path (`\\server\share...`), a Windows drive-letter path (`C:\...` or
/// `C:/...`), or a Unix path with at least two `/`-separated segments
/// (`/home/alice`, `/etc/passwd`) — a bare leading `/` alone (e.g. inside
/// JSON like `"/"`) is left untouched since it carries no identifying info.
fn looks_like_absolute_path(word: &str) -> bool {
    if word.starts_with("\\\\") {
        return true;
    }
    let bytes = word.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return word[2..].starts_with(['\\', '/']);
    }
    word.starts_with('/') && word.matches('/').count() >= 2
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

fn parse_search_result(text: &str) -> Result<SearchResult, Error> {
    let result: SearchResult =
        serde_json::from_str(text).map_err(|e| Error::InvalidResponse(e.to_string()))?;
    if result.schema != "simplicio.search-result/v1" {
        return Err(Error::InvalidResponse(format!(
            "unsupported schema {}",
            result.schema
        )));
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
    fn secure_glob_rejects_parent_traversal_and_absolute_patterns() {
        assert!(secure_glob("../outside/*.rs").is_err());
        assert!(secure_glob("src/../../etc/*").is_err());
        assert!(secure_glob("/etc/*").is_err());
        assert!(secure_glob("C:\\Windows\\*").is_err());
        assert!(secure_glob("").is_err());
        assert_eq!(secure_glob("*.rs").unwrap(), "*.rs");
        assert_eq!(secure_glob("src/**/*.ts").unwrap(), "src/**/*.ts");
    }

    #[test]
    fn search_rejects_path_escaping_repo() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("inside.txt"), "ok").unwrap();
        // Mirrors `secure_path_rejects_parent_traversal_and_external_absolute_paths`
        // above: `search`'s optional `path` scope must fail closed exactly like
        // read/write/delete/list/stat, not just canonicalize the repo root.
        assert!(secure_relative_path(repo.path(), Path::new("../outside")).is_err());
        let outside = repo.path().parent().unwrap().join("outside");
        assert!(secure_relative_path(repo.path(), &outside).is_err());
    }

    #[test]
    fn parses_search_result_contract() {
        let result = parse_search_result(
            r#"{"schema":"simplicio.search-result/v1","matches":[{"path":"src/main.rs","line":3,"text":"fn main() {}"}],"truncated":false}"#,
        )
        .unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "src/main.rs");
        assert_eq!(result.matches[0].line, 3);
        assert!(!result.truncated);
    }

    #[test]
    fn parse_search_result_rejects_unknown_schema() {
        let error =
            parse_search_result(r#"{"schema":"unexpected/v9","matches":[],"truncated":false}"#)
                .unwrap_err();
        assert!(matches!(error, Error::InvalidResponse(_)));
    }

    #[test]
    fn spawn_in_fails_closed_when_runtime_binary_is_missing() {
        // Regression for PR #26's fail-closed guarantee: pointing SIMPLICIO_BIN
        // at a path that doesn't exist must produce `Error::RuntimeNotFound`,
        // never spawn a process or fall back to any local search/read.
        let missing = std::env::temp_dir().join("definitely-not-a-real-simplicio-binary-xyz");
        let repo = tempfile::tempdir().unwrap();
        // SAFETY: test-only, single-threaded within this test; no other test in
        // this process reads `SIMPLICIO_BIN` concurrently with a conflicting value.
        unsafe {
            std::env::set_var("SIMPLICIO_BIN", &missing);
        }
        let result = RuntimeClient::spawn_in(repo.path());
        unsafe {
            std::env::remove_var("SIMPLICIO_BIN");
        }
        assert!(matches!(result, Err(Error::RuntimeNotFound)));
    }

    #[test]
    fn redact_snippet_masks_absolute_paths_but_keeps_the_rest_of_the_message() {
        let raw = br#"Simplicio Runtime starting up at /home/alice/repos/simplicio banner"#;
        let out = redact_snippet(raw, 200);
        assert!(!out.contains("alice"), "username leaked: {out}");
        assert!(out.contains("Simplicio Runtime starting up at"));
        assert!(out.contains("<REDACTED>"));
        assert!(out.contains("banner"));
    }

    #[test]
    fn redact_snippet_masks_windows_and_unc_paths() {
        let raw = br#"loaded C:\Users\alice\config.json and \\server\share\alice\x"#;
        let out = redact_snippet(raw, 200);
        assert!(!out.contains("alice"), "leaked: {out}");
        assert!(out.contains("loaded <REDACTED>"));
    }

    #[test]
    fn redact_snippet_truncates_to_max_bytes() {
        let raw = vec![b'a'; 1000];
        let out = redact_snippet(&raw, 50);
        assert_eq!(out.len(), 50);
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
