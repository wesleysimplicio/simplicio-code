use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use simplicio_agent_client::{AgentHostClient, resolve_socket_path};
use simplicio_runtime_client::{DEFAULT_MAX_FILE_BYTES, RuntimeClient, start_workspace_map};

use crate::computer::types::{
    AsyncFileSystem, AsyncSearch, ComputerError, SearchMatch, SearchOutcome,
};

/// Project filesystem whose effects are owned by the Simplicio Runtime and
/// gated on a compatible, independently running Simplicio Agent host.
///
/// Every operation requires both products and fails closed: there is
/// intentionally no direct-local or built-in-agent fallback for any of them.
pub struct SimplicioRuntimeFs {
    root: PathBuf,
    agent_socket: PathBuf,
    agent_client: Arc<Mutex<Option<AgentHostClient>>>,
    client: Arc<Mutex<Option<RuntimeClient>>>,
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
                    agent.refresh_status().map(|_| ())
                } else {
                    AgentHostClient::connect(agent_socket.clone()).map(|agent| {
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

            let mut guard = client
                .lock()
                .map_err(|_| ComputerError::io("Simplicio Runtime client lock poisoned"))?;
            if guard.is_none() {
                *guard = Some(
                    RuntimeClient::spawn_in(&root).map_err(|e| ComputerError::io(e.to_string()))?,
                );
            }
            let result = op(guard.as_mut().expect("runtime initialized"), &root);
            if result.is_err() {
                *guard = None;
            }
            result.map_err(|e| ComputerError::io(e.to_string()))
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
