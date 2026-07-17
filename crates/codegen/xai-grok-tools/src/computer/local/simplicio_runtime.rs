use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use simplicio_runtime_client::{DEFAULT_MAX_FILE_BYTES, RuntimeClient, start_workspace_map};

use crate::computer::{
    local::LocalFs,
    types::{AsyncFileSystem, ComputerError},
};

/// Project filesystem whose reads are owned by the Simplicio Runtime.
///
/// Reads fail closed: there is intentionally no direct-local fallback.
pub struct SimplicioRuntimeFs {
    root: PathBuf,
    client: Arc<Mutex<Option<RuntimeClient>>>,
    local_writes: LocalFs,
}

impl SimplicioRuntimeFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        if let Err(error) = start_workspace_map(&root) {
            tracing::warn!(%error, workspace = %root.display(), "Simplicio Mapper bootstrap failed");
        }
        Self {
            root,
            client: Arc::new(Mutex::new(None)),
            local_writes: LocalFs,
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
        normalized
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                ComputerError::io(format!(
                    "Simplicio Runtime denied read outside workspace: {}",
                    path.display()
                ))
            })
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for SimplicioRuntimeFs {
    #[tracing::instrument(name = "simplicio_runtime.fs.read_file", skip_all)]
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        let root = self.root.clone();
        let relative = self.relative_path(path)?;
        let client = Arc::clone(&self.client);
        tokio::task::spawn_blocking(move || {
            let mut guard = client
                .lock()
                .map_err(|_| ComputerError::io("Simplicio Runtime client lock poisoned"))?;
            if guard.is_none() {
                *guard = Some(
                    RuntimeClient::spawn_in(&root).map_err(|e| ComputerError::io(e.to_string()))?,
                );
            }
            let result = guard.as_mut().expect("runtime initialized").read_file(
                &root,
                &relative,
                DEFAULT_MAX_FILE_BYTES,
            );
            match result {
                Ok(read) => Ok(read.content.into_bytes()),
                Err(error) => {
                    *guard = None;
                    Err(ComputerError::io(error.to_string()))
                }
            }
        })
        .await
        .map_err(|e| ComputerError::io(format!("Simplicio Runtime task failed: {e}")))?
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        self.local_writes.write_file(path, data).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        self.local_writes.delete_file(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_escape() {
        let fs = SimplicioRuntimeFs::new("/workspace");
        assert!(fs.relative_path(Path::new("/outside/secret")).is_err());
        assert!(fs.relative_path(Path::new("../outside/secret")).is_err());
    }
}
