use simplicio_code_formats::{
    HbiReader, HbiSection, HbpRecord, decode_hbp, encode_hbi, encode_hbp, migrate_bytes_atomically,
};

fn encode_legacy(source: &[u8]) -> std::io::Result<Vec<u8>> {
    let text = std::str::from_utf8(source)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let (workspace, revision) = text.trim().split_once('|').ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid legacy state")
    })?;
    encode_hbi(
        "simplicio.workspace-state/v1",
        &[
            HbiSection {
                kind: 1,
                bytes: workspace.as_bytes().to_vec(),
            },
            HbiSection {
                kind: 2,
                bytes: revision.as_bytes().to_vec(),
            },
        ],
    )
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[test]
fn upgrade_restart_receipt_and_idempotent_resume() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("workspace.legacy");
    let current = dir.path().join("workspace.hbi");
    let backup = dir.path().join("workspace.legacy.bak");
    std::fs::write(&legacy, b"demo|abc123\n").unwrap();

    let first = migrate_bytes_atomically(&legacy, &current, &backup, false, encode_legacy).unwrap();
    assert!(first.migrated);
    let reopened = std::fs::read(&current).unwrap();
    let state = HbiReader::open(&reopened).unwrap();
    assert!(state.schema_matches("simplicio.workspace-state/v1"));
    assert_eq!(state.section(0).unwrap().1, b"demo");
    assert_eq!(state.section(1).unwrap().1, b"abc123");
    assert_eq!(std::fs::read(&backup).unwrap(), b"demo|abc123\n");

    let receipt = encode_hbp(&[
        HbpRecord {
            sequence: 0,
            payload: b"migration-started".to_vec(),
        },
        HbpRecord {
            sequence: 1,
            payload: blake3::hash(&reopened).as_bytes().to_vec(),
        },
    ])
    .unwrap();
    assert_eq!(decode_hbp(&receipt).unwrap().len(), 2);

    let resumed =
        migrate_bytes_atomically(&legacy, &current, &backup, false, encode_legacy).unwrap();
    assert!(!resumed.migrated);
    assert_eq!(std::fs::read(&current).unwrap(), reopened);
}

#[test]
fn corrupt_input_and_mixed_version_fail_closed_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("workspace.legacy");
    let current = dir.path().join("workspace.hbi");
    let backup = dir.path().join("workspace.legacy.bak");
    std::fs::write(&legacy, b"truncated").unwrap();
    assert!(migrate_bytes_atomically(&legacy, &current, &backup, false, encode_legacy).is_err());
    assert!(!current.exists() && !backup.exists());

    std::fs::write(&legacy, b"demo|abc123").unwrap();
    std::fs::write(&current, b"future-version-artifact").unwrap();
    let error =
        migrate_bytes_atomically(&legacy, &current, &backup, false, encode_legacy).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&current).unwrap(), b"future-version-artifact");
    assert!(!backup.exists());
}

#[test]
fn receipt_rejects_missing_or_reordered_lineage() {
    let records = [HbpRecord {
        sequence: 1,
        payload: b"out-of-order".to_vec(),
    }];
    assert!(encode_hbp(&records).is_err());
}
