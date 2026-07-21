use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use simplicio_agent_client::{AgentHostCoordinator, CausalIdentity, resolve_socket_path};
use simplicio_runtime_client::{
    DEFAULT_MAX_FILE_BYTES, RuntimeClient, SharedRuntimeClient, start_workspace_map,
};

use crate::computer::types {
    AsyncFileSystem, AsyncSearch, BackgroundHandle, ComputerError, KillOutcome, SearchMatch,
    SearchOutcome, TaskSnapshot, TerminalBackend, TerminalRunRequest, TerminalRunResult,
};
use super::preflight::{ProductivePreflightReport, run_installed_preflight};

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
        return Err(ComputerError::io("Simplicio Runtime rejected empty command"));
    }
    Ok(argv)
}

fn runtime_error(message: impl Into<String>) -> ComputerError {
    ComputerError::io(format!("Simplicio Runtime exec failed closed: {}", message.into()))
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

impl SimplicioRuntimeFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_agent_socket(root, resolve_socket_path())
    }

    fn with_agent_socket(root: impl Into<PathBuf>, agent_socket: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            agent_socket: agent_socket.into(),
            agent_client: Arc::new(Mutex::new(None)),
            client: Arc::new(Mutex::new(None)),
        }
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
            {
                let mut agent_guard = agent_client
                    .lock()
                    .map_err(|_| ComputerError::io("Simplicio Agent client lock poisoned"))?;
                let validation = if let Some(agent) = agent_guard.as_mut() {
                    agent.ensure_ready()
                } else {
                    AgentHostCoordinator::connect_at(
                        Self::agent_profile(),
                        agent_socket.clone(),
                    )
                    .map(|agent| {
                        *agent_guard = Some(agent);
                    })
                };
                if let Err(error) = validation {
                    *agent_guard = None;
                    return Err(ComputerError::io(error.to_string()));
                }
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
        self.with_client(path, move |client, root, relative| {
            client.list(root, relative, options)
        })
        .await
    }

    /// Returns Runtime-owned metadata for one workspace-relative path. Like
    /// [`Self::list_workspace`], this remains fail-closed on either missing
    /// dependency and performs no local filesystem fallback.
    pub async fn stat_workspace(&self, path: &Path) -> Result<serde_json::Value, ComputerError> {
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
        self.with_client(path, |client, root, relative| {
            client
                .read_file(root, relative, DEFAULT_MAX_FILE_BYTES)
                .map(|read| read.content)
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
                1_000,
                1_024,
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
}
