use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use simplicio_agent_client::{AgentHostCoordinator, CausalIdentity, resolve_socket_path};
use simplicio_runtime_client::{
    DEFAULT_MAX_FILE_BYTES, FileReadResult, RuntimeClient, SearchResult, SharedRuntimeClient,
    start_workspace_map,
};
use simplicio_runtime_client::{
    SocketPipeHubTransportFactory,
    loop_hub::{HubClientConfig, HubMode, LoopHubClient, RuntimeExecuteRequest},
};

use super::preflight::{ProductivePreflightReport, run_installed_preflight};
use crate::computer::types::{
    AsyncFileSystem, AsyncSearch, BackgroundHandle, ComputerError, KillOutcome, SearchMatch,
    SearchOutcome, TaskSnapshot, TerminalBackend, TerminalRunRequest, TerminalRunResult,
};

pub const LOOP_HUB_CLIENT_SCHEMA: &str = "simplicio.loop-hub-client/v1";
pub const LOOP_HUB_RUNTIME_CALL_SCHEMA: &str = "simplicio.loop-runtime-call/v1";
const LOOP_HUB_ENDPOINT_ENV: &str = "SIMPLICIO_LOOP_HUB_ENDPOINT";
const LOOP_HUB_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Payload for the Loop Hub's external `runtime_call` method.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuntimeCallRequest {
    pub workspace: String,
    pub tool: String,
    pub arguments: Value,
    pub cwd: String,
    pub timeout_ms: u64,
    pub idempotency_key: String,
}

/// Result envelope returned by the Loop Hub's external `runtime_call` method.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RuntimeCallResponse {
    pub schema: String,
    pub workspace: String,
    pub tool: String,
    pub result: Value,
}

/// Client-only seam for fake Hub tests and the production socket/pipe adapter.
/// Implementations must not perform a local filesystem fallback.
pub trait RuntimeCallTransport: Send + Sync {
    fn call(&self, request: RuntimeCallRequest) -> Result<RuntimeCallResponse, ComputerError>;
}

#[derive(Debug, Serialize)]
struct RuntimeCallWireRequest<'a> {
    schema: &'static str,
    id: u64,
    method: &'static str,
    payload: &'a RuntimeCallRequest,
}

#[derive(Debug, Deserialize)]
struct RuntimeCallWireResponse {
    schema: String,
    id: u64,
    ok: bool,
    #[serde(default)]
    result: Option<RuntimeCallResponse>,
    #[serde(default)]
    error: Option<String>,
}

trait RuntimeCallReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> RuntimeCallReadWrite for T {}

struct RuntimeCallChannel {
    reader: BufReader<Box<dyn RuntimeCallReadWrite>>,
    writer: Box<dyn RuntimeCallReadWrite>,
}

impl RuntimeCallChannel {
    fn connect(endpoint: &str) -> Result<Self, ComputerError> {
        let endpoint = endpoint.trim();
        if let Some(path) = endpoint.strip_prefix("unix://") {
            #[cfg(unix)]
            {
                let stream = std::os::unix::net::UnixStream::connect(path)?;
                let reader_stream = stream.try_clone()?;
                return Ok(Self {
                    reader: BufReader::new(Box::new(reader_stream)),
                    writer: Box::new(stream),
                });
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(hub_runtime_error(
                    "unix:// endpoints are only supported on Unix",
                ));
            }
        }
        if let Some(name) = endpoint.strip_prefix("pipe://") {
            #[cfg(windows)]
            {
                let path = if name.starts_with(r"\\.\pipe\") {
                    name.to_owned()
                } else {
                    format!(r"\\.\pipe\{name}")
                };
                let stream = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)?;
                let reader_stream = stream.try_clone()?;
                return Ok(Self {
                    reader: BufReader::new(Box::new(reader_stream)),
                    writer: Box::new(stream),
                });
            }
            #[cfg(not(windows))]
            {
                let _ = name;
                return Err(hub_runtime_error(
                    "pipe:// endpoints are only supported on Windows",
                ));
            }
        }
        Err(hub_runtime_error(
            "Loop Hub endpoint must use unix:// or pipe://",
        ))
    }

    fn request(
        &mut self,
        id: u64,
        payload: &RuntimeCallRequest,
    ) -> Result<RuntimeCallResponse, ComputerError> {
        let wire = RuntimeCallWireRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA,
            id,
            method: "runtime_call",
            payload,
        };
        serde_json::to_writer(&mut self.writer, &wire)
            .map_err(|error| hub_runtime_error(error.to_string()))?;
        self.writer
            .write_all(b"\n")
            .and_then(|_| self.writer.flush())
            .map_err(|error| hub_runtime_error(error.to_string()))?;

        let mut frame = Vec::new();
        let bytes = self
            .reader
            .read_until(b'\n', &mut frame)
            .map_err(|error| hub_runtime_error(error.to_string()))?;
        if bytes == 0 {
            return Err(hub_runtime_error("Loop Hub closed the transport"));
        }
        if frame.len() > LOOP_HUB_MAX_FRAME_BYTES {
            return Err(hub_runtime_error(
                "Loop Hub response exceeded the frame limit",
            ));
        }
        let response: RuntimeCallWireResponse = serde_json::from_slice(&frame)
            .map_err(|error| hub_runtime_error(format!("invalid Loop Hub response: {error}")))?;
        if response.schema != LOOP_HUB_CLIENT_SCHEMA {
            return Err(hub_runtime_error(
                "Loop Hub response uses an unsupported client schema",
            ));
        }
        if response.id != id {
            return Err(hub_runtime_error(
                "Loop Hub response id does not match the request",
            ));
        }
        if !response.ok {
            return Err(hub_runtime_error(
                response
                    .error
                    .unwrap_or_else(|| "Loop Hub rejected the runtime_call".into()),
            ));
        }
        let result = response
            .result
            .ok_or_else(|| hub_runtime_error("Loop Hub response omitted runtime_call result"))?;
        if result.schema != LOOP_HUB_RUNTIME_CALL_SCHEMA {
            return Err(hub_runtime_error(format!(
                "Loop Hub runtime_call response uses unsupported schema {}",
                result.schema
            )));
        }
        if result.workspace != payload.workspace || result.tool != payload.tool {
            return Err(hub_runtime_error(
                "Loop Hub runtime_call response identity does not match the request",
            ));
        }
        Ok(result)
    }
}

/// Transport for an already-running Loop Hub Unix socket or Windows named pipe.
/// It attaches to the endpoint and never starts Hub, Runtime, Mapper, or a local
/// filesystem implementation.
pub struct SocketRuntimeCallTransport {
    endpoint: String,
    next_id: AtomicU64,
}

impl SocketRuntimeCallTransport {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl RuntimeCallTransport for SocketRuntimeCallTransport {
    fn call(&self, request: RuntimeCallRequest) -> Result<RuntimeCallResponse, ComputerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        RuntimeCallChannel::connect(&self.endpoint)?.request(id, &request)
    }
}

fn hub_runtime_error(message: impl Into<String>) -> ComputerError {
    ComputerError::io(format!(
        "Simplicio Loop Hub runtime_call failed closed: {}",
        message.into()
    ))
}

fn configured_hub_endpoint() -> Option<String> {
    std::env::var(LOOP_HUB_ENDPOINT_ENV)
        .ok()
        .map(|endpoint| endpoint.trim().to_owned())
        .filter(|endpoint| !endpoint.is_empty())
}

fn decode_hub_read_result(value: Value) -> Result<Vec<u8>, ComputerError> {
    let schema = value
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if schema != "simplicio.read-result/v1" {
        return Err(hub_runtime_error(format!(
            "unsupported Runtime read response schema {schema}"
        )));
    }
    if value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(hub_runtime_error("file exceeded the Runtime read limit"));
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| hub_runtime_error("Runtime read response omitted content"))?;
    if value.get("encoding").and_then(Value::as_str) == Some("base64") {
        let encoded = value
            .get("bytes_base64")
            .and_then(Value::as_str)
            .unwrap_or(content);
        return base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| hub_runtime_error(format!("invalid base64 read: {error}")));
    }
    Ok(content.as_bytes().to_vec())
}

fn decode_hub_file_read_result(value: Value) -> Result<FileReadResult, ComputerError> {
    let result: FileReadResult = serde_json::from_value(value)
        .map_err(|error| hub_runtime_error(format!("invalid Runtime read response: {error}")))?;
    if result.schema != "simplicio.read-result/v1" {
        return Err(hub_runtime_error(format!(
            "unsupported Runtime read response schema {}",
            result.schema
        )));
    }
    Ok(result)
}

fn decode_hub_search_result(value: Value) -> Result<SearchOutcome, ComputerError> {
    let result: SearchResult = serde_json::from_value(value)
        .map_err(|error| hub_runtime_error(format!("invalid Runtime search response: {error}")))?;
    if result.schema != "simplicio.search-result/v1" {
        return Err(hub_runtime_error(format!(
            "unsupported Runtime search response schema {}",
            result.schema
        )));
    }
    Ok(SearchOutcome {
        matches: result
            .matches
            .into_iter()
            .map(|m| SearchMatch {
                path: m.path,
                line: m.line,
                text: m.text,
            })
            .collect(),
        truncated: result.truncated,
    })
}

/// Runtime-owned execution boundary used by [`SimplicioRuntimeTerminalBackend`].
/// The trait makes the production adapter independently testable with a fake
/// Runtime while preserving the exact `simplicio_exec` response parser.
#[async_trait::async_trait]
pub trait RuntimeExecInvoker: Send + Sync {
    async fn exec_workspace(
        &self,
        cwd: &Path,
        argv: &[String],
        env: &BTreeMap<String, String>,
        timeout_ms: u64,
        max_output_bytes: usize,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, ComputerError>;
}

/// Project filesystem whose effects are owned by the Simplicio Runtime and
/// gated on a compatible, independently running Simplicio Agent host.
///
/// Every operation requires both products and fails closed: there is
/// intentionally no direct-local or built-in-agent fallback for any of them.
pub struct SimplicioRuntimeFs {
    root: PathBuf,
    agent_socket: PathBuf,
    agent_client: Arc<Mutex<Option<AgentHostCoordinator>>>,
    client: Arc<Mutex<Option<SharedRuntimeClient>>>,
    hub_transport: Option<Arc<dyn RuntimeCallTransport>>,
    next_hub_id: AtomicU64,
}

/// Terminal backend for productive Code sessions. It submits an argv-safe
/// command to Runtime and never starts a local process. Background lifecycle
/// operations remain unavailable until Runtime publishes a versioned task
/// capability; they fail closed instead of delegating to LocalTerminalBackend.
pub struct SimplicioRuntimeTerminalBackend {
    runtime: Arc<dyn RuntimeExecInvoker>,
}

impl SimplicioRuntimeTerminalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_runtime(Arc::new(SimplicioRuntimeFs::new(root)))
    }

    pub fn with_runtime(runtime: Arc<dyn RuntimeExecInvoker>) -> Self {
        Self { runtime }
    }
}

fn parse_terminal_argv(command: &str) -> Result<Vec<String>, ComputerError> {
    let argv = shlex::split(command).ok_or_else(|| {
        ComputerError::io("Simplicio Runtime rejected bash: unmatched quote or escape")
    })?;
    if argv.is_empty() {
        return Err(ComputerError::io(
            "Simplicio Runtime rejected empty command",
        ));
    }
    Ok(argv)
}

fn runtime_error(message: impl Into<String>) -> ComputerError {
    ComputerError::io(format!(
        "Simplicio Runtime exec failed closed: {}",
        message.into()
    ))
}

fn terminal_result(
    request: &TerminalRunRequest,
    value: &serde_json::Value,
) -> Result<TerminalRunResult, ComputerError> {
    let result = simplicio_runtime_client::parse_exec_result(value)
        .map_err(|error| runtime_error(error.to_string()))?;
    use simplicio_runtime_client::ExecEffectState;
    match result.effect_state {
        Some(ExecEffectState::Completed) => {}
        Some(ExecEffectState::NotStarted) | Some(ExecEffectState::Denied) => {
            return Err(runtime_error(format!(
                "effect state is {:?}",
                result.effect_state
            )));
        }
        Some(ExecEffectState::EffectUnknown) => {
            return Err(runtime_error(
                "effect state is effect_unknown; the command is not retryable",
            ));
        }
        None => return Err(runtime_error("response omitted effect_state")),
    }
    let mut combined_output = result.stdout;
    combined_output.push_str(&result.stderr);
    Ok(TerminalRunResult {
        total_bytes: result
            .total_bytes
            .unwrap_or_else(|| combined_output.as_bytes().len()),
        output_file: result
            .output_file
            .map(PathBuf::from)
            .unwrap_or_else(|| request.output_file.clone()),
        combined_output,
        exit_code: result.exit_code,
        truncated: result.truncated,
        signal: result.signal,
        timed_out: result.timed_out,
        pid: None,
    })
}

#[async_trait::async_trait]
impl RuntimeExecInvoker for SimplicioRuntimeFs {
    async fn exec_workspace(
        &self,
        cwd: &Path,
        argv: &[String],
        env: &BTreeMap<String, String>,
        timeout_ms: u64,
        max_output_bytes: usize,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, ComputerError> {
        if loop_hub_owns_runtime() {
            return self
                .exec_workspace_via_loop_hub(
                    cwd,
                    argv,
                    env,
                    timeout_ms,
                    max_output_bytes,
                    idempotency_key,
                )
                .await;
        }
        SimplicioRuntimeFs::exec_workspace(
            self,
            cwd,
            argv,
            env,
            timeout_ms,
            max_output_bytes,
            idempotency_key,
        )
        .await
    }
}

#[async_trait::async_trait]
impl TerminalBackend for SimplicioRuntimeTerminalBackend {
    async fn run(&self, request: TerminalRunRequest) -> Result<TerminalRunResult, ComputerError> {
        let argv = parse_terminal_argv(&request.command)?;
        let env = request
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        if request.tool_call_id.trim().is_empty() {
            return Err(runtime_error("tool_call_id is required as idempotency key"));
        }
        let value = self
            .runtime
            .exec_workspace(
                &request.working_directory,
                &argv,
                &env,
                request.timeout.as_millis().min(u64::MAX as u128) as u64,
                request.output_byte_limit,
                &request.tool_call_id,
            )
            .await?;
        terminal_result(&request, &value)
    }

    async fn run_background(
        &self,
        _request: TerminalRunRequest,
    ) -> Result<BackgroundHandle, ComputerError> {
        Err(runtime_error(
            "Runtime has no versioned background exec lifecycle capability; required dependency: simplicio_exec task start/status/cancel contract",
        ))
    }

    async fn get_task(&self, _task_id: &str) -> Option<TaskSnapshot> {
        None
    }

    async fn kill_task(&self, _task_id: &str) -> KillOutcome {
        KillOutcome::NotFound
    }

    async fn wait_for_completion(
        &self,
        _task_id: &str,
        _timeout: Option<std::time::Duration>,
    ) -> Option<TaskSnapshot> {
        None
    }

    async fn list_tasks(&self) -> Vec<TaskSnapshot> {
        Vec::new()
    }
}

fn loop_hub_endpoint_configured(endpoint: Option<&std::ffi::OsStr>) -> bool {
    endpoint.is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}

fn loop_hub_owns_runtime() -> bool {
    loop_hub_endpoint_configured(std::env::var_os("SIMPLICIO_LOOP_HUB_ENDPOINT").as_deref())
}

impl SimplicioRuntimeFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_agent_socket(root, resolve_socket_path())
    }

    fn with_agent_socket(root: impl Into<PathBuf>, agent_socket: impl Into<PathBuf>) -> Self {
        Self::with_agent_socket_and_hub_transport(root, agent_socket, None)
    }

    fn with_agent_socket_and_hub_transport(
        root: impl Into<PathBuf>,
        agent_socket: impl Into<PathBuf>,
        hub_transport: Option<Arc<dyn RuntimeCallTransport>>,
    ) -> Self {
        Self {
            root: root.into(),
            agent_socket: agent_socket.into(),
            agent_client: Arc::new(Mutex::new(None)),
            client: Arc::new(Mutex::new(None)),
            hub_transport,
            next_hub_id: AtomicU64::new(1),
        }
    }

    /// Creates a filesystem adapter backed by an injected Hub transport.
    /// This is the production adapter's fake seam; the transport remains the
    /// sole authority and no local filesystem fallback is introduced.
    pub fn with_hub_transport(
        root: impl Into<PathBuf>,
        transport: Arc<dyn RuntimeCallTransport>,
    ) -> Self {
        Self::with_agent_socket_and_hub_transport(root, resolve_socket_path(), Some(transport))
    }

    fn agent_profile() -> String {
        std::env::var("SIMPLICIO_AGENT_PROFILE")
            .ok()
            .filter(|profile| !profile.trim().is_empty())
            .unwrap_or_else(|| "desktop".into())
    }

    /// Runs the installed AgentHost + Runtime contract checks without
    /// entering the productive effect path.  This is the same dependency
    /// boundary used by [`Self::with_runtime`], but it deliberately does not
    /// create a Runtime tool call or grant effect authority.  Callers must
    /// still provide the real turn identity; an offline fixture cannot be
    /// substituted for this report.
    pub async fn preflight(&self, identity: CausalIdentity) -> ProductivePreflightReport {
        let workspace = self.root.clone();
        let agent_socket = self.agent_socket.clone();
        let profile = Self::agent_profile();
        tokio::task::spawn_blocking(move || {
            run_installed_preflight(&workspace, &agent_socket, &profile, &identity)
        })
        .await
        .unwrap_or_else(|error| ProductivePreflightReport {
            schema: super::preflight::PREFLIGHT_SCHEMA.into(),
            protocol_version: super::preflight::PREFLIGHT_PROTOCOL_VERSION,
            mode: super::preflight::INSTALLED_MODE.into(),
            effects_enabled: false,
            agent_host: super::preflight::CheckDiagnostic {
                component: "agent_host".into(),
                status: super::preflight::CheckStatus::Incompatible,
                code: "preflight.worker_failed".into(),
                detail: format!("preflight worker failed: {error}"),
            },
            runtime: super::preflight::CheckDiagnostic {
                component: "runtime".into(),
                status: super::preflight::CheckStatus::Incompatible,
                code: "preflight.worker_failed".into(),
                detail: "preflight worker did not complete".into(),
            },
            causal_identity: super::preflight::CausalIdentityDiagnostic {
                status: super::preflight::CheckStatus::Incompatible,
                code: "preflight.worker_failed".into(),
                detail: "causal identity was not granted effect authority".into(),
            },
        })
    }

    fn relative_path(&self, path: &Path) -> Result<PathBuf, ComputerError> {
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ComputerError::io(format!(
                "Simplicio Runtime denied parent traversal: {}",
                path.display()
            )));
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let normalized = absolute.components().collect::<PathBuf>();
        let relative = normalized
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                ComputerError::io(format!(
                    "Simplicio Runtime denied read outside workspace: {}",
                    path.display()
                ))
            })?;

        // Defense in depth against symlink escape. The checks above are
        // purely syntactic (`Path::components`), so a symlink placed
        // *inside* the workspace root that resolves to a target *outside*
        // it passes them untouched — e.g. `root/link -> /etc/passwd`
        // normalizes to `root/link`, strips the root prefix cleanly, and
        // would be handed to the Runtime as a seemingly in-root relative
        // path. When both sides exist, canonicalize and re-verify
        // containment; a not-yet-existing target (e.g. a file about to be
        // created) can't be canonicalized and keeps the syntactic result
        // above — the Runtime remains the read authority for those paths
        // (see docs/ARCHITECTURE.md).
        if let Ok(canonical_root) = self.root.canonicalize()
            && let Ok(canonical_target) = normalized.canonicalize()
            && !canonical_target.starts_with(&canonical_root)
        {
            return Err(ComputerError::io(format!(
                "Simplicio Runtime denied symlink escape: {}",
                path.display()
            )));
        }

        Ok(relative)
    }

    fn hub_transport(&self) -> Option<Arc<dyn RuntimeCallTransport>> {
        self.hub_transport.clone().or_else(|| {
            configured_hub_endpoint().map(|endpoint| {
                Arc::new(SocketRuntimeCallTransport::new(endpoint)) as Arc<dyn RuntimeCallTransport>
            })
        })
    }

    fn hub_workspace(&self) -> String {
        self.root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone())
            .to_string_lossy()
            .into_owned()
    }

    fn next_hub_id(&self) -> String {
        format!(
            "code-runtime-call-{}",
            self.next_hub_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    async fn call_hub(&self, tool: &str, arguments: Value) -> Result<Value, ComputerError> {
        let transport = self
            .hub_transport()
            .ok_or_else(|| hub_runtime_error("runtime_call transport is not configured"))?;
        let request = RuntimeCallRequest {
            workspace: self.hub_workspace(),
            tool: tool.to_owned(),
            arguments,
            cwd: ".".into(),
            timeout_ms: 120_000,
            idempotency_key: self.next_hub_id(),
        };
        let agent_client = Arc::clone(&self.agent_client);
        let agent_socket = self.agent_socket.clone();
        let requires_agent = configured_hub_endpoint().is_some();
        tokio::task::spawn_blocking(move || {
            if requires_agent {
                Self::ensure_agent_ready(&agent_client, &agent_socket)?;
            }
            transport.call(request)
        })
        .await
        .map_err(|error| hub_runtime_error(format!("transport task failed: {error}")))?
        .map(|response| response.result)
    }

    /// Verifies the independent Agent host, then runs `op` against a lazily
    /// initialized Runtime MCP session. Either missing/incompatible dependency
    /// blocks the operation; no local coordinator/filesystem fallback exists.
    async fn with_runtime<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut RuntimeClient, &Path) -> Result<T, simplicio_runtime_client::Error>
        + Send
        + 'static,
    ) -> Result<T, ComputerError> {
        let root = self.root.clone();
        let agent_socket = self.agent_socket.clone();
        let agent_client = Arc::clone(&self.agent_client);
        let client = Arc::clone(&self.client);
        tokio::task::spawn_blocking(move || {
            Self::ensure_agent_ready(&agent_client, &agent_socket)?;

            if loop_hub_owns_runtime() {
                return Err(ComputerError::io("Simplicio Loop Hub owns Runtime/Mapper; local Runtime spawn is disabled while SIMPLICIO_LOOP_HUB_ENDPOINT is configured"));
            }

            // Runtime-owned mapping starts only after the mandatory Agent
            // handshake; Code cannot partially run one dependency without the
            // other. Mapping remains best-effort and never bypasses Runtime.
            if let Err(error) = start_workspace_map(&root) {
                tracing::warn!(%error, workspace = %root.display(), "Simplicio Mapper bootstrap failed");
            }

            let shared = {
                let mut guard = client
                    .lock()
                    .map_err(|_| ComputerError::io("Simplicio Runtime client lock poisoned"))?;
                if guard.is_none() {
                    *guard = Some(
                        SharedRuntimeClient::connect_in(&root)
                            .map_err(|e| ComputerError::io(e.to_string()))?,
                    );
                }
                guard
                    .as_ref()
                    .expect("shared Runtime session initialized")
                    .clone()
            };
            shared
                .with_client(|runtime, workspace| op(runtime, workspace))
                .map_err(|e| ComputerError::io(e.to_string()))
        })
        .await
        .map_err(|e| ComputerError::io(format!("Simplicio Runtime task failed: {e}")))?
    }

    fn ensure_agent_ready(
        agent_client: &Arc<Mutex<Option<AgentHostCoordinator>>>,
        agent_socket: &Path,
    ) -> Result<(), ComputerError> {
        let mut agent_guard = agent_client
            .lock()
            .map_err(|_| ComputerError::io("Simplicio Agent client lock poisoned"))?;
        let validation = if let Some(agent) = agent_guard.as_mut() {
            agent.ensure_ready()
        } else {
            AgentHostCoordinator::connect_at(Self::agent_profile(), agent_socket.to_path_buf())
                .map(|agent| *agent_guard = Some(agent))
        };
        if let Err(error) = validation {
            *agent_guard = None;
            return Err(ComputerError::io(error.to_string()));
        }
        Ok(())
    }

    async fn exec_workspace_via_loop_hub(
        &self,
        cwd: &Path,
        argv: &[String],
        env: &BTreeMap<String, String>,
        timeout_ms: u64,
        max_output_bytes: usize,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, ComputerError> {
        let relative_cwd = self.relative_path(cwd)?;
        let root = self.root.clone();
        let agent_socket = self.agent_socket.clone();
        let agent_client = Arc::clone(&self.agent_client);
        let argv = argv.to_vec();
        let env = env.clone();
        let idempotency_key = idempotency_key.to_owned();
        tokio::task::spawn_blocking(move || {
            Self::ensure_agent_ready(&agent_client, &agent_socket)?;
            let endpoint = std::env::var("SIMPLICIO_LOOP_HUB_ENDPOINT")
                .map_err(|_| ComputerError::io("Loop Hub endpoint is not configured"))?;
            let mut config = HubClientConfig::new(
                HubMode::Required,
                "code-runtime-fs",
                root.to_string_lossy().to_string(),
                std::env::var("SIMPLICIO_CODE_SESSION_ID")
                    .unwrap_or_else(|_| "code-runtime-session".into()),
            );
            config.endpoint = Some(endpoint);
            let hub = LoopHubClient::connect(config, &SocketPipeHubTransportFactory)
                .map_err(|error| {
                    ComputerError::io(format!("Loop Hub Runtime connection failed: {error}"))
                })?
                .ok_or_else(|| {
                    ComputerError::io("Loop Hub Runtime connection was not established")
                })?;
            let request = RuntimeExecuteRequest {
                schema: "simplicio.loop-runtime-execute/v1".into(),
                workspace: root.to_string_lossy().to_string(),
                cwd: relative_cwd.to_string_lossy().to_string(),
                argv,
                env,
                timeout_ms,
                max_output_bytes,
                idempotency_key,
            };
            hub.shared_runtime_handle()
                .runtime_execute(&request)
                .map(|receipt| receipt.result)
                .map_err(|error| {
                    ComputerError::io(format!("Loop Hub Runtime effect failed: {error}"))
                })
        })
        .await
        .map_err(|error| ComputerError::io(format!("Loop Hub Runtime task failed: {error}")))?
    }

    /// Read/write/delete variant of [`Self::with_runtime`]: resolves and
    /// sandbox-checks `path` up front (see [`Self::relative_path`]), then
    /// hands `op` the validated repo root + relative path.
    async fn with_client<T: Send + 'static>(
        &self,
        path: &Path,
        op: impl FnOnce(&mut RuntimeClient, &Path, &Path) -> Result<T, simplicio_runtime_client::Error>
        + Send
        + 'static,
    ) -> Result<T, ComputerError> {
        let relative = self.relative_path(path)?;
        self.with_runtime(move |client, root| op(client, root, &relative))
            .await
    }

    /// Lists a workspace-relative directory through Runtime after the
    /// mandatory AgentHost handshake. This is intentionally read-only so it
    /// can be surfaced as direct TUI observability without bypassing the
    /// Agent's approval path for productive effects.
    pub async fn list_workspace(
        &self,
        path: &Path,
        options: serde_json::Value,
    ) -> Result<serde_json::Value, ComputerError> {
        let relative = self.relative_path(path)?;
        if self.hub_transport().is_some() {
            let workspace = self.hub_workspace();
            return self
                .call_hub(
                    "simplicio_fs_list",
                    json!({
                        "repo": workspace,
                        "path": relative.to_string_lossy(),
                        "options": options,
                    }),
                )
                .await;
        }
        self.with_client(path, move |client, root, relative| {
            client.list(root, relative, options)
        })
        .await
    }

    /// Returns Runtime-owned metadata for one workspace-relative path. Like
    /// [`Self::list_workspace`], this remains fail-closed on either missing
    /// dependency and performs no local filesystem fallback.
    pub async fn stat_workspace(&self, path: &Path) -> Result<serde_json::Value, ComputerError> {
        let relative = self.relative_path(path)?;
        if self.hub_transport().is_some() {
            let workspace = self.hub_workspace();
            return self
                .call_hub(
                    "simplicio_fs_stat",
                    json!({
                        "repo": workspace,
                        "path": relative.to_string_lossy(),
                    }),
                )
                .await;
        }
        self.with_client(path, |client, root, relative| client.stat(root, relative))
            .await
    }

    /// Applies an atomic Runtime edit plan. Its schema belongs to the
    /// independently versioned Runtime contract; this adapter owns the Agent
    /// gate and fail-closed lifecycle.
    pub async fn edit_workspace(
        &self,
        plan: serde_json::Value,
    ) -> Result<serde_json::Value, ComputerError> {
        if self.hub_transport().is_some() {
            let plan = serde_json::to_string(&plan)
                .map_err(|error| hub_runtime_error(format!("invalid edit plan: {error}")))?;
            return self
                .call_hub(
                    "simplicio_edit",
                    json!({
                        "repo": self.hub_workspace(),
                        "plan": plan,
                        "atomic": true,
                        "rollback": true,
                    }),
                )
                .await;
        }
        self.with_runtime(move |client, root| client.edit(root, plan))
            .await
    }

    /// Executes argv through Runtime without accepting a shell string.
    pub async fn exec_workspace(
        &self,
        cwd: &Path,
        argv: &[String],
        env: &BTreeMap<String, String>,
        timeout_ms: u64,
        max_output_bytes: usize,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, ComputerError> {
        let relative_cwd = self.relative_path(cwd)?;
        let argv = argv.to_vec();
        let env = env.clone();
        let idempotency_key = idempotency_key.to_owned();
        self.with_runtime(move |client, root| {
            client.exec(
                root,
                &relative_cwd,
                &argv,
                &env,
                timeout_ms,
                max_output_bytes,
                &idempotency_key,
            )
        })
        .await
    }

    /// Reads UTF-8 text through the Runtime boundary for the explicit TUI
    /// inspection route. Binary-oriented callers keep using `read_file`.
    pub async fn read_workspace(&self, path: &Path) -> Result<String, ComputerError> {
        if self.hub_transport().is_some() {
            let bytes = self.read_file(path).await?;
            return String::from_utf8(bytes).map_err(|error| {
                hub_runtime_error(format!("Runtime returned non-UTF-8 text: {error}"))
            });
        }
        self.with_client(path, |client, root, relative| {
            client
                .read_file(root, relative, DEFAULT_MAX_FILE_BYTES)
                .map(|read| read.content)
        })
        .await
    }

    /// Reads a bounded byte range through Runtime without probing the local
    /// filesystem for size, metadata, or content.
    pub async fn read_workspace_range(
        &self,
        path: &Path,
        start: Option<u64>,
        end: Option<u64>,
        max_bytes: usize,
    ) -> Result<FileReadResult, ComputerError> {
        if self.hub_transport().is_some() {
            let relative = self.relative_path(path)?;
            return self
                .call_hub(
                    "simplicio_file_read",
                    json!({
                        "repo": self.hub_workspace(),
                        "path": relative.to_string_lossy(),
                        "start": start,
                        "end": end,
                        "max_bytes": max_bytes,
                    }),
                )
                .await
                .and_then(decode_hub_file_read_result);
        }
        self.with_client(path, move |client, root, relative| {
            client.read_file_range(root, relative, start, end, max_bytes)
        })
        .await
    }

    /// Searches through Runtime while retaining the mandatory AgentHost gate.
    pub async fn search_workspace(
        &self,
        pattern: &str,
        path: Option<&Path>,
    ) -> Result<SearchOutcome, ComputerError> {
        self.search(pattern, path, &[], false, false, 100, 100)
            .await
    }
}

#[async_trait::async_trait]
impl AsyncSearch for SimplicioRuntimeFs {
    /// Searches through the Runtime's `simplicio_search` MCP tool. Fails
    /// closed exactly like read/write/delete: a missing/incompatible Runtime,
    /// or a `path` scope that escapes the workspace (checked via the same
    /// [`Self::relative_path`] sandbox used by every other operation,
    /// including its symlink-escape hardening), blocks the search with an
    /// actionable error instead of silently searching local disk.
    #[tracing::instrument(name = "simplicio_runtime.search", skip_all)]
    async fn search(
        &self,
        pattern: &str,
        path: Option<&Path>,
        globs: &[String],
        case_insensitive: bool,
        literal: bool,
        max_files: usize,
        max_matches: usize,
    ) -> Result<SearchOutcome, ComputerError> {
        let relative_scope = path.map(|p| self.relative_path(p)).transpose()?;
        let pattern = pattern.to_owned();
        let globs = globs.to_vec();
        if self.hub_transport().is_some() {
            let path = relative_scope
                .as_deref()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .into_owned();
            return self
                .call_hub(
                    "simplicio_fs_search",
                    json!({
                        "repo": self.hub_workspace(),
                        "path": path,
                        "pattern": pattern,
                        "globs": globs,
                        "case_insensitive": case_insensitive,
                        "literal": literal,
                        "max_files": max_files,
                        "max_matches": max_matches,
                    }),
                )
                .await
                .and_then(decode_hub_search_result);
        }
        self.with_runtime(move |client, root| {
            let result = client.search(
                root,
                &pattern,
                relative_scope.as_deref(),
                &globs,
                case_insensitive,
                literal,
                max_files,
                max_matches,
            )?;
            Ok(SearchOutcome {
                matches: result
                    .matches
                    .into_iter()
                    .map(|m| SearchMatch {
                        path: m.path,
                        line: m.line,
                        text: m.text,
                    })
                    .collect(),
                truncated: result.truncated,
            })
        })
        .await
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for SimplicioRuntimeFs {
    #[tracing::instrument(name = "simplicio_runtime.fs.read_file", skip_all)]
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        if self.hub_transport().is_some() {
            let relative = self.relative_path(path)?;
            return self
                .call_hub(
                    "simplicio_file_read",
                    json!({
                        "repo": self.hub_workspace(),
                        "path": relative.to_string_lossy(),
                        "max_bytes": DEFAULT_MAX_FILE_BYTES,
                    }),
                )
                .await
                .and_then(decode_hub_read_result);
        }
        self.with_client(path, |client, root, relative| {
            client
                .read_file(root, relative, DEFAULT_MAX_FILE_BYTES)
                .map(|read| read.content.into_bytes())
        })
        .await
    }

    /// Writes fail closed exactly like reads: there is no local fallback, so
    /// a missing/incompatible Runtime blocks the write with an actionable
    /// error rather than silently touching disk directly.
    #[tracing::instrument(name = "simplicio_runtime.fs.write_file", skip_all)]
    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        let data = data.to_vec();
        if self.hub_transport().is_some() {
            let relative = self.relative_path(path)?;
            return self
                .call_hub(
                    "simplicio_fs_write",
                    json!({
                        "repo": self.hub_workspace(),
                        "path": relative.to_string_lossy(),
                        "content_base64": base64::engine::general_purpose::STANDARD.encode(data),
                        "encoding": "base64",
                        "atomic": true,
                        "rollback": true,
                    }),
                )
                .await
                .map(|_| ());
        }
        self.with_client(path, move |client, root, relative| {
            client.write_file(root, relative, &data).map(|_| ())
        })
        .await
    }

    /// Deletes fail closed exactly like reads: there is no local fallback, so
    /// a missing/incompatible Runtime blocks the delete with an actionable
    /// error rather than silently touching disk directly.
    #[tracing::instrument(name = "simplicio_runtime.fs.delete_file", skip_all)]
    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        if self.hub_transport().is_some() {
            let relative = self.relative_path(path)?;
            return self
                .call_hub(
                    "simplicio_fs_delete",
                    json!({
                        "repo": self.hub_workspace(),
                        "path": relative.to_string_lossy(),
                        "atomic": true,
                        "rollback": true,
                    }),
                )
                .await
                .map(|_| ());
        }
        self.with_client(path, |client, root, relative| {
            client.delete_file(root, relative).map(|_| ())
        })
        .await
    }

    /// Routes a complete patch through Runtime's atomic `simplicio_edit`
    /// contract. A Runtime error is returned as-is; this backend never
    /// degrades an edit into local per-file writes.
    #[tracing::instrument(name = "simplicio_runtime.fs.apply_edit", skip_all)]
    async fn apply_edit(
        &self,
        plan: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ComputerError> {
        self.edit_workspace(plan).await.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_hub_endpoint_ownership_is_fail_closed() {
        assert!(!loop_hub_endpoint_configured(None));
        assert!(!loop_hub_endpoint_configured(Some(std::ffi::OsStr::new(
            "  "
        ))));
        assert!(loop_hub_endpoint_configured(Some(std::ffi::OsStr::new(
            "/tmp/simplicio-loop.sock"
        ))));
    }

    #[derive(Default)]
    struct FakeRuntimeCallTransport {
        calls: Mutex<Vec<RuntimeCallRequest>>,
    }

    impl RuntimeCallTransport for FakeRuntimeCallTransport {
        fn call(&self, request: RuntimeCallRequest) -> Result<RuntimeCallResponse, ComputerError> {
            let tool = request.tool.clone();
            self.calls.lock().unwrap().push(request.clone());
            let result = match tool.as_str() {
                "simplicio_file_read" => json!({
                    "schema": "simplicio.read-result/v1",
                    "path": "read.txt",
                    "content": "aHViLXJlYWQ=",
                    "bytes_base64": "aHViLXJlYWQ=",
                    "encoding": "base64",
                    "truncated": false,
                }),
                "simplicio_fs_search" => json!({
                    "schema": "simplicio.search-result/v1",
                    "matches": [{"path": "read.txt", "line": 1, "text": "needle"}],
                    "truncated": false,
                }),
                _ => json!({"schema": "simplicio.fake-result/v1", "tool": tool}),
            };
            Ok(RuntimeCallResponse {
                schema: LOOP_HUB_RUNTIME_CALL_SCHEMA.into(),
                workspace: request.workspace,
                tool,
                result,
            })
        }
    }

    #[tokio::test]
    async fn hub_runtime_call_routes_supported_filesystem_operations_to_fake_transport() {
        let workspace = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRuntimeCallTransport::default());
        let fs = SimplicioRuntimeFs::with_hub_transport(workspace.path(), fake.clone());
        let edit_plan = json!({
            "files": [{"file": "edited.txt", "operation": "update", "content": "hub"}]
        });

        fs.list_workspace(Path::new("."), json!({"hidden": false}))
            .await
            .unwrap();
        fs.stat_workspace(Path::new("read.txt")).await.unwrap();
        assert_eq!(
            fs.read_file(Path::new("read.txt")).await.unwrap(),
            b"hub-read"
        );
        fs.write_file(Path::new("write.txt"), b"hub-write")
            .await
            .unwrap();
        fs.delete_file(Path::new("delete.txt")).await.unwrap();
        fs.apply_edit(edit_plan.clone()).await.unwrap();

        let calls = fake.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|call| call.tool.as_str())
                .collect::<Vec<_>>(),
            vec![
                "simplicio_fs_list",
                "simplicio_fs_stat",
                "simplicio_file_read",
                "simplicio_fs_write",
                "simplicio_fs_delete",
                "simplicio_edit",
            ]
        );
        assert!(calls.iter().all(|call| call.cwd == "."));
        assert!(
            calls
                .iter()
                .all(|call| call.idempotency_key.starts_with("code-runtime-call-"))
        );
        assert_eq!(calls[0].arguments["options"], json!({"hidden": false}));
        assert_eq!(
            calls[2].arguments["max_bytes"],
            json!(DEFAULT_MAX_FILE_BYTES)
        );
        assert_eq!(
            calls[3].arguments["content_base64"],
            json!(base64::engine::general_purpose::STANDARD.encode(b"hub-write"))
        );
        assert_eq!(
            calls[5].arguments["plan"],
            serde_json::to_string(&edit_plan).unwrap()
        );
        assert!(!workspace.path().join("write.txt").exists());
    }

    #[tokio::test]
    async fn hub_runtime_call_routes_search_through_versioned_contract() {
        let workspace = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeRuntimeCallTransport::default());
        let fs = SimplicioRuntimeFs::with_hub_transport(workspace.path(), fake.clone());

        let result = fs
            .search("needle", None, &[], false, false, 100, 100)
            .await
            .expect("search must use the Runtime contract");
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "read.txt");
        assert_eq!(fake.calls.lock().unwrap()[0].tool, "simplicio_fs_search");
    }

    #[cfg(unix)]
    #[test]
    fn socket_runtime_call_transport_serializes_and_validates_wire() {
        use std::os::unix::net::UnixListener;
        use std::thread;

        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().join("hub.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["schema"], LOOP_HUB_CLIENT_SCHEMA);
            assert_eq!(request["method"], "runtime_call");
            assert_eq!(request["payload"]["tool"], "simplicio_fs_stat");

            let mut stream = stream;
            let response = json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "id": request["id"],
                "ok": true,
                "result": {
                    "schema": LOOP_HUB_RUNTIME_CALL_SCHEMA,
                    "workspace": "/workspace",
                    "tool": "simplicio_fs_stat",
                    "result": {"schema": "simplicio.fs-stat-result/v1", "exists": true},
                },
            });
            writeln!(stream, "{response}").unwrap();
        });

        let transport = SocketRuntimeCallTransport::new(format!("unix://{}", endpoint.display()));
        let response = transport
            .call(RuntimeCallRequest {
                workspace: "/workspace".into(),
                tool: "simplicio_fs_stat".into(),
                arguments: json!({"repo": "/workspace", "path": "probe.txt"}),
                cwd: ".".into(),
                timeout_ms: 1_000,
                idempotency_key: "wire-test-1".into(),
            })
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.schema, LOOP_HUB_RUNTIME_CALL_SCHEMA);
        assert_eq!(response.result["exists"], true);
    }

    #[tokio::test]
    async fn operation_fails_closed_before_runtime_when_agent_is_missing() {
        let workspace = tempfile::tempdir().unwrap();
        let missing_socket = workspace.path().join("missing-agent.sock");
        let fs = SimplicioRuntimeFs::with_agent_socket(workspace.path(), missing_socket);

        let error = fs
            .search("needle", None, &[], false, false, 100, 100)
            .await
            .expect_err("an operation without Agent must fail before Runtime startup");

        assert!(
            error
                .to_string()
                .contains("Simplicio Agent socket was not found"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn inspection_commands_fail_closed_before_runtime_when_agent_is_missing() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("present.txt"), b"present").unwrap();
        let missing_socket = workspace.path().join("missing-agent.sock");
        let fs = SimplicioRuntimeFs::with_agent_socket(workspace.path(), missing_socket);

        let list_error = fs
            .list_workspace(Path::new("."), serde_json::json!({}))
            .await
            .expect_err("list must require Agent before Runtime startup");
        let stat_error = fs
            .stat_workspace(Path::new("present.txt"))
            .await
            .expect_err("stat must require Agent before Runtime startup");

        assert!(
            list_error
                .to_string()
                .contains("Simplicio Agent socket was not found")
        );
        assert!(
            stat_error
                .to_string()
                .contains("Simplicio Agent socket was not found")
        );
    }

    #[tokio::test]
    async fn edit_and_exec_fail_closed_without_local_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let edit = fs
            .edit_workspace(serde_json::json!({"file": "missing.txt"}))
            .await;
        let exec = fs
            .exec_workspace(
                Path::new("."),
                &["echo".to_owned(), "blocked".to_owned()],
                &BTreeMap::new(),
                1_000,
                1_024,
                "fail-closed-test",
            )
            .await;
        assert!(edit.is_err(), "edit must require the Runtime boundary");
        assert!(exec.is_err(), "exec must require the Runtime boundary");
    }

    /// Regression: writes must fail closed exactly like the PR #1/#2 read
    /// path when mandatory Agent/Runtime dependencies are unavailable, and
    /// critically no file is silently created on local disk as a fallback.
    #[tokio::test]
    async fn write_file_fails_closed_without_local_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let target = workspace.path().join("should_not_exist.txt");
        let result = fs
            .write_file(Path::new("should_not_exist.txt"), b"data")
            .await;
        assert!(
            result.is_err(),
            "write must fail closed without mandatory dependencies"
        );
        assert!(
            !target.exists(),
            "write must not silently fall back to local disk"
        );
    }

    /// Same guarantee for deletes: missing mandatory dependencies must not let
    /// a delete silently fall back to touching the file directly.
    #[tokio::test]
    async fn delete_file_fails_closed_without_local_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("keep.txt");
        std::fs::write(&target, b"keep me").unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let result = fs.delete_file(Path::new("keep.txt")).await;
        assert!(
            result.is_err(),
            "delete must fail closed without mandatory dependencies"
        );
        assert!(
            target.exists(),
            "delete must not silently fall back to local disk"
        );
    }

    /// New capability, same fail-closed contract as read/write/delete: missing
    /// mandatory dependencies reject `search` outright, never silently
    /// degrading to a local ripgrep/`tokio::fs` walk.
    #[tokio::test]
    async fn search_fails_closed_without_local_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let result = fs.search("needle", None, &[], false, false, 100, 100).await;
        assert!(
            result.is_err(),
            "search must fail closed without mandatory dependencies"
        );
    }

    /// Security regression, mirroring `rejects_symlink_escape_to_outside_target`
    /// below: a search scoped to a `path` that is a symlink escaping the
    /// workspace root must be denied before ever reaching the Runtime, not
    /// just when reading/writing/deleting that same path.
    #[cfg(unix)]
    #[tokio::test]
    async fn search_rejects_symlink_escape_scope() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("secret_dir")).unwrap();
        symlink(
            outside.path().join("secret_dir"),
            workspace.path().join("escape_dir"),
        )
        .unwrap();

        let fs = SimplicioRuntimeFs::new(workspace.path());
        let error = fs
            .search(
                "needle",
                Some(Path::new("escape_dir")),
                &[],
                false,
                false,
                100,
                100,
            )
            .await
            .expect_err("search scoped to a symlink escaping the workspace must be denied");
        assert!(
            error.to_string().contains("symlink escape"),
            "unexpected error: {error}"
        );
    }

    /// Regression: a search scoped via plain parent-traversal (`../`) must
    /// still be denied the same way read/write/delete already are.
    #[tokio::test]
    async fn search_rejects_parent_traversal_scope() {
        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let result = fs
            .search(
                "needle",
                Some(Path::new("../outside")),
                &[],
                false,
                false,
                100,
                100,
            )
            .await;
        assert!(
            result.is_err(),
            "search scope must reject parent traversal exactly like read/write/delete"
        );
    }

    /// Regression: the syntactic parent-traversal / outside-root checks
    /// shipped in PR #1/#2 must still fail closed after the symlink-escape
    /// hardening added on top of them.
    #[test]
    fn rejects_parent_escape() {
        let fs = SimplicioRuntimeFs::new("/workspace");
        assert!(fs.relative_path(Path::new("/outside/secret")).is_err());
        assert!(fs.relative_path(Path::new("../outside/secret")).is_err());
    }

    #[test]
    fn allows_plain_relative_path_inside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("main.rs"), b"fn main() {}").unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        let relative = fs
            .relative_path(Path::new("main.rs"))
            .expect("in-root file should resolve");
        assert_eq!(relative, Path::new("main.rs"));
    }

    /// A path that doesn't exist yet (e.g. a file about to be created) can't
    /// be canonicalized, so the symlink-escape check must not hard-fail it —
    /// it falls back to the syntactic containment check, matching the
    /// pre-hardening behavior for reads of not-yet-materialized paths.
    #[test]
    fn allows_syntactically_contained_nonexistent_path() {
        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        assert!(fs.relative_path(Path::new("not_created_yet.rs")).is_ok());
    }

    /// A symlink that stays inside the workspace root must keep working —
    /// the hardening targets escapes, not symlinks in general.
    #[cfg(unix)]
    #[test]
    fn allows_symlink_that_stays_inside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("real.rs"), b"fn real() {}").unwrap();
        symlink(
            workspace.path().join("real.rs"),
            workspace.path().join("alias.rs"),
        )
        .unwrap();
        let fs = SimplicioRuntimeFs::new(workspace.path());
        assert!(fs.relative_path(Path::new("alias.rs")).is_ok());
    }

    /// Security regression: a symlink placed *inside* the workspace root
    /// that resolves to a target *outside* it must be denied. Before the
    /// canonicalize-based check, `relative_path` only inspected path syntax
    /// (`Path::components`), so `root/escape -> <outside>/secret.txt`
    /// normalized to `root/escape`, stripped the root prefix cleanly, and
    /// would have been forwarded to the Runtime as if it were an ordinary
    /// in-root relative path — a real sandbox bypass.
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_to_outside_target() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        symlink(
            outside.path().join("secret.txt"),
            workspace.path().join("escape.txt"),
        )
        .unwrap();

        let fs = SimplicioRuntimeFs::new(workspace.path());
        let error = fs
            .relative_path(Path::new("escape.txt"))
            .expect_err("symlink escaping the workspace root must be denied");
        assert!(
            error.to_string().contains("symlink escape"),
            "unexpected error: {error}"
        );
    }

    /// Same as above but through a nested directory, proving the check
    /// isn't limited to a top-level symlink.
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_via_nested_directory() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        std::fs::create_dir_all(workspace.path().join("nested/dir")).unwrap();
        symlink(
            outside.path().join("secret.txt"),
            workspace.path().join("nested/dir/escape.txt"),
        )
        .unwrap();

        let fs = SimplicioRuntimeFs::new(workspace.path());
        let error = fs
            .relative_path(Path::new("nested/dir/escape.txt"))
            .expect_err("nested symlink escaping the workspace root must be denied");
        assert!(
            error.to_string().contains("symlink escape"),
            "unexpected error: {error}"
        );
    }

    // Windows equivalents of the three Unix symlink tests above, using
    // `std::os::windows::fs::symlink_file`. Symlink creation on Windows
    // needs either Developer Mode or an elevated process; when neither is
    // available the call fails with a permissions error unrelated to the
    // sandboxing logic under test, so these skip (rather than fail) in that
    // case instead of reporting a false negative for the security check.

    #[cfg(windows)]
    #[test]
    fn allows_symlink_that_stays_inside_workspace_windows() {
        use std::os::windows::fs::symlink_file;

        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("real.rs"), b"fn real() {}").unwrap();
        if symlink_file(
            workspace.path().join("real.rs"),
            workspace.path().join("alias.rs"),
        )
        .is_err()
        {
            eprintln!("skipping: symlink creation unavailable (no Developer Mode/elevation)");
            return;
        }
        let fs = SimplicioRuntimeFs::new(workspace.path());
        assert!(fs.relative_path(Path::new("alias.rs")).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_symlink_escape_to_outside_target_windows() {
        use std::os::windows::fs::symlink_file;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        if symlink_file(
            outside.path().join("secret.txt"),
            workspace.path().join("escape.txt"),
        )
        .is_err()
        {
            eprintln!("skipping: symlink creation unavailable (no Developer Mode/elevation)");
            return;
        }

        let fs = SimplicioRuntimeFs::new(workspace.path());
        let error = fs
            .relative_path(Path::new("escape.txt"))
            .expect_err("symlink escaping the workspace root must be denied");
        assert!(
            error.to_string().contains("symlink escape"),
            "unexpected error: {error}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_symlink_escape_via_nested_directory_windows() {
        use std::os::windows::fs::symlink_file;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        std::fs::create_dir_all(workspace.path().join("nested/dir")).unwrap();
        if symlink_file(
            outside.path().join("secret.txt"),
            workspace.path().join("nested/dir/escape.txt"),
        )
        .is_err()
        {
            eprintln!("skipping: symlink creation unavailable (no Developer Mode/elevation)");
            return;
        }

        let fs = SimplicioRuntimeFs::new(workspace.path());
        let error = fs
            .relative_path(Path::new("nested/dir/escape.txt"))
            .expect_err("nested symlink escaping the workspace root must be denied");
        assert!(
            error.to_string().contains("symlink escape"),
            "unexpected error: {error}"
        );
    }

    /// Full product seam: Code performs the real Agent handshake, sends the
    /// filesystem calls over the real Loop Hub socket, and Loop owns the real
    /// Runtime MCP process. Run with `--ignored` and the three explicit
    /// environment variables; the default suite remains dependency-free.
    #[tokio::test]
    #[ignore = "requires real Agent Host, Loop Hub, and Runtime processes"]
    async fn real_agent_code_loop_runtime_filesystem_e2e() {
        let agent_socket = std::env::var("SIMPLICIO_AGENT_SOCKET")
            .expect("SIMPLICIO_AGENT_SOCKET is required for the real E2E");
        assert!(
            !std::env::var(LOOP_HUB_ENDPOINT_ENV)
                .expect("SIMPLICIO_LOOP_HUB_ENDPOINT is required for the real E2E")
                .trim()
                .is_empty()
        );

        let workspace = tempfile::tempdir().unwrap();
        let fs = SimplicioRuntimeFs::with_agent_socket(workspace.path(), agent_socket);
        fs.write_file(Path::new("probe.txt"), b"hello code")
            .await
            .unwrap();
        assert_eq!(
            fs.read_file(Path::new("probe.txt")).await.unwrap(),
            b"hello code"
        );
        assert_eq!(
            fs.stat_workspace(Path::new("probe.txt")).await.unwrap()["type"],
            "file"
        );
        let listing = fs
            .list_workspace(Path::new("."), json!({"hidden": false}))
            .await
            .unwrap();
        assert!(
            listing["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| { entry["path"] == "probe.txt" })
        );
        let search = fs
            .search("hello", None, &[], false, true, 20, 20)
            .await
            .unwrap();
        assert_eq!(search.matches.len(), 1);
        let plan = json!({
            "file": "probe.txt",
            "operations": [{"op": "append", "text": "\nedit"}]
        });
        fs.apply_edit(plan).await.unwrap();
        assert_eq!(
            fs.read_workspace(Path::new("probe.txt")).await.unwrap(),
            "hello code\nedit"
        );
        fs.delete_file(Path::new("probe.txt")).await.unwrap();
        assert!(!workspace.path().join("probe.txt").exists());
    }
}
