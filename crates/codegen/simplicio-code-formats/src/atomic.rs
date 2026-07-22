use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

/// Publish bytes with a same-directory temp file and rename.
///
/// Readers see either the previous complete file or the new complete file;
/// they never observe a partially written artifact. The temporary file is
/// removed on every failure path.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        // rename(2) is atomic for observers, but the directory entry is not
        // crash-durable until the containing directory is flushed.  Runtime
        // migration/restart must not acknowledge a publish that can vanish
        // after a power loss.
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_complete_content_without_leaking_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("state.hbi");
        write_atomically(&target, b"first").unwrap();
        write_atomically(&target, b"second").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
