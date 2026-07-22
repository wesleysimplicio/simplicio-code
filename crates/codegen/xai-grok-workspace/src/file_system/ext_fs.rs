//! Filesystem extension ops (`workspace.fs_*`) — the server-proxied backing
//! for the shell's `x.ai/fs/*` ACP extension methods.
//!
//! These mirror the pure functions that previously lived only in the
//! shell (`xai-grok-shell/src/session/file_system.rs`) so that, in proxy
//! mode, a `x.ai/fs/*` request executes on the *remote* workspace server
//! instead of the agent host. Each request type implements
//! [`WorkspaceOp`], so it runs in-process for local sessions and routes
//! over the server `workspace_rpc` tool for proxy sessions — identical wire
//! output either way.
//!
//! Path resolution: an absolute `path` is used directly; a relative
//! `path` is joined onto `cwd` (the per-session cwd the shell resolves
//! and sends) or, when absent, the workspace root.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{WorkspaceError, WorkspaceResult};
use crate::handle::WorkspaceHandle;
use crate::workspace_ops::WorkspaceOp;
use xai_grok_tools::computer::local::SimplicioRuntimeFs;

// Canonical in xai-grok-workspace-types; re-exported for existing paths.
use xai_grok_workspace_types::rpc::fs::FsReadEncoding;
pub use xai_grok_workspace_types::rpc::fs::{
    FsDeleteFileReq, FsExistsData, FsExistsReq, FsListData, FsListNode, FsListReq, FsReadFileData,
    FsReadFileReq, FsWriteFileReq,
};

/// Resolve a request `path` to an absolute path. Absolute paths are used
/// directly; relative paths join `cwd` (the shell-resolved per-session
/// cwd) or, when absent, the workspace root.
fn resolve_abs(
    path: &str,
    cwd: &Option<PathBuf>,
    ws: &WorkspaceHandle,
) -> WorkspaceResult<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let base = match cwd {
        Some(c) => c.clone(),
        None => ws.root_cwd()?,
    };
    Ok(base.join(p))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeListNode {
    name: String,
    path: String,
    #[serde(rename = "type", alias = "nodeType")]
    node_type: String,
    #[serde(default)]
    is_symlink: Option<bool>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mtime_ms: Option<i64>,
    #[serde(default)]
    modified_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimeListResponse {
    #[serde(alias = "entries")]
    nodes: Vec<RuntimeListNode>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatResponse {
    #[serde(default)]
    exists: Option<bool>,
    #[serde(rename = "type", alias = "nodeType", default)]
    node_type: Option<String>,
}

fn runtime_payload(value: Value) -> WorkspaceResult<Value> {
    let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.iter().find_map(|item| item.get("text")))
        .and_then(Value::as_str)
    else {
        return Ok(value);
    };
    serde_json::from_str(text).map_err(|error| {
        WorkspaceError::HubError(format!(
            "Simplicio Runtime returned invalid filesystem JSON: {error}"
        ))
    })
}

fn runtime_list_response(value: Value, root: &Path) -> WorkspaceResult<FsListData> {
    let response: RuntimeListResponse =
        serde_json::from_value(runtime_payload(value)?).map_err(|error| {
            WorkspaceError::HubError(format!("invalid Runtime list response: {error}"))
        })?;
    let nodes = response
        .nodes
        .into_iter()
        .map(|node| {
            let modified_at = node.modified_at.or_else(|| {
                node.mtime_ms.and_then(|mtime_ms| {
                    chrono::DateTime::from_timestamp_millis(mtime_ms)
                        .map(|timestamp| timestamp.to_rfc3339())
                })
            });
            let path = Path::new(&node.path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            Ok(FsListNode {
                name: node.name,
                path: path.to_string_lossy().into_owned(),
                node_type: node.node_type,
                is_symlink: node.is_symlink,
                size: node.size,
                modified_at,
            })
        })
        .collect::<WorkspaceResult<Vec<_>>>()?;
    Ok(FsListData {
        nodes,
        truncated: response.truncated,
    })
}

fn runtime_exists(value: Value) -> WorkspaceResult<bool> {
    let response: RuntimeStatResponse =
        serde_json::from_value(runtime_payload(value)?).map_err(|error| {
            WorkspaceError::HubError(format!("invalid Runtime stat response: {error}"))
        })?;
    Ok(response
        .exists
        .unwrap_or_else(|| response.node_type.is_some()))
}

fn is_missing_path_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such file")
        || error.contains("path not found")
        || error.contains("file not found")
        || error.contains("resource_not_found")
}

#[async_trait]
impl WorkspaceOp for FsListReq {
    async fn execute(
        &self,
        ws: &WorkspaceHandle,
        _session_id: Option<&str>,
    ) -> WorkspaceResult<Self::Response> {
        let path = resolve_abs(&self.path, &self.cwd, ws)?;
        let root = ws.root_cwd()?.to_path_buf();
        let runtime = SimplicioRuntimeFs::new(root.clone());
        let value = runtime
            .list_workspace(
                &path,
                json!({
                    "depth": self.depth,
                    "include_hidden": self.include_hidden,
                    "limit": self.limit,
                    "offset": self.offset,
                    "follow_symlinks": self.follow_symlinks,
                    "respect_git_ignore": self.respect_git_ignore,
                    "include_globs": self.include_globs,
                    "exclude_globs": self.exclude_globs,
                }),
            )
            .await
            .map_err(|error| {
                WorkspaceError::HubError(format!("Simplicio Runtime list denied: {error}"))
            })?;
        runtime_list_response(value, &root)
    }
}

#[async_trait]
impl WorkspaceOp for FsExistsReq {
    async fn execute(
        &self,
        ws: &WorkspaceHandle,
        _session_id: Option<&str>,
    ) -> WorkspaceResult<Self::Response> {
        let path = resolve_abs(&self.path, &self.cwd, ws)?;
        let runtime = SimplicioRuntimeFs::new(ws.root_cwd()?.to_path_buf());
        let exists = match runtime.stat_workspace(&path).await {
            Ok(value) => runtime_exists(value)?,
            Err(error) if is_missing_path_error(&error.to_string()) => false,
            Err(error) => {
                return Err(WorkspaceError::HubError(format!(
                    "Simplicio Runtime stat denied: {error}"
                )));
            }
        };
        Ok(FsExistsData { exists })
    }
}

#[async_trait]
impl WorkspaceOp for FsReadFileReq {
    async fn execute(
        &self,
        ws: &WorkspaceHandle,
        _session_id: Option<&str>,
    ) -> WorkspaceResult<Self::Response> {
        let abs_unconfined = resolve_abs(&self.path, &self.cwd, ws)?;
        let (abs, _) = ws.confine_to_workspace_root(&abs_unconfined).await?;

        // Legacy full-file read path: preserves the pre-range wire output
        // (auto utf8/base64 detect, MIME `type`, `lineCount`).
        let ranged = self.offset.is_some()
            || self.length.is_some()
            || self.encoding == FsReadEncoding::Base64;
        if !ranged {
            let bytes = tokio::fs::read(&abs)
                .await
                .map_err(|e| WorkspaceError::HubError(e.to_string()))?;
            return Ok(build_file_entry(&bytes));
        }

        // Binary-safe ranged read: `size` is the full file size, the
        // chunk is `[offset, offset + min(length, max_bytes, cap))`.
        let md = tokio::fs::metadata(&abs)
            .await
            .map_err(|e| WorkspaceError::HubError(e.to_string()))?;
        if md.is_dir() {
            return Err(WorkspaceError::HubError(format!(
                "not a file: {}",
                self.path
            )));
        }
        // Best-effort snapshot: a concurrent truncate/grow between here and
        // read_range can make `size` inconsistent with the returned chunk.
        let size = md.len();
        let offset = self.offset.unwrap_or(0);
        let length = super::walk::clamp_read_length(self.length, self.max_bytes);
        let chunk = super::walk::read_range(&abs, offset, length)
            .await
            .map_err(|e| WorkspaceError::HubError(e.to_string()))?;
        Ok(build_ranged_entry(chunk, size, self.encoding))
    }
}

#[async_trait]
impl WorkspaceOp for FsWriteFileReq {
    async fn execute(
        &self,
        ws: &WorkspaceHandle,
        _session_id: Option<&str>,
    ) -> WorkspaceResult<Self::Response> {
        let abs_unconfined = resolve_abs(&self.path, &self.cwd, ws)?;
        let (abs, _) = ws.confine_to_workspace_root(&abs_unconfined).await?;
        let content = self.content.clone();
        let create_dirs = self.create_dirs;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if create_dirs && let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, content.as_bytes())
        })
        .await
        .map_err(|e| WorkspaceError::HubError(e.to_string()))?
        .map_err(|e| WorkspaceError::HubError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl WorkspaceOp for FsDeleteFileReq {
    async fn execute(
        &self,
        ws: &WorkspaceHandle,
        _session_id: Option<&str>,
    ) -> WorkspaceResult<Self::Response> {
        let abs_unconfined = resolve_abs(&self.path, &self.cwd, ws)?;
        let (abs, _) = ws.confine_to_workspace_root(&abs_unconfined).await?;
        tokio::fs::remove_file(&abs)
            .await
            .map_err(|e| WorkspaceError::HubError(e.to_string()))?;
        Ok(())
    }
}

/// Map a binary-safe ranged chunk to the shell-facing `FsReadFileData`.
/// `size` is the full file size; `lineCount` is omitted for ranged reads
/// and the MIME `type` is a coarse text/binary tag (mid-file chunks make
/// magic-byte sniffing meaningless).
fn build_ranged_entry(chunk: Vec<u8>, size: u64, encoding: FsReadEncoding) -> FsReadFileData {
    let (payload, is_text) = super::walk::encode_chunk(chunk, encoding);
    let (content, content_base64) = match payload {
        super::walk::ChunkPayload::Text(t) => (t, None),
        super::walk::ChunkPayload::Base64(b) => (String::new(), Some(b)),
    };
    FsReadFileData {
        content,
        content_base64,
        size,
        line_count: None,
        content_type: if is_text {
            "text/plain".to_string()
        } else {
            "application/octet-stream".to_string()
        },
    }
}

fn build_file_entry(bytes: &[u8]) -> FsReadFileData {
    let size = bytes.len() as u64;
    let inferred = infer::get(bytes).map(|t| t.mime_type().to_string());
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => FsReadFileData {
            line_count: Some(text.lines().count() as u64),
            content: text,
            content_base64: None,
            size,
            content_type: inferred.unwrap_or_else(|| "text/plain".to_string()),
        },
        Err(_) => FsReadFileData {
            content: String::new(),
            content_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            size,
            line_count: None,
            content_type: inferred.unwrap_or_else(|| "application/octet-stream".to_string()),
        },
    }
}

// =========================================================================
// Tests for the pure helpers (no `WorkspaceHandle` required).
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::tests::make_handle;

    #[test]
    fn build_file_entry_utf8_sets_content_and_line_count() {
        let bytes = b"line one\nline two\n";
        let entry = build_file_entry(bytes);
        assert_eq!(entry.content, "line one\nline two\n");
        assert!(entry.content_base64.is_none());
        assert_eq!(entry.line_count, Some(2));
        assert_eq!(entry.size, bytes.len() as u64);
    }

    #[test]
    fn build_file_entry_invalid_utf8_uses_base64() {
        let bytes: &[u8] = &[0xff, 0xfe, 0x00];
        let entry = build_file_entry(bytes);
        assert!(entry.content.is_empty());
        assert_eq!(
            entry.content_base64,
            Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
        assert!(entry.line_count.is_none());
        assert_eq!(entry.size, bytes.len() as u64);
    }

    #[test]
    fn runtime_list_response_decodes_mcp_payload_and_absolutizes_paths() {
        let root = Path::new("/workspace");
        let value = serde_json::json!({
            "content": [{"type": "text", "text": serde_json::json!({
                "nodes": [{
                    "name": "main.rs",
                    "path": "src/main.rs",
                    "type": "file",
                    "mtimeMs": 42
                }],
                "truncated": false
            }).to_string()}]
        });
        let response = runtime_list_response(value, root).unwrap();
        assert_eq!(response.nodes[0].path, "/workspace/src/main.rs");
        assert_eq!(
            response.nodes[0].modified_at.as_deref(),
            Some("1970-01-01T00:00:00.042+00:00")
        );
    }

    #[tokio::test]
    async fn list_and_exists_fail_closed_without_runtime() {
        let ws = make_handle();
        let list_error = FsListReq {
            path: ".".into(),
            cwd: None,
            depth: 1,
            limit: 10,
            offset: 0,
            include_hidden: true,
            follow_symlinks: true,
            respect_git_ignore: false,
            include_globs: vec![],
            exclude_globs: vec![],
        }
        .execute(&ws, None)
        .await
        .expect_err("list must not fall back to local traversal");
        assert!(list_error.to_string().contains("Runtime"), "{list_error}");

        let stat_error = FsExistsReq {
            path: "present.txt".into(),
            cwd: None,
        }
        .execute(&ws, None)
        .await
        .expect_err("exists must not fall back to local metadata");
        assert!(stat_error.to_string().contains("Runtime"), "{stat_error}");
    }

    #[test]
    fn build_ranged_entry_encodes_utf8_and_binary() {
        // UTF-8 chunk under the default encoding → `content`, text/plain,
        // full `size` echoed, no line count.
        let e = build_ranged_entry(b"hello".to_vec(), 100, FsReadEncoding::Utf8);
        assert_eq!(e.content, "hello");
        assert!(e.content_base64.is_none());
        assert_eq!(e.size, 100);
        assert!(e.line_count.is_none());
        assert_eq!(e.content_type, "text/plain");

        // Explicit base64 of valid UTF-8 stays text/plain but travels in
        // `contentBase64`.
        let e = build_ranged_entry(b"hi".to_vec(), 2, FsReadEncoding::Base64);
        assert!(e.content.is_empty());
        assert_eq!(
            e.content_base64,
            Some(base64::engine::general_purpose::STANDARD.encode(b"hi")),
        );
        assert_eq!(e.content_type, "text/plain");

        // Non-UTF-8 bytes fall back to base64 + octet-stream.
        let raw = vec![0xff_u8, 0x00, 0xfe];
        let e = build_ranged_entry(raw.clone(), 3, FsReadEncoding::Utf8);
        assert!(e.content.is_empty());
        assert_eq!(
            e.content_base64,
            Some(base64::engine::general_purpose::STANDARD.encode(&raw)),
        );
        assert_eq!(e.content_type, "application/octet-stream");
    }

    // Confinement (WorkspaceOp::execute) — covers both local and proxy dispatch.

    #[tokio::test]
    async fn read_write_within_root_ok() {
        let ws = crate::handle::tests::make_confining_handle();
        let root = ws.root_cwd().unwrap();
        FsWriteFileReq {
            path: "sub/data.txt".into(),
            cwd: Some(root.clone()),
            content: "hello".into(),
            create_dirs: true,
        }
        .execute(&ws, None)
        .await
        .expect("in-root write must succeed");
        let data = FsReadFileReq {
            path: "sub/data.txt".into(),
            cwd: Some(root.clone()),
            offset: None,
            length: None,
            max_bytes: 1 << 20,
            encoding: FsReadEncoding::Utf8,
        }
        .execute(&ws, None)
        .await
        .expect("in-root read must succeed");
        assert_eq!(data.content, "hello");
    }

    #[tokio::test]
    async fn read_file_rejects_absolute_escape() {
        let ws = crate::handle::tests::make_confining_handle();
        let err = FsReadFileReq {
            path: "/etc/passwd".into(),
            cwd: None,
            offset: None,
            length: None,
            max_bytes: 1 << 20,
            encoding: FsReadEncoding::Utf8,
        }
        .execute(&ws, None)
        .await
        .expect_err("absolute escape must be rejected");
        assert!(
            err.to_string().contains("workspace root"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn read_file_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let ws = crate::handle::tests::make_confining_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        symlink(outside.path(), root.join("escape_link")).unwrap();

        let err = FsReadFileReq {
            path: "escape_link/secret.txt".into(),
            cwd: Some(root.clone()),
            offset: None,
            length: None,
            max_bytes: 1 << 20,
            encoding: FsReadEncoding::Utf8,
        }
        .execute(&ws, None)
        .await
        .expect_err("symlink escape must be rejected");
        assert!(
            err.to_string().contains("workspace root"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn write_file_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let ws = crate::handle::tests::make_confining_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.join("escape_link")).unwrap();

        let err = FsWriteFileReq {
            path: "escape_link/injected.txt".into(),
            cwd: Some(root.clone()),
            content: "x".into(),
            create_dirs: true,
        }
        .execute(&ws, None)
        .await
        .expect_err("symlink escape write must be rejected");
        assert!(
            err.to_string().contains("workspace root"),
            "unexpected error: {err}"
        );
        assert!(
            !outside.path().join("injected.txt").exists(),
            "write must not land outside the workspace root"
        );
    }

    // A *dangling* in-root symlink (target outside root, not yet created) must
    // not let a write escape via `open(O_CREAT)` following the link.
    #[tokio::test]
    #[cfg(unix)]
    async fn write_file_rejects_dangling_symlink_escape() {
        use std::os::unix::fs::symlink;
        let ws = crate::handle::tests::make_confining_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("new.txt");
        symlink(&target, root.join("lnk")).unwrap();

        let err = FsWriteFileReq {
            path: "lnk".into(),
            cwd: Some(root.clone()),
            content: "x".into(),
            create_dirs: true,
        }
        .execute(&ws, None)
        .await
        .expect_err("dangling symlink escape write must be rejected");
        assert!(
            err.to_string().contains("workspace root"),
            "unexpected error: {err}"
        );
        assert!(
            !target.exists(),
            "write must not create the file outside root"
        );
    }
}
