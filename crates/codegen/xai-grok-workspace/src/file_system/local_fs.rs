use crate::file_system::RuntimeFs;
use crate::file_system::{AsyncFileSystem, FsError};
use std::path::{Path, PathBuf};

pub struct LocalFs {
    root: PathBuf,
    runtime: RuntimeFs,
}

impl LocalFs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            runtime: RuntimeFs::new(root.clone()),
            root,
        }
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for LocalFs {
    fn root(&self) -> &Path {
        &self.root
    }

    async fn exists(&self, path: &Path) -> Result<bool, FsError> {
        self.runtime.exists(path).await
    }

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.runtime.read_file(path).await
    }

    async fn try_read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, FsError> {
        self.runtime.try_read_file(path).await
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        self.runtime.write_file(path, data).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), FsError> {
        self.runtime.delete_file(path).await
    }
}
