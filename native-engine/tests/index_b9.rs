use sedravasql_native_engine::backup::plan_backup;
use sedravasql_native_engine::index::{create_index, VerifiedIndexReader};
use sedravasql_native_engine::snapshot::create_snapshot;
use sedravasql_native_engine::{NativeChange, NativeLog, NativeMutation, TransactionalStore};
use std::fs;
use tempfile::tempdir;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = dir.path().join("native.log");
    let mut db = NativeLog::create(&source).unwrap();
    for batch in 0..4 {
        let ops = (0..16)
            .map(|i| NativeMutation {
                namespace: "test".into(),
                key: format!("key_{:03}", batch * 16 + i),
                expected_revision: 0,
                change: NativeChange::Put(format!("v{batch}-{i}:{}", "x".repeat(1024))),
            })
            .collect::<Vec<_>>();
        db.commit_batch(&format!("b9-index-batch-{batch:08}"), &ops)
            .unwrap();
    }
    drop(db);
    let snapshot = dir.path().join("native.snapshot");
    let plan = plan_backup(&source).unwrap();
    let snap = create_snapshot(&source, &snapshot, &plan.source_sha256).unwrap();
    let index = dir.path().join("native.index");
    create_index(&snapshot, &index, &snap.snapshot_sha256).unwrap();
    (dir, index)
}
#[test]
fn one_full_open_then_bounded_random_seek_reads() {
    let (_dir, path) = fixture();
    let mut index = VerifiedIndexReader::open(&path).unwrap();
    assert_eq!(index.report().entries, 64);
    assert_eq!(index.row_bytes_read(), 0);
    let whole = index.report().index_bytes;
    for key in ["key_007", "key_030", "key_047", "key_063"] {
        let found = index.get("test", key).unwrap().unwrap();
        assert_eq!(found.key, key);
        assert!(found.value.unwrap().contains('x'));
    }
    for _ in 0..50 {
        assert!(index.get("test", "nonexistent").unwrap().is_none());
    }
    assert!(
        index.row_bytes_read() < whole / 8,
        "row data should be read only for four chosen records"
    );
    assert!(index.get("../bad", "key_007").is_err());
}
#[test]
fn detect_mutation_after_open_without_full_rescan() {
    let (_dir, path) = fixture();
    let mut index = VerifiedIndexReader::open(&path).unwrap();
    assert!(index.get("test", "key_007").unwrap().is_some());
    let mut bytes = fs::read(&path).unwrap();
    let where_to_flip = bytes.len() / 2;
    bytes[where_to_flip] ^= 1;
    fs::write(&path, &bytes).unwrap();
    assert!(index.get("test", "key_030").is_err());
}
#[test]
fn detect_rename_and_inode_replacement_after_open() {
    let (dir, path) = fixture();
    let mut index = VerifiedIndexReader::open(&path).unwrap();
    let new = dir.path().join("replacement");
    fs::copy(&path, &new).unwrap();
    fs::rename(&new, &path).unwrap();
    assert!(index.get("test", "key_001").is_err());
}
#[test]
fn unopened_invalid_format_remains_fail_closed() {
    let (_dir, path) = fixture();
    let bytes = fs::read(&path).unwrap();
    let mut corrupt = bytes.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    fs::write(&path, &corrupt).unwrap();
    assert!(VerifiedIndexReader::open(&path).is_err());
    fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
    assert!(VerifiedIndexReader::open(&path).is_err());
}
