//! Runtime-owned directory listing adapter.
//!
//! Kept in a separate module so the edit/list-stat implementation files stay
//! disjoint from this issue-49 search/tree-walk slice.

use std::path::Path;

use serde_json::Value;

use super::simplicio_runtime::SimplicioRuntimeFs;
use crate::types::resources::AsyncDirectoryListing;

/// Decode the JSON returned by the MCP transport. Runtime filesystem tools
/// return their versioned payload directly in tests and as `content[].text`
/// through the real MCP bridge; both forms are part of the boundary adapter.
pub(crate) fn runtime_payload(value: Value) -> Result<Value, String> {
    let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.iter().find_map(|item| item.get("text")))
        .and_then(Value::as_str)
    else {
        return Ok(value);
    };
    serde_json::from_str(text)
        .map_err(|error| format!("Simplicio Runtime returned invalid filesystem JSON: {error}"))
}

#[async_trait::async_trait]
impl AsyncDirectoryListing for SimplicioRuntimeFs {
    async fn list_directory(
        &self,
        path: &Path,
        options: serde_json::Value,
    ) -> Result<serde_json::Value, crate::computer::types::ComputerError> {
        self.list_workspace(path, options).await
    }
}
