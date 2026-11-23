use sedravasql_native_engine::backup::plan_backup;
use sedravasql_native_engine::restore::restore_to_new_file;
use sedravasql_native_engine::snapshot::{create_snapshot, verify_snapshot};
use sedravasql_native_engine::{NativeChange, NativeLog, NativeMutation, TransactionalStore};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn private() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}
fn put(key: &str, rev: u64, value: &str) -> NativeMutation {
    NativeMutation {
        namespace: "demo".into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Put(value.into()),
    }
}
fn delete(key: &str, rev: u64) -> NativeMutation {
    NativeMutation {
        namespace: "demo".into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Delete,
    }
}
fn seed(path: &Path) -> Vec<u8> {
    let mut db = NativeLog::create(path).unwrap();
    for rev in 0..9 {
        db.commit_batch(
            &format!("restore-batch-{rev:06}"),
            &[
                put("a", rev, &format!("value-{rev}")),
                put("b", rev, &format!("🍀{rev}")),
            ],
        )
        .unwrap();
    }
    db.commit_batch("restore-delete-000001", &[delete("b", 9)])
        .unwrap();
    drop(db);
    fs::read(path).unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn snapshot(source: &Path, dest: &Path) -> String {
    let planned = plan_backup(source).unwrap();
    let made = create_snapshot(source, dest, &planned.source_sha256).unwrap();
    assert_eq!(made.snapshot_sha256, sha(&fs::read(dest).unwrap()));
    made.snapshot_sha256
}

#[test]
fn byte_identical_rebuild_and_idempotency_with_tombstones() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let input = dir.path().join("snapshot.bin");
    let output = dir.path().join("rebuilt.slog");
    let original = seed(&source);
    let source_sha = sha(&original);
    let snap_sha = snapshot(&source, &input);
    let report = restore_to_new_file(&input, &output, &snap_sha).unwrap();
    assert_eq!(report.snapshot_sha256, snap_sha);
    assert_eq!(report.recovered_log_sha256, source_sha);
    assert_eq!(report.sequence, 10);
    assert_eq!(report.entries, 2);
    assert_eq!(report.receipts, 10);
    assert_eq!(report.recovered_log_bytes, original.len() as u64);
    assert_eq!(fs::read(&output).unwrap(), original);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(sha(&fs::read(&input).unwrap()), snap_sha);
    let mut rebuilt = NativeLog::open(&output).unwrap();
    rebuilt.verify().unwrap();
    assert_eq!(rebuilt.sequence(), 10);
    for rev in 0..9 {
        let result = rebuilt
            .commit_batch(
                &format!("restore-batch-{rev:06}"),
                &[
                    put("a", rev, &format!("value-{rev}")),
                    put("b", rev, &format!("🍀{rev}")),
                ],
            )
            .unwrap();
        assert!(result.replayed);
        assert_eq!(result.sequence, rev + 1);
    }
    let old_delete = rebuilt
        .commit_batch("restore-delete-000001", &[delete("b", 9)])
        .unwrap();
    assert!(old_delete.replayed);
    assert_eq!(rebuilt.read("demo", "b").unwrap().unwrap().value, None);
    assert_eq!(rebuilt.read("demo", "b").unwrap().unwrap().revision, 10);
    assert!(rebuilt
        .commit_batch("restore-batch-000000", &[put("a", 0, "altered")])
        .is_err());
    assert!(rebuilt
        .commit_batch("new-stale-request", &[put("a", 5, "stale")])
        .is_err());
    assert_eq!(
        rebuilt
            .commit_batch("restore-new-request01", &[put("a", 9, "after")])
            .unwrap()
            .sequence,
        11
    );
    rebuilt.verify().unwrap();
}

#[test]
fn zero_commit_snapshot_restores_valid_empty_journal() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let snap = dir.path().join("snap.bin");
    let dest = dir.path().join("dest.slog");
    drop(NativeLog::create(&source).unwrap());
    let input_sha = snapshot(&source, &snap);
    let report = restore_to_new_file(&snap, &dest, &input_sha).unwrap();
    assert_eq!(report.sequence, 0);
    assert_eq!(report.entries, 0);
    assert_eq!(report.receipts, 0);
    assert_eq!(fs::read(&source).unwrap(), fs::read(&dest).unwrap());
    let mut db = NativeLog::open(&dest).unwrap();
    assert_eq!(
        db.commit_batch("restore-first-request", &[put("a", 0, "new")])
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn wrong_sha_and_existing_targets_never_touch_preexisting_data() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = private();
    let source = dir.path().join("source.slog");
    let snap = dir.path().join("snapshot");
    let dest = dir.path().join("dest");
    let source_bytes = seed(&source);
    let approved = snapshot(&source, &snap);
    assert!(restore_to_new_file(&snap, &dest, &"f".repeat(64)).is_err());
    assert!(!dest.exists());
    assert!(restore_to_new_file(&snap, &dest, "invalid").is_err());
    assert!(!dest.exists());
    fs::write(&dest, b"important").unwrap();
    assert!(restore_to_new_file(&snap, &dest, &approved).is_err());
    assert_eq!(fs::read(&dest).unwrap(), b"important");
    fs::remove_file(&dest).unwrap();
    symlink("nonexistent", &dest).unwrap();
    assert!(restore_to_new_file(&snap, &dest, &approved).is_err());
    assert!(dest.is_symlink());
    fs::remove_file(&dest).unwrap();
    let hard = dir.path().join("hardlink");
    fs::hard_link(&snap, &hard).unwrap();
    assert!(restore_to_new_file(&snap, &dest, &approved).is_err());
    fs::remove_file(hard).unwrap();
    let public = dir.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(restore_to_new_file(&snap, &public.join("data"), &approved).is_err());
    assert!(!public.join("data").exists());
    assert_eq!(source_bytes, fs::read(&source).unwrap());
}

#[test]
fn broken_and_forged_checksummed_snapshot_is_rejected() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let snap = dir.path().join("snapshot");
    let dest = dir.path().join("restored.slog");
    seed(&source);
    let approved = snapshot(&source, &snap);
    let clean = fs::read(&snap).unwrap();
    for tampered in [
        clean[..clean.len() - 1].to_vec(),
        {
            let mut data = clean.clone();
            data[18] ^= 1;
            data
        },
        {
            let mut data = clean.clone();
            data.extend_from_slice(b"trailing");
            data
        },
    ] {
        fs::write(&snap, &tampered).unwrap();
        assert!(restore_to_new_file(&snap, &dest, &sha(&tampered)).is_err());
        assert!(!dest.exists());
    }
    fs::write(&snap, &clean).unwrap();
    assert_eq!(verify_snapshot(&snap).unwrap().snapshot_sha256, approved);
    // Tamper with the first receipt revision, RECOMPUTE the outer SHA.
    // The restore should fail its semantic/history checks without a panic.
    let forged = clean.clone();
    let payload_len = u32::from_be_bytes(forged[8..12].try_into().unwrap()) as usize;
    let mut json: serde_json::Value =
        serde_json::from_slice(&forged[12..12 + payload_len]).unwrap();
    let first = json["receipts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["result"]["sequence"] == 1)
        .unwrap();
    first["result"]["entries"][0]["revision"] = serde_json::json!(0);
    let bytes = serde_json::to_vec(&json).unwrap();
    let mut result = b"SEDRSNP1".to_vec();
    result.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    result.extend_from_slice(&bytes);
    result.extend_from_slice(&Sha256::digest(&result));
    fs::write(&snap, &result).unwrap();
    assert!(
        verify_snapshot(&snap).is_ok(),
        "outer checksums are intentionally valid"
    );
    assert!(restore_to_new_file(&snap, &dest, &sha(&result)).is_err());
    assert!(!dest.exists());

    // Independently forge an apparently valid snapshot of another source.
    json["receipts"][0]["result"]["entries"][0]["revision"] = serde_json::json!(1);
    json["log_sha256"] = serde_json::json!("0".repeat(64));
    let bytes = serde_json::to_vec(&json).unwrap();
    let mut result = b"SEDRSNP1".to_vec();
    result.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    result.extend_from_slice(&bytes);
    result.extend_from_slice(&Sha256::digest(&result));
    fs::write(&snap, &result).unwrap();
    assert!(restore_to_new_file(&snap, &dest, &sha(&result)).is_err());
    assert!(!dest.exists());
}

#[test]
fn restored_log_does_not_overwrite_old_log_and_copy_can_be_repeated_to_distinct_paths() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let snap = dir.path().join("snapshot");
    let a = dir.path().join("restoredA");
    let b = dir.path().join("restoredB");
    let original = seed(&source);
    let approved = snapshot(&source, &snap);
    let ra = restore_to_new_file(&snap, &a, &approved).unwrap();
    let rb = restore_to_new_file(&snap, &b, &approved).unwrap();
    assert_eq!(ra, rb);
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    assert_eq!(fs::read(&source).unwrap(), original);
    // Replacing an existing journal is never allowed, even if it has the same bytes.
    assert!(restore_to_new_file(&snap, &source, &approved).is_err());
    assert_eq!(fs::read(&source).unwrap(), original);
}
