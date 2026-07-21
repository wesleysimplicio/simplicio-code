use crate::write_atomically;
use std::path::{Path, PathBuf};

const MAX_LEGACY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub dry_run: bool,
    pub migrated: bool,
    pub backup: Option<PathBuf>,
}

/// Converts one bounded legacy artifact through a typed caller-supplied
/// decoder, preserving the source as a backup and publishing the replacement
/// atomically. The callback is the only place where a legacy protocol parser
/// may live; the new artifact is always bytes in a typed binary container.
pub fn migrate_bytes_atomically<F>(
    legacy: &Path,
    target: &Path,
    backup: &Path,
    dry_run: bool,
    encode: F,
) -> std::io::Result<MigrationOutcome>
where
    F: FnOnce(&[u8]) -> std::io::Result<Vec<u8>>,
{
    let source = std::fs::read(legacy)?;
    if source.len() > MAX_LEGACY_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "legacy artifact exceeds migration limit"));
    }
    let converted = encode(&source)?;
    if dry_run {
        return Ok(MigrationOutcome { dry_run: true, migrated: false, backup: None });
    }
    if !backup.exists() {
        std::fs::copy(legacy, backup)?;
    }
    write_atomically(target, &converted)?;
    Ok(MigrationOutcome { dry_run: false, migrated: true, backup: Some(backup.to_path_buf()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_does_not_create_target_or_backup() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("old.state");
        let target = dir.path().join("new.state");
        let backup = dir.path().join("old.state.bak");
        std::fs::write(&legacy, b"legacy").unwrap();
        let outcome = migrate_bytes_atomically(&legacy, &target, &backup, true, |_| Ok(b"new".to_vec())).unwrap();
        assert!(outcome.dry_run && !target.exists() && !backup.exists());
    }
}
