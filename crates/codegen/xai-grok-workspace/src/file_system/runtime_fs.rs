//! Workspace filesystem adapter backed by the Simplicio Runtime.
//!
//! Project content must not be read, written, or deleted through the host
//! filesystem. The tools crate owns the Runtime/Agent handshake; this thin
//! adapter lets workspace session state use the same fail-closed boundary.

use std::path::{Path, PathBuf};

use crate::file_system::{AsyncFileSystem, FsError};
use xai_grok_tools::computer::types::AsyncFileSystem as ToolAsyncFileSystem;
use xai_grok_tools::computer::local::SimplicioRuntimeFs;

/// Workspace filesystem whose project effects are delegated to Runtime.
pub struct RuntimeFs {
    root: PathBuf,
    runtime: SimplicioRuntimeFs,
}

impl RuntimeFs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            runtime: SimplicioRuntimeFs::new(root.clone()),
            root,
        }
    }

    fn map_error(error: xai_grok_tools::computer::types::ComputerError) -> FsError {
        FsError::Other(format!("Simplicio Runtime denied workspace operation: {error}"))
    }

    fn is_not_found(error: &xai_grok_tools::computer::types::ComputerError) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("not found") || message.contains("resource_not_found")
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for RuntimeFs {
    fn root(&self) -> &Path {
        &self.root
    }

    async fn exists(&self, path: &Path) -> Result<bool, FsError> {
        match self.runtime.stat_workspace(path).await {
            Ok(_) => Ok(true),
            Err(error) if Self::is_not_found(&error) => Ok(false),
            Err(error) => Err(Self::map_error(error)),
        }
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.runtime
            .read_file(path)
            .await
            .map_err(Self::map_error)
    }

    async fn try_read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, FsError> {
        match self.runtime.read_file(path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if Self::is_not_found(&error) => Ok(None),
            Err(error) => Err(Self::map_error(error)),
        }
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        self.runtime
            .write_file(path, data)
            .await
            .map_err(Self::map_error)
    }

    async fn delete_file(&self, path: &Path) -> Result<(), FsError> {
        self.runtime
            .delete_file(path)
            .await
            .map_err(Self::map_error)
    }
}
