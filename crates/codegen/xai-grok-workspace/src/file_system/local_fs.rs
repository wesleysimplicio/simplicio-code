use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::file_system::{AsyncFileSystem, FsError, RuntimeFs};

pub struct LocalFs {
    root: PathBuf,
    backend: Arc<dyn AsyncFileSystem>,
}

impl LocalFs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            backend: Arc::new(RuntimeFs::new(root.clone())),
            root,
        }
    }

    #[cfg(test)]
    fn with_backend(root: PathBuf, backend: Arc<dyn AsyncFileSystem>) -> Self {
        Self { root, backend }
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for LocalFs {
    fn root(&self) -> &Path {
        &self.root
    }

    async fn exists(&self, path: &Path) -> Result<bool, FsError> {
        self.backend.exists(path).await
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.backend.read_file(path).await
    }

    async fn try_read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, FsError> {
        self.backend.try_read_file(path).await
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        self.backend.write_file(path, data).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), FsError> {
        self.backend.delete_file(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::MockFs;

    #[tokio::test]
    async fn fake_seam_delegates_without_host_io() {
        let root = PathBuf::from("/virtual-workspace");
        let backend = Arc::new(MockFs::new(root.clone()));
        let fs = LocalFs::with_backend(root.clone(), backend);
        let path = root.join("nested/file.txt");

        fs.write_file(&path, b"runtime-owned").await.unwrap();
        assert_eq!(fs.read_file(&path).await.unwrap(), b"runtime-owned");
        fs.delete_file(&path).await.unwrap();
        assert!(!fs.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn real_seam_fails_closed_without_agent() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("must-not-exist.txt");
        let fs = LocalFs::new(workspace.path().to_path_buf());

        let error = fs
            .write_file(&path, b"must not reach host storage")
            .await
            .expect_err("the real seam must require Agent and Runtime");
        assert!(error.to_string().contains("Simplicio Runtime denied"));
        assert!(
            !path.exists(),
            "failure must not fall back to local storage"
        );
    }
}
