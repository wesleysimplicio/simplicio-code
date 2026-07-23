//! Shared limits and binary-safe encoding for Runtime-owned filesystem operations.

use async_trait::async_trait;
use base64::Engine;
use xai_grok_workspace_types::rpc::fs::FsReadEncoding;

/// Server-side cap on a single ranged read's effective byte budget
/// (`min(length, max_bytes)`). 4 MiB raw (≈ 5.3 MiB base64) stays under
/// the server's 8 MiB frame cap. Shared by every fs read surface.
pub const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

pub fn clamp_read_length(length: Option<u64>, max_bytes: u64) -> u64 {
    length
        .unwrap_or(u64::MAX)
        .min(max_bytes)
        .min(MAX_READ_BYTES)
}

/// A read chunk in the requested transfer encoding.
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
        assert_eq!(clamp_read_length(Some(u64::MAX), u64::MAX), MAX_READ_BYTES);
    }
}
