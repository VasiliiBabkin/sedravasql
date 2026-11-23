use sedravasql_native_engine::backup::{backup_to_new_file, plan_backup};
use sedravasql_native_engine::recovery::{plan_recovery, recover_to_new_file};
use sedravasql_native_engine::{NativeChange, NativeLog, NativeMutation, TransactionalStore};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn private() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn put(key: &str, rev: u64, value: &str) -> NativeMutation {
    NativeMutation {
        namespace: "demo".into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Put(value.into()),
    }
}
fn seed(source: &Path) -> Vec<u8> {
    let mut db = NativeLog::create(source).unwrap();
    for rev in 0..8 {
        let ops = vec![
            put("alpha", rev, &format!("a{rev}")),
            put("beta", rev, &format!("b{rev}")),
        ];
        db.commit_batch(&format!("backup-test-{rev:06}"), &ops)
            .unwrap();
    }
    drop(db);
    fs::read(source).unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn copy_clean_log_verifies_every_byte_and_preserves_receipts() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let dest = dir.path().join("backup.slog");
    let original = seed(&source);
    let plan = plan_backup(&source).unwrap();
    assert_eq!(plan.source_sha256, sha(&original));
    assert_eq!(plan.source_bytes, original.len() as u64);
    assert_eq!(plan.sequence, 8);
    let made = backup_to_new_file(&source, &dest, &plan.source_sha256).unwrap();
    assert_eq!(made.source_sha256, plan.source_sha256);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read(&dest).unwrap(), original);
    // The backup is a valid *independent* native log, not an opaque image.
    let mut backup = NativeLog::open(&dest).unwrap();
    assert_eq!(backup.sequence(), 8);
    backup.verify().unwrap();
    for rev in 0..8 {
        assert!(
            backup
                .commit_batch(
                    &format!("backup-test-{rev:06}"),
                    &[
                        put("alpha", rev, &format!("a{rev}")),
                        put("beta", rev, &format!("b{rev}"))
                    ]
                )
                .unwrap()
                .replayed
        );
    }
    assert_eq!(
        backup
            .read("demo", "beta")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("b7")
    );
    // Neither copying nor plan is a repair operation.
    assert!(plan_recovery(&source).is_err());
    assert!(
        recover_to_new_file(&source, &dir.path().join("bad-recovery"), &sha(&original)).is_err()
    );
}

#[test]
fn reject_stale_operator_hash_and_never_clobber_existing_output() {
    let dir = private();
    let source = dir.path().join("src.slog");
    let output = dir.path().join("keep.slog");
    let source_data = seed(&source);
    let earlier = plan_backup(&source).unwrap().source_sha256;
    {
        let mut db = NativeLog::open(&source).unwrap();
        db.commit_batch("after-plan-00000001", &[put("alpha", 8, "new")])
            .unwrap();
    }
    assert_ne!(plan_backup(&source).unwrap().source_sha256, earlier);
    assert!(backup_to_new_file(&source, &output, &earlier).is_err());
    assert!(!output.exists());
    assert!(backup_to_new_file(&source, &output, "broken").is_err());
    assert!(!output.exists());
    fs::write(&output, b"operator existing").unwrap();
    assert!(backup_to_new_file(
        &source,
        &output,
        &plan_backup(&source).unwrap().source_sha256
    )
    .is_err());
    assert_eq!(fs::read(&output).unwrap(), b"operator existing");
    assert_ne!(source_data, fs::read(&source).unwrap());
}

#[test]
fn fail_closed_on_lock_torn_or_corrupt_source() {
    use std::io::Write;
    let dir = private();
    let source = dir.path().join("source.slog");
    let bytes = seed(&source);
    let dest = dir.path().join("out.slog");
    let lock = dir.path().join("source.slog.lock");
    fs::write(&lock, b"busy").unwrap();
    assert!(plan_backup(&source).is_err());
    assert!(backup_to_new_file(&source, &dest, &sha(&bytes)).is_err());
    assert!(!dest.exists());
    fs::remove_file(lock).unwrap();
    {
        let mut f = fs::OpenOptions::new().append(true).open(&source).unwrap();
        f.write_all(&[0, 0]).unwrap();
    }
    assert!(plan_backup(&source).is_err());
    assert!(backup_to_new_file(&source, &dest, &sha(&fs::read(&source).unwrap())).is_err());
    assert!(!dest.exists());
    fs::write(&source, &bytes).unwrap();
    let mut corrupted = bytes;
    *corrupted.last_mut().unwrap() ^= 0x10;
    fs::write(&source, &corrupted).unwrap();
    assert!(plan_backup(&source).is_err());
    assert!(backup_to_new_file(&source, &dest, &sha(&corrupted)).is_err());
    assert!(!dest.exists());
}

#[cfg(unix)]
#[test]
fn deny_symlinks_hardlinks_and_public_paths() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = private();
    let source = dir.path().join("original.slog");
    let bytes = seed(&source);
    let plan = plan_backup(&source).unwrap();
    let target = dir.path().join("symlink-out");
    symlink("nonexistent", &target).unwrap();
    assert!(backup_to_new_file(&source, &target, &plan.source_sha256).is_err());
    assert!(target.is_symlink());
    let src_link = dir.path().join("linked.slog");
    fs::hard_link(&source, &src_link).unwrap();
    assert!(plan_backup(&source).is_err());
    assert!(plan_backup(&src_link).is_err());
    fs::remove_file(src_link).unwrap();
    let symlink_source = dir.path().join("alias");
    symlink(&source, &symlink_source).unwrap();
    assert!(plan_backup(&symlink_source).is_err());
    fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(plan_backup(&source).is_err());
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let public = dir.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(backup_to_new_file(&source, &public.join("copy"), &plan.source_sha256).is_err());
    assert!(!public.join("copy").exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn reject_oversized_source_before_copy_even_with_correct_hash() {
    let dir = private();
    let source = dir.path().join("source.slog");
    let output = dir.path().join("backup.slog");
    let bytes = seed(&source);
    let file = fs::OpenOptions::new().write(true).open(&source).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert!(plan_backup(&source).is_err());
    assert!(backup_to_new_file(&source, &output, &sha(&bytes)).is_err());
    assert!(!output.exists());
}
