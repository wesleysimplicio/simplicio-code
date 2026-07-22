//! Read-only filesystem helpers backing the client-facing
//! `workspace.client_fs_*` RPCs (the grok.com conversation-files UI,
//! tunneled through the server).
//!
//! Deliberately separate from the shell-facing ext ops in
//! [`ext_fs`](super::ext_fs): every path here is workspace-root-relative
//! and delegates list/stat authority to the Simplicio Runtime. Reads remain
//! binary-safe (base64 chunks) and use the workspace root-confinement helper
//! for the legacy read path.
//!
//! Wire types live in `xai_grok_workspace_types::rpc::fs` (the
//! `ClientFs*` types), shared with the backend caller.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use xai_grok_workspace_types::rpc::fs::{
    ClientFsListNode as FsListNode, ClientFsListReq as FsListReq, ClientFsListRes as FsListRes,
    ClientFsReadFileReq as FsReadFileReq, ClientFsReadFileRes as FsReadFileRes,
    ClientFsStatReq as FsStatReq, ClientFsStatRes as FsStatRes, FsContentType, FsNodeType,
};

use crate::error::{WorkspaceError, WorkspaceResult};
use crate::handle::WorkspaceHandle;
use xai_grok_tools::computer::local::SimplicioRuntimeFs;

/// Server-side cap on `FsListReq::limit`.
const MAX_LIST_LIMIT: u32 = 1000;

/// Server-side cap on a single read's effective byte budget (shared
/// across all fs surfaces; see [`super::walk::MAX_READ_BYTES`]). Only
/// referenced by tests now that the clamp lives in `walk::clamp_read_length`.
#[cfg(test)]
const MAX_READ_BYTES: u64 = super::walk::MAX_READ_BYTES;

/// Bound on memoized hashes; the memo is cleared (not LRU-evicted) when
/// full — entries simply re-hash on next use.
const HASH_MEMO_CAPACITY: usize = 4096;

// =========================================================================
// (path, size, mtime_ms) → hash memo
// =========================================================================

#[derive(Debug, Clone)]
struct MemoEntry {
    size: u64,
    mtime_ms: i64,
    hash: String,
}

/// Memo of full-content SHA-256 digests keyed by absolute path and
/// validated against `(size, mtime_ms)`, so unchanged files hash once
/// instead of on every `client_fs_stat`. The memo only avoids redundant
/// hashing — it never substitutes mtime for content addressing: a
/// `(size, mtime_ms)` mismatch is a miss and the caller re-hashes.
#[derive(Debug, Default)]
pub(crate) struct FileHashMemo {
    entries: parking_lot::Mutex<HashMap<PathBuf, MemoEntry>>,
}

impl FileHashMemo {
    /// Return the memoized hash when `(size, mtime_ms)` still match.
    pub(crate) fn lookup(&self, path: &Path, size: u64, mtime_ms: i64) -> Option<String> {
        let entries = self.entries.lock();
        let entry = entries.get(path)?;
        (entry.size == size && entry.mtime_ms == mtime_ms).then(|| entry.hash.clone())
    }

    /// Record a freshly computed hash, replacing any stale entry for the
    /// same path. Clears the whole memo when inserting a new path would
    /// exceed [`HASH_MEMO_CAPACITY`].
    pub(crate) fn store(&self, path: &Path, size: u64, mtime_ms: i64, hash: String) {
        let mut entries = self.entries.lock();
        if !entries.contains_key(path) && entries.len() >= HASH_MEMO_CAPACITY {
            entries.clear();
        }
        entries.insert(
            path.to_path_buf(),
            MemoEntry {
                size,
                mtime_ms,
                hash,
            },
        );
    }
}

// =========================================================================
// Path resolution
// =========================================================================

/// Resolve a root-relative request path through the workspace's
/// root-confinement helper, returning the resolved path together with the
/// canonical root it was checked against. `""` and `"."` mean the
/// workspace root; absolute paths, `..` escapes, and symlink escapes are
/// rejected there.
async fn resolve_with_root(
    ws: &WorkspaceHandle,
    path: &str,
) -> WorkspaceResult<(PathBuf, PathBuf)> {
    let rel = if path.is_empty() { "." } else { path };
    let canonical_root = ws.canonical_root().await?;
    let abs = ws.resolve_service_path(rel, &canonical_root).await?;
    Ok((abs, canonical_root))
}

/// [`resolve_with_root`] for callers that don't need the canonical root.
async fn resolve(ws: &WorkspaceHandle, path: &str) -> WorkspaceResult<PathBuf> {
    resolve_with_root(ws, path).await.map(|(abs, _)| abs)
}

fn system_time_ms(st: std::time::SystemTime) -> i64 {
    match st.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

// =========================================================================
// list
// =========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeListNode {
    name: String,
    path: String,
    #[serde(rename = "type", alias = "nodeType", alias = "kind")]
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
    #[serde(default)]
    node_type: Option<FsNodeType>,
    #[serde(rename = "type", alias = "kind", default)]
    legacy_node_type: Option<FsNodeType>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mtime_ms: Option<i64>,
    #[serde(default)]
    hash: Option<String>,
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

fn runtime_list_response(value: Value) -> WorkspaceResult<FsListRes> {
    let response: RuntimeListResponse =
        serde_json::from_value(runtime_payload(value)?).map_err(|error| {
            WorkspaceError::HubError(format!("invalid Runtime list response: {error}"))
        })?;
    let nodes = response
        .nodes
        .into_iter()
        .map(|node| {
            let node_type = match node.node_type.as_str() {
                "directory" => FsNodeType::Directory,
                "file" => FsNodeType::File,
                other => {
                    return Err(WorkspaceError::HubError(format!(
                        "invalid Runtime list node type: {other}"
                    )));
                }
            };
            Ok(FsListNode {
                name: node.name,
                path: node.path,
                node_type,
                is_symlink: node.is_symlink,
                size: node.size,
                mtime_ms: node.mtime_ms.or_else(|| {
                    node.modified_at.and_then(|modified_at| {
                        chrono::DateTime::parse_from_rfc3339(&modified_at)
                            .ok()
                            .map(|timestamp| timestamp.timestamp_millis())
                    })
                }),
            })
        })
        .collect::<WorkspaceResult<Vec<_>>>()?;
    Ok(FsListRes {
        nodes,
        truncated: response.truncated,
    })
}

fn runtime_stat_response(value: Value) -> WorkspaceResult<FsStatRes> {
    let response: RuntimeStatResponse =
        serde_json::from_value(runtime_payload(value)?).map_err(|error| {
            WorkspaceError::HubError(format!("invalid Runtime stat response: {error}"))
        })?;
    Ok(FsStatRes {
        exists: response
            .exists
            .unwrap_or_else(|| response.node_type.is_some() || response.legacy_node_type.is_some()),
        node_type: response.node_type.or(response.legacy_node_type),
        size: response.size,
        mtime_ms: response.mtime_ms,
        hash: response.hash,
    })
}

fn is_missing_path_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such file")
        || error.contains("path not found")
        || error.contains("file not found")
        || error.contains("resource_not_found")
}

/// List `req.path` through the Simplicio Runtime. The Code process does not
/// walk or stat workspace entries locally; an unavailable/incompatible
/// Runtime is returned as an error without a local fallback.
pub(crate) async fn list(ws: &WorkspaceHandle, req: &FsListReq) -> WorkspaceResult<FsListRes> {
    let runtime = SimplicioRuntimeFs::new(ws.root_cwd()?.to_path_buf());
    let path = if req.path.is_empty() {
        Path::new(".")
    } else {
        Path::new(&req.path)
    };
    let value = runtime
        .list_workspace(
            path,
            json!({
                "depth": req.depth,
                "include_hidden": req.include_hidden,
                "limit": req.limit.min(MAX_LIST_LIMIT),
                "offset": req.offset,
                "follow_symlinks": req.follow_symlinks,
                "respect_git_ignore": req.respect_git_ignore,
                "include_globs": req.include_globs,
                "exclude_globs": req.exclude_globs,
            }),
        )
        .await
        .map_err(|error| {
            WorkspaceError::HubError(format!("Simplicio Runtime list denied: {error}"))
        })?;
    runtime_list_response(value)
}

// =========================================================================
// stat
// =========================================================================

/// Stat `req.path` through the Simplicio Runtime. The Runtime owns metadata
/// and content hashing; this process never probes the workspace locally.
pub(crate) async fn stat(ws: &WorkspaceHandle, req: &FsStatReq) -> WorkspaceResult<FsStatRes> {
    let runtime = SimplicioRuntimeFs::new(ws.root_cwd()?.to_path_buf());
    let path = if req.path.is_empty() {
        Path::new(".")
    } else {
        Path::new(&req.path)
    };
    match runtime.stat_workspace(path).await {
        Ok(value) => runtime_stat_response(value),
        Err(error) if is_missing_path_error(&error.to_string()) => Ok(FsStatRes {
            exists: false,
            node_type: None,
            size: None,
            mtime_ms: None,
            hash: None,
        }),
        Err(error) => Err(WorkspaceError::HubError(format!(
            "Simplicio Runtime stat denied: {error}"
        ))),
    }
}

// =========================================================================
// read_file
// =========================================================================

/// Read a byte range of `req.path` through Runtime (binary-safe, capped at
/// `min(req.max_bytes, MAX_READ_BYTES)`) together with Runtime-owned metadata.
/// Code never probes or hashes workspace files locally on this productive
/// path.
pub(crate) async fn read_file(
    ws: &WorkspaceHandle,
    req: &FsReadFileReq,
) -> WorkspaceResult<FsReadFileRes> {
    let abs = resolve(ws, &req.path).await?;
    let runtime = SimplicioRuntimeFs::new(ws.root_cwd()?.to_path_buf());
    let metadata = runtime
        .stat_workspace(&abs)
        .await
        .map_err(|error| WorkspaceError::HubError(error.to_string()))
        .and_then(runtime_stat_response)?;
    if metadata.node_type == Some(FsNodeType::Directory) {
        return Err(WorkspaceError::HubError(format!(
            "not a file: {}",
            req.path
        )));
    }
    let offset = req.offset.unwrap_or(0);
    // Server-side clamp: a hostile/buggy caller cannot lift the per-chunk
    // budget past MAX_READ_BYTES regardless of `maxBytes`.
    let length = super::walk::clamp_read_length(req.length, req.max_bytes);
    let chunk = runtime
        .read_workspace_range(
            &abs,
            Some(offset),
            Some(offset.saturating_add(length)),
            req.max_bytes as usize,
        )
        .await
        .map_err(|error| WorkspaceError::HubError(error.to_string()))?
        .bytes()
        .map_err(|error| WorkspaceError::HubError(error.to_string()))?;
    let size = metadata
        .size
        .ok_or_else(|| WorkspaceError::HubError("Runtime stat omitted file size".into()))?;
    let hash = metadata
        .hash
        .ok_or_else(|| WorkspaceError::HubError("Runtime stat omitted file hash".into()))?;

    // Shared encoder keeps the paired wire fields coherent with one UTF-8
    // validation pass; `type` is text iff the bytes were valid UTF-8.
    let (payload, is_text) = super::walk::encode_chunk(chunk, req.encoding);
    let (content, content_base64) = match payload {
        super::walk::ChunkPayload::Text(t) => (Some(t), None),
        super::walk::ChunkPayload::Base64(b) => (None, Some(b)),
    };
    let content_type = if is_text {
        FsContentType::Text
    } else {
        FsContentType::Binary
    };
    Ok(FsReadFileRes {
        content,
        content_base64,
        size,
        hash,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use xai_grok_workspace_types::rpc::fs::FsReadEncoding;

    use super::*;
    use crate::handle::tests::make_handle;

    #[test]
    fn runtime_list_response_decodes_mcp_payload() {
        let value = serde_json::json!({
            "content": [{"type": "text", "text": serde_json::json!({
                "nodes": [{
                    "name": "src",
                    "path": "src",
                    "type": "directory",
                    "mtimeMs": 42
                }],
                "truncated": false
            }).to_string()}]
        });
        let response = runtime_list_response(value).unwrap();
        assert_eq!(response.nodes[0].node_type, FsNodeType::Directory);
        assert_eq!(response.nodes[0].mtime_ms, Some(42));
    }

    #[test]
    fn runtime_stat_response_accepts_shell_type_alias() {
        let response = runtime_stat_response(serde_json::json!({
            "exists": true,
            "type": "file",
            "size": 7,
            "mtimeMs": 42,
            "hash": "abc"
        }))
        .unwrap();
        assert_eq!(response.node_type, Some(FsNodeType::File));
        assert_eq!(response.hash.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn list_fails_closed_without_runtime() {
        let ws = make_handle();
        let error = list(
            &ws,
            &FsListReq {
                path: ".".into(),
                depth: 1,
                include_hidden: true,
                limit: 10,
                offset: 0,
                follow_symlinks: true,
                respect_git_ignore: false,
                include_globs: vec![],
                exclude_globs: vec![],
            },
        )
        .await
        .expect_err("list must not fall back to local traversal");
        assert!(
            error.to_string().contains("Runtime"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn memo_lookup_hits_and_invalidates_on_mismatch() {
        let memo = FileHashMemo::default();
        let path = Path::new("/ws/a.txt");
        memo.store(path, 10, 1000, "h1".into());
        assert_eq!(memo.lookup(path, 10, 1000).as_deref(), Some("h1"));
        // Size change ⇒ miss.
        assert_eq!(memo.lookup(path, 11, 1000), None);
        // Mtime change ⇒ miss.
        assert_eq!(memo.lookup(path, 10, 2000), None);
        // Re-store replaces the stale entry.
        memo.store(path, 11, 2000, "h2".into());
        assert_eq!(memo.lookup(path, 11, 2000).as_deref(), Some("h2"));
        assert_eq!(memo.lookup(path, 10, 1000), None);
    }

    #[tokio::test]
    async fn stat_fails_closed_without_runtime() {
        let ws = make_handle();
        let error = stat(
            &ws,
            &FsStatReq {
                path: "present.txt".into(),
            },
        )
        .await
        .expect_err("stat must not fall back to local metadata");
        assert!(
            error.to_string().contains("Runtime"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn read_file_chunks_are_binary_safe_and_capped() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        // Non-UTF-8 payload: every byte value once.
        let payload: Vec<u8> = (0u8..=255).collect();
        std::fs::write(root.join("blob.bin"), &payload).unwrap();

        let req = FsReadFileReq {
            path: "blob.bin".into(),
            // Bytes 200..210 are bare continuation bytes — never valid UTF-8.
            offset: Some(200),
            length: Some(50),
            max_bytes: 10, // cap below the requested length
            encoding: FsReadEncoding::Base64,
        };
        let res = read_file(&ws, &req).await.unwrap();
        assert_eq!(res.size, 256);
        assert_eq!(res.content, None);
        assert_eq!(res.content_type, FsContentType::Binary);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(res.content_base64.unwrap())
            .unwrap();
        assert_eq!(bytes, payload[200..210], "maxBytes caps the chunk");

        // Full-file hash regardless of the requested range.
        use sha2::{Digest, Sha256};
        assert_eq!(res.hash, format!("{:x}", Sha256::digest(&payload)));

        // Memoized second read (range-only fast path) returns the
        // identical chunk + hash.
        let again = read_file(&ws, &req).await.unwrap();
        assert_eq!(again.hash, res.hash);
        let again_bytes = base64::engine::general_purpose::STANDARD
            .decode(again.content_base64.unwrap())
            .unwrap();
        assert_eq!(again_bytes, payload[200..210]);
    }

    /// `maxBytes` is server-capped at [`MAX_READ_BYTES`]:
    /// a caller-supplied huge budget cannot make the workspace buffer the
    /// whole file.
    #[tokio::test]
    async fn read_file_server_caps_max_bytes() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let payload = vec![0u8; (MAX_READ_BYTES + 100) as usize];
        std::fs::write(root.join("big.bin"), &payload).unwrap();

        let res = read_file(
            &ws,
            &FsReadFileReq {
                path: "big.bin".into(),
                offset: None,
                length: None,
                max_bytes: u64::MAX,
                encoding: FsReadEncoding::Base64,
            },
        )
        .await
        .unwrap();
        assert_eq!(res.size, payload.len() as u64);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(res.content_base64.unwrap())
            .unwrap();
        assert_eq!(
            bytes.len() as u64,
            MAX_READ_BYTES,
            "clamped to the server cap"
        );
    }

    #[tokio::test]
    async fn read_file_utf8_default_and_binary_fallback() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        std::fs::write(root.join("text.txt"), "héllo").unwrap();
        std::fs::write(root.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();

        let text = read_file(
            &ws,
            &FsReadFileReq {
                path: "text.txt".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Utf8,
            },
        )
        .await
        .unwrap();
        assert_eq!(text.content.as_deref(), Some("héllo"));
        assert_eq!(text.content_base64, None);
        assert_eq!(text.content_type, FsContentType::Text);

        // Invalid UTF-8 under the utf8 default degrades to base64.
        let bin = read_file(
            &ws,
            &FsReadFileReq {
                path: "bin.dat".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Utf8,
            },
        )
        .await
        .unwrap();
        assert_eq!(bin.content, None);
        assert_eq!(bin.content_type, FsContentType::Binary);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bin.content_base64.unwrap())
            .unwrap();
        assert_eq!(bytes, [0xff, 0xfe, 0x00]);
    }

    #[tokio::test]
    async fn resolve_rejects_escapes() {
        let ws = make_handle();
        for path in ["/etc/passwd", "../escape.txt"] {
            let err = stat(
                &ws,
                &FsStatReq {
                    path: path.to_owned(),
                },
            )
            .await
            .expect_err("escape must be rejected");
            assert!(matches!(err, WorkspaceError::HubError(_)), "{err:?}");
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn resolve_rejects_symlink_escape() {
        let ws = make_handle();
        let root = ws.root_cwd().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape_link")).unwrap();

        let err = read_file(
            &ws,
            &FsReadFileReq {
                path: "escape_link/secret.txt".into(),
                offset: None,
                length: None,
                max_bytes: 1_048_576,
                encoding: FsReadEncoding::Base64,
            },
        )
        .await
        .expect_err("symlink escape must be rejected");
        assert!(
            err.to_string().contains("symlink escape"),
            "unexpected error: {err}"
        );
    }
}
