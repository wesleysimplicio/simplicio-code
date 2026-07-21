//! Fail-closed MCP client used by Simplicio Code for project operations.
//!
//! The client deliberately contains no local filesystem or shell fallback. A
//! Runtime connection is negotiated with `initialize` and `tools/list`; each
//! operation checks its required capability immediately before sending the
//! request. This lets older Runtimes keep the already-supported read path
//! while rejecting newer operations with an actionable incompatibility error.

pub mod component_release;
pub mod map_cache;
pub mod loop_hub;

pub mod generated;

pub use component_release::{
    BundleManifest, BundleStore, CompatibilityContract, CompatibilityHandshake,
    ComponentRelease, ReleaseError, ReleaseIdentity, CODE_VERSIONS_SCHEMA, REQUIRED_COMPONENTS,
};

pub use map_cache::{
    MAP_RESULT_SCHEMA_V1, MapCache, MapResult, MapState, budgeted_summary, compute_repo_hash,
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const MCP_CONTRACT_SCHEMA: &str = "simplicio.code-mcp/v1";
const PROTOCOL_VERSION: &str = "2024-11-05";
const EXPECTED_SERVER: &str = "simplicio";
pub const DEFAULT_MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_EXEC_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MAX_SEARCH_FILES: usize = 2_000;
pub const DEFAULT_MAX_SEARCH_MATCHES: usize = 10_000;
/// Runtime-owned namespace for Prototype-First receipts and candidate
/// artifacts. Callers must use [`RuntimeClient::write_prototype_artifact`]
/// instead of writing this directory directly.
pub const PROTOTYPE_ARTIFACT_ROOT: &str = ".simplicio/artifacts/prototype-first";\n/// Bound on the handshake (`initialize` / `tools/list`) round trip, distinct
/// from [`DEFAULT_EXEC_TIMEOUT_MS`]: a broken or hung Runtime must fail fast
/// during connection negotiation instead of hanging for tens of seconds.
pub const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 2_000;
/// Bound on how many raw bytes of an unparsable handshake response are
/// surfaced in [`Error::InvalidResponse`] diagnostics.
const RAW_SNIPPET_MAX_BYTES: usize = 200;
static MAPPED_WORKSPACES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static SHARED_RUNTIME_CLIENTS: LazyLock<Mutex<HashMap<PathBuf, Weak<SharedRuntimeSession>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    #[error(
        "Simplicio Runtime handshake ('{method}') timed out after {elapsed_ms}ms; the Runtime is unresponsive or misbehaving"
    )]
    HandshakeTimeout { method: String, elapsed_ms: u64 },
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
    #[error("Simplicio Runtime release is incompatible: {0}")]
    CompatibilityMismatch(String),
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
    /// Provenance announced by the Runtime during initialize. Old Runtimes
    /// may omit this field; callers that need artifact pinning must use
    /// [`RuntimeClient::spawn_in_with_manifest`].
    #[serde(default)]
    pub component_release: Option<ReleaseIdentity>,
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
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    capabilities: RuntimeCapabilities,
}

struct SharedRuntimeSession {
    client: Mutex<Option<RuntimeClient>>,
    workspace: PathBuf,
}

/// A Runtime MCP connection shared by every Code surface in one process for a
/// workspace. The Hub remains the cross-process owner when present; this
/// registry is the Code-side guard against TUI/headless/ACP opening duplicate
/// Runtime children and handshakes.
#[derive(Clone)]
pub struct SharedRuntimeClient {
    session: Arc<SharedRuntimeSession>,
}

impl SharedRuntimeClient {
    /// Returns the existing connection for `workspace`, or creates exactly
    /// one lazy connection slot. No process is started until the first
    /// operation is executed.
    pub fn connect_in(workspace: &Path) -> Result<Self, Error> {
        let workspace = canonical_repo(workspace)?;
        let mut sessions = SHARED_RUNTIME_CLIENTS
            .lock()
            .map_err(|_| Error::Spawn("shared Runtime session lock poisoned".into()))?;
        if let Some(session) = sessions.get(&workspace).and_then(Weak::upgrade) {
            return Ok(Self { session });
        }
        let session = Arc::new(SharedRuntimeSession {
            client: Mutex::new(None),
            workspace: workspace.clone(),
        });
        sessions.insert(workspace, Arc::downgrade(&session));
        Ok(Self { session })
    }

    /// Executes one operation on the shared MCP connection. A failed
    /// operation invalidates the shared connection so the next caller gets a
    /// fresh, negotiated Runtime session instead of falling back locally.
    pub fn with_client<T>(
        &self,
        operation: impl FnOnce(&mut RuntimeClient, &Path) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut client = self
            .session
            .client
            .lock()
            .map_err(|_| Error::Spawn("shared Runtime client lock poisoned".into()))?;
        if client.is_none() {
            *client = Some(RuntimeClient::spawn_in(&self.session.workspace)?);
        }
        let result = operation(
            client.as_mut().expect("shared Runtime client initialized"),
            &self.session.workspace,
        );
        if result.is_err() {
            *client = None;
        }
        result
    }

    /// Drops a failed connection without changing the registry entry. The
    /// session handle remains reusable and reconnects on the next operation.
    pub fn invalidate(&self) -> Result<(), Error> {
        let mut client = self
            .session
            .client
            .lock()
            .map_err(|_| Error::Spawn("shared Runtime client lock poisoned".into()))?;
        *client = None;
        Ok(())
    }

    pub fn workspace(&self) -> &Path {
        &self.session.workspace
    }
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
                component_release: None,
                tools: BTreeSet::new(),
            },
        };
        let initialized = client.request_with_timeout(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "simplicio-code", "version": env!("CARGO_PKG_VERSION") }
            }),
            DEFAULT_HANDSHAKE_TIMEOUT_MS,
        )?;
        let server = initialized
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if !server.eq_ignore_ascii_case(EXPECTED_SERVER) {
            return Err(Error::IdentityMismatch(server.to_owned()));
        }
        client.notify("notifications/initialized", json!({}))?;
        let listed =
            client.request_with_timeout("tools/list", json!({}), DEFAULT_HANDSHAKE_TIMEOUT_MS)?;
        client.capabilities = parse_capabilities(&initialized, &listed)?;
        Ok(client)
    }

    /// Spawn and fail closed unless the Runtime advertises the exact pinned
    /// release and protocol range required by `manifest`.
    pub fn spawn_in_with_manifest(
        workspace: &Path,
        manifest: &BundleManifest,
    ) -> Result<Self, Error> {
        let client = Self::spawn_in(workspace)?;
        client.verify_compatibility(manifest)?;
        Ok(client)
    }

    /// Verify the already-negotiated Runtime against the installed bundle.
    /// This is intentionally explicit so legacy, unpinned callers remain
    /// readable while release-managed entry points cannot skip provenance.
    pub fn verify_compatibility(&self, manifest: &BundleManifest) -> Result<(), Error> {
        let release = self
            .capabilities
            .component_release
            .as_ref()
            .ok_or_else(|| {
                Error::CompatibilityMismatch(
                    "Runtime did not announce component_release provenance".into(),
                )
            })?;
        let handshake = CompatibilityHandshake::from_runtime(release, &self.capabilities.tools);
        handshake
            .verify_against(manifest)
            .map_err(|error| Error::CompatibilityMismatch(error.to_string()))
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

    /// Persist a preview artifact through the negotiated Runtime write tool.
    ///
    /// The Code process never creates this path locally. The Runtime remains
    /// the authority for artifact storage, atomicity, and rollback.
    pub fn write_prototype_artifact(
        &mut self,
        repo: &Path,
        artifact_id: &str,
        data: &[u8],
    ) -> Result<Value, Error> {
        if !safe_artifact_id(artifact_id) {
            return Err(Error::PathRejected(
                "prototype artifact id contains unsafe path characters".into(),
            ));
        }
        let path = Path::new(PROTOTYPE_ARTIFACT_ROOT).join(format!("{artifact_id}.json"));
        self.write_file(repo, &path, data)
    }\n\n    pub fn delete_file(&mut self, repo: &Path, path: &Path) -> Result<Value, Error> {
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
        let response: Value = serde_json::from_str(&line).map_err(|e| {
            Error::InvalidResponse(format!(
                "got non-JSON-RPC output from Runtime for '{method}' ({e}); first bytes: {}",
                redact_snippet(&line)
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

    /// Like [`Self::request`], but bounds the round trip to `timeout_ms`: if
    /// the Runtime hasn't answered by the deadline, the child process is
    /// force-killed (unblocking the pending stdout read) and a
    /// [`Error::HandshakeTimeout`] is returned instead of hanging. Intended
    /// for the `initialize`/`tools/list` handshake, where a misbehaving or
    /// wedged Runtime must fail fast rather than block for the much longer
    /// [`DEFAULT_EXEC_TIMEOUT_MS`] used by ordinary tool calls.
    fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<Value, Error> {
        let done = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let pid = self.child.id();
        let watcher = {
            let done = done.clone();
            let timed_out = timed_out.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                while Instant::now() < deadline {
                    if done.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !done.load(Ordering::SeqCst) {
                    timed_out.store(true, Ordering::SeqCst);
                    kill_pid(pid);
                }
            })
        };
        let started = Instant::now();
        let result = self.request(method, params);
        done.store(true, Ordering::SeqCst);
        let _ = watcher.join();
        match result {
            Err(_) if timed_out.load(Ordering::SeqCst) => Err(Error::HandshakeTimeout {
                method: method.to_owned(),
                elapsed_ms: started.elapsed().as_millis() as u64,
            }),
            other => other,
        }
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
        component_release: initialized
            .pointer("/serverInfo/metadata/componentRelease")
            .or_else(|| initialized.pointer("/serverInfo/componentRelease"))
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
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
    let mut paths = Vec::new();
    if let Some(files) = plan.get("files").and_then(Value::as_array) {
        for file in files {
            if let Some(path) = file.as_str() {
                paths.push(path);
            } else if let Some(object) = file.as_object() {
                for key in ["file", "move_to"] {
                    if let Some(path) = object.get(key).and_then(Value::as_str) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    if let Some(object) = plan.as_object() {
        for key in ["file", "move_to"] {
            if let Some(path) = object.get(key).and_then(Value::as_str) {
                paths.push(path);
            }
        }
    }
    for file in paths {
        secure_relative_path(repo, Path::new(file))?;
    }
    Ok(())
}

fn contains_shell_metacharacters(arg: &str) -> bool {
    arg.chars()
        .any(|c| matches!(c, '|' | ';' | '&' | '`' | '$' | '>' | '<' | '\n' | '\r'))
}

fn safe_artifact_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}\n\n/// Force-kills a process by pid, cross-platform, best-effort. Used only to
/// unblock a hung handshake read after [`DEFAULT_HANDSHAKE_TIMEOUT_MS`]; a
/// failure to kill is not itself fatal (the caller has already decided to
/// report a timeout either way).
fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Bounds and redacts a raw response line for inclusion in an error message:
/// truncates to [`RAW_SNIPPET_MAX_BYTES`] and blanks out anything that looks
/// like a home-directory path (`/Users/...`, `/home/...`, `C:\Users\...`) so
/// a malformed-response diagnostic never leaks a local username or absolute
/// path into logs.
fn redact_snippet(raw: &str) -> String {
    let bounded: String = raw.chars().take(RAW_SNIPPET_MAX_BYTES).collect();
    let truncated = raw.chars().count() > RAW_SNIPPET_MAX_BYTES;
    let redacted: String = bounded
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            let trimmed = token.trim_end();
            let lower = trimmed.to_ascii_lowercase();
            let looks_like_home_path = (trimmed.contains('/') || trimmed.contains('\\'))
                && (lower.contains("/users/")
                    || lower.contains("/home/")
                    || lower.contains(r"c:\users"));
            if looks_like_home_path {
                let suffix = &token[trimmed.len()..];
                format!("<redacted-path>{suffix}")
            } else {
                token.to_string()
            }
        })
        .collect();
    if truncated {
        format!("{redacted}...")
    } else {
        redacted
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
    use proptest::prelude::*;
    use std::fs;

    #[test]
    fn parses_runtime_capabilities_contract() {
        let capabilities = parse_capabilities(
            &json!({"protocolVersion": PROTOCOL_VERSION, "serverInfo": {"name": "simplicio", "version": "3.5.3"}}),
            &json!({"tools": [{"name": "simplicio_file_read"}, {"name": "simplicio_exec"}]}),
        )
        .unwrap();
        assert_eq!(capabilities.schema, MCP_CONTRACT_SCHEMA);
        assert!(capabilities.supports("simplicio_file_read"));
        assert!(!capabilities.supports("simplicio_fs_write"));
    }

    #[test]
    fn parses_runtime_component_release_metadata_for_pinned_handshake() {
        let capabilities = parse_capabilities(
            &json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {
                    "name": "simplicio",
                    "metadata": {
                        "componentRelease": {
                            "name": "runtime",
                            "version": "0.3.0",
                            "commit": "a0123456789abcdef",
                            "protocol": "RuntimeProtocol/v1",
                            "artifact_digest": "b".repeat(64)
                        }
                    }
                }
            }),
            &json!({"tools": []}),
        )
        .unwrap();
        assert_eq!(
            capabilities
                .component_release
                .as_ref()
                .map(|release| release.name.as_str()),
            Some("runtime")
        );
    }

    #[test]
    #[test]
    fn shared_runtime_handles_reuse_one_lazy_session_slot() {
        SHARED_RUNTIME_CLIENTS.lock().unwrap().clear();
        let workspace = tempfile::tempdir().unwrap();
        let first = SharedRuntimeClient::connect_in(workspace.path()).unwrap();
        let second = SharedRuntimeClient::connect_in(workspace.path()).unwrap();
        assert!(Arc::ptr_eq(&first.session, &second.session));
        assert!(first.session.client.lock().unwrap().is_none());
    }

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

    proptest! {
        /// Generalizes the example-based traversal checks to arbitrary path
        /// prefixes. The Runtime client must reject every generated path that
        /// contains a parent component before canonicalization can resolve it.
        #[test]
        fn secure_relative_path_rejects_generated_parent_segments(
            prefix in prop::collection::vec("[a-zA-Z0-9_-]{1,16}", 0..5),
        ) {
            let repo = tempfile::tempdir().unwrap();
            let mut path = PathBuf::new();
            for segment in prefix {
                path.push(segment);
            }
            path.push("..");
            path.push("outside");
            prop_assert!(secure_relative_path(repo.path(), &path).is_err());
        }

        /// A safe single-segment path stays within the repository and is
        /// returned in normalized repository-relative form.
        #[test]
        fn secure_relative_path_accepts_generated_safe_segments(
            segment in "[a-zA-Z0-9_-]{1,32}",
        ) {
            let repo = tempfile::tempdir().unwrap();
            let path = PathBuf::from(&segment);
            let result = secure_relative_path(repo.path(), &path).unwrap();
            prop_assert_eq!(result, segment);
        }

        /// No glob containing a parent segment may reach the Runtime search
        /// contract, regardless of the surrounding wildcard/text content.
        #[test]
        fn secure_glob_rejects_generated_parent_segments(
            left in "[a-zA-Z0-9_*?/]{0,24}",
            right in "[a-zA-Z0-9_*?]{0,24}",
        ) {
            let glob = format!("{left}/../{right}");
            prop_assert!(secure_glob(&glob).is_err());
        }
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
