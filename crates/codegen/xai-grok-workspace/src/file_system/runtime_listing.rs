//! Typed, fail-closed access to Runtime-owned workspace directory listings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use xai_grok_tools::types::resources::AsyncDirectoryListing;

use super::FsError;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type", alias = "nodeType", alias = "kind")]
    pub node_type: String,
}

#[derive(Deserialize)]
struct RuntimePage {
    #[serde(alias = "entries")]
    nodes: Vec<RuntimeEntry>,
    #[serde(default)]
    truncated: bool,
}

pub(crate) async fn collect_runtime_entries(
    backend: Arc<dyn AsyncDirectoryListing>,
    root: &Path,
    depth: usize,
    include_hidden: bool,
    max_entries: usize,
) -> Result<Vec<RuntimeEntry>, FsError> {
    const PAGE_SIZE: usize = 1000;
    let mut entries = Vec::new();
    loop {
        let remaining = max_entries.saturating_sub(entries.len());
        if remaining == 0 {
            break;
        }
        let limit = remaining.min(PAGE_SIZE);
        let value = backend
            .list_directory(
                root,
                json!({
                    "depth": depth,
                    "include_hidden": include_hidden,
                    "limit": limit,
                    "offset": entries.len(),
                    "follow_symlinks": false,
                    "respect_git_ignore": true,
                    "include_globs": [],
                    "exclude_globs": [".git"],
                }),
            )
            .await
            .map_err(|error| FsError::Other(format!("Simplicio Runtime list denied: {error}")))?;
        let value = if let Some(text) = value
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|items| items.iter().find_map(|item| item.get("text")))
            .and_then(|v| v.as_str())
        {
            serde_json::from_str(text).map_err(|error| {
                FsError::Other(format!("invalid Runtime filesystem JSON: {error}"))
            })?
        } else {
            value
        };
        let page: RuntimePage = serde_json::from_value(value)
            .map_err(|error| FsError::Other(format!("invalid Runtime list response: {error}")))?;
        let count = page.nodes.len();
        entries.extend(page.nodes);
        if !page.truncated || count == 0 {
            break;
        }
    }
    Ok(entries)
}

pub(crate) fn absolute_entry_path(root: &Path, entry: &RuntimeEntry) -> PathBuf {
    let path = Path::new(&entry.path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}
