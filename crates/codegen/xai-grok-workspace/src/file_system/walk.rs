//! Runtime-backed shared filesystem primitives for directory listings and ranged reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use xai_grok_tools::computer::local::SimplicioRuntimeFs;
use xai_grok_tools::types::resources::AsyncDirectoryListing;
use xai_grok_workspace_types::rpc::fs::FsReadEncoding;

pub const MAX_LIST_COLLECT: usize = 50_000;
pub const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

pub fn clamp_read_length(length: Option<u64>, max_bytes: u64) -> u64 {
    length
        .unwrap_or(u64::MAX)
        .min(max_bytes)
        .min(MAX_READ_BYTES)
}

pub struct ListedEntry {
    pub name: String,
    pub abs_path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

pub struct ListPage {
    pub entries: Vec<ListedEntry>,
    pub truncated: bool,
}

pub struct ListOptions<'a> {
    pub depth: usize,
    pub follow_symlinks: bool,
    pub respect_git_ignore: bool,
    pub include_hidden: bool,
    pub include_globs: &'a [String],
    pub exclude_globs: &'a [String],
    pub offset: u64,
    pub limit: usize,
    pub confine_to_canonical_root: Option<PathBuf>,
}

#[derive(Deserialize)]
struct RuntimeList {
    #[serde(alias = "entries")]
    nodes: Vec<RuntimeNode>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeNode {
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
}

fn payload(value: Value) -> std::io::Result<Value> {
    let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find_map(|item| item.get("text")))
        .and_then(Value::as_str)
    else {
        return Ok(value);
    };
    serde_json::from_str(text).map_err(std::io::Error::other)
}

fn io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

async fn list_with(
    backend: Arc<dyn AsyncDirectoryListing>,
    root: &Path,
    abs_dir: &Path,
    opts: ListOptions<'_>,
    max_collect: usize,
) -> std::io::Result<ListPage> {
    let limit = opts.limit.min(max_collect);
    let value = backend
        .list_directory(
            abs_dir,
            json!({
                "depth": opts.depth,
                "follow_symlinks": opts.follow_symlinks,
                "respect_git_ignore": opts.respect_git_ignore,
                "include_hidden": opts.include_hidden,
                "include_globs": opts.include_globs,
                "exclude_globs": opts.exclude_globs,
                "offset": opts.offset,
                "limit": limit,
                "confine_to_canonical_root": opts.confine_to_canonical_root,
                "max_collect": max_collect,
            }),
        )
        .await
        .map_err(io_error)?;
    let response: RuntimeList = serde_json::from_value(payload(value)?).map_err(io_error)?;
    let entries = response
        .nodes
        .into_iter()
        .map(|node| {
            let path = PathBuf::from(node.path);
            ListedEntry {
                name: node.name,
                abs_path: if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                },
                is_dir: node.node_type == "directory" || node.node_type == "dir",
                is_symlink: node.is_symlink.unwrap_or(false),
                size: node.size,
                modified: node
                    .mtime_ms
                    .and_then(|ms| u64::try_from(ms).ok())
                    .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
            }
        })
        .collect();
    Ok(ListPage {
        entries,
        truncated: response.truncated,
    })
}

/// List through the productive Runtime authority. Runtime owns filtering,
/// ordering, pagination, confinement, and the collection cap.
pub fn list_directory_paged(abs_dir: &Path, opts: ListOptions<'_>, max_collect: usize) -> ListPage {
    let root = opts
        .confine_to_canonical_root
        .clone()
        .unwrap_or_else(|| abs_dir.to_path_buf());
    let abs_dir = abs_dir.to_path_buf();
    let depth = opts.depth;
    let follow_symlinks = opts.follow_symlinks;
    let respect_git_ignore = opts.respect_git_ignore;
    let include_hidden = opts.include_hidden;
    let include_globs = opts.include_globs.to_vec();
    let exclude_globs = opts.exclude_globs.to_vec();
    let offset = opts.offset;
    let limit = opts.limit;
    let confine_to_canonical_root = opts.confine_to_canonical_root;
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Simplicio Runtime directory-list runtime must build");
        let backend: Arc<dyn AsyncDirectoryListing> =
            Arc::new(SimplicioRuntimeFs::new(root.clone()));
        runtime.block_on(list_with(
            backend,
            &root,
            &abs_dir,
            ListOptions {
                depth,
                follow_symlinks,
                respect_git_ignore,
                include_hidden,
                include_globs: &include_globs,
                exclude_globs: &exclude_globs,
                offset,
                limit,
                confine_to_canonical_root,
            },
            max_collect,
        ))
    })
    .join()
    .expect("Simplicio Runtime directory-list worker must not panic")
    .expect("Simplicio Runtime directory listing failed closed")
}

pub enum ChunkPayload {
    Text(String),
    Base64(String),
}

pub fn encode_chunk(bytes: Vec<u8>, encoding: FsReadEncoding) -> (ChunkPayload, bool) {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    match (encoding, String::from_utf8(bytes)) {
        (FsReadEncoding::Utf8, Ok(text)) => (ChunkPayload::Text(text), true),
        (FsReadEncoding::Utf8, Err(e)) => (ChunkPayload::Base64(b64(e.as_bytes())), false),
        (FsReadEncoding::Base64, Ok(text)) => (ChunkPayload::Base64(b64(text.as_bytes())), true),
        (FsReadEncoding::Base64, Err(e)) => (ChunkPayload::Base64(b64(e.as_bytes())), false),
    }
}

#[async_trait]
trait RangeReader: Send + Sync {
    async fn read(&self, path: &Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>>;
}

#[async_trait]
impl RangeReader for SimplicioRuntimeFs {
    async fn read(&self, path: &Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| io_error("range overflow"))?;
        let max_bytes = usize::try_from(length).map_err(io_error)?;
        self.read_workspace_range(path, Some(offset), Some(end), max_bytes)
            .await
            .map_err(io_error)?
            .bytes()
            .map_err(io_error)
    }
}

async fn read_with(
    reader: &dyn RangeReader,
    path: &Path,
    offset: u64,
    length: u64,
) -> std::io::Result<Vec<u8>> {
    reader.read(path, offset, length).await
}

pub async fn read_range(abs: &Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
    let root = abs.parent().unwrap_or_else(|| Path::new("/"));
    read_with(&SimplicioRuntimeFs::new(root), abs, offset, length).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeList {
        calls: Mutex<Vec<Value>>,
        response: Value,
    }
    #[async_trait]
    impl AsyncDirectoryListing for FakeList {
        async fn list_directory(
            &self,
            _: &Path,
            options: Value,
        ) -> Result<Value, xai_grok_tools::computer::types::ComputerError> {
            self.calls.lock().unwrap().push(options);
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn fake_listing_preserves_runtime_wire_page_and_caps_limit() {
        let fake = Arc::new(FakeList {
            calls: Mutex::new(vec![]),
            response: json!({
                "nodes": [{"name":"a","path":"dir/a","type":"file","size":3,"mtimeMs":1000}], "truncated": true
            }),
        });
        let page = list_with(
            fake.clone(),
            Path::new("/workspace"),
            Path::new("/workspace/dir"),
            ListOptions {
                depth: 2,
                follow_symlinks: false,
                respect_git_ignore: true,
                include_hidden: false,
                include_globs: &[],
                exclude_globs: &[],
                offset: 7,
                limit: 99,
                confine_to_canonical_root: Some(PathBuf::from("/workspace")),
            },
            10,
        )
        .await
        .unwrap();
        assert!(page.truncated);
        assert_eq!(page.entries[0].abs_path, Path::new("/workspace/dir/a"));
        assert_eq!(fake.calls.lock().unwrap()[0]["limit"], 10);
        assert_eq!(fake.calls.lock().unwrap()[0]["offset"], 7);
    }

    struct FakeRead;
    #[async_trait]
    impl RangeReader for FakeRead {
        async fn read(&self, _: &Path, offset: u64, length: u64) -> std::io::Result<Vec<u8>> {
            assert_eq!((offset, length), (4, 3));
            Ok(vec![0, 255, 1])
        }
    }

    #[tokio::test]
    async fn fake_range_seam_is_binary_safe() {
        assert_eq!(
            read_with(&FakeRead, Path::new("x"), 4, 3).await.unwrap(),
            vec![0, 255, 1]
        );
    }

    #[test]
    fn clamp_read_length_preserves_hard_cap() {
        assert_eq!(clamp_read_length(None, u64::MAX), MAX_READ_BYTES);
        assert_eq!(clamp_read_length(Some(512), 1024), 512);
    }
}
