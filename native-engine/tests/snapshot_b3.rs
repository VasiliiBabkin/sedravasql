use sedravasql_native_engine::backup::plan_backup;
use sedravasql_native_engine::snapshot::{create_snapshot, verify_snapshot};
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
fn put(key: &str, value: &str, rev: u64) -> NativeMutation {
    NativeMutation {
        namespace: "demo".into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Put(value.into()),
    }
}
fn del(key: &str, rev: u64) -> NativeMutation {
    NativeMutation {
        namespace: "demo".into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Delete,
    }
}
fn seed(path: &Path) -> Vec<u8> {
    let mut log = NativeLog::create(path).unwrap();
    for rev in 0..7 {
        log.commit_batch(
            &format!("snapshot-request-{rev:05}"),
            &[
                put("alpha", &format!("a{rev}"), rev),
                put("beta", &format!("b{rev}"), rev),
            ],
        )
        .unwrap();
    }
    log.commit_batch("snapshot-delete-9999", &[del("beta", 7)])
        .unwrap();
    drop(log);
    fs::read(path).unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn materialization_has_sorted_keys_tombstones_and_receipts_and_is_deterministic() {
    let dir = private();
    let source = dir.path().join("original.slog");
    let first = dir.path().join("first.snap");
    let second = dir.path().join("second.snap");
    let before = seed(&source);
    let approved = plan_backup(&source).unwrap();
    assert_eq!(approved.sequence, 8);
    let a = create_snapshot(&source, &first, &approved.source_sha256).unwrap();
    let b = create_snapshot(&source, &second, &approved.source_sha256).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.schema, "sedravasql.snapshot.b3-preview.v1");
    assert_eq!(a.entries, 2);
    assert_eq!(a.receipts, 8);
    assert_eq!(a.sequence, 8);
    assert_eq!(a.source_sha256, sha(&before));
    assert_eq!(verify_snapshot(&first).unwrap(), a);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert_eq!(fs::read(&source).unwrap(), before);
    // Distinct magic prevents accidental treating a materialization as a log.
    assert!(NativeLog::open(&first).is_err());
    assert!(verify_snapshot(&source).is_err());
    let bytes = fs::read(&first).unwrap();
    let size = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let body: serde_json::Value = serde_json::from_slice(&bytes[12..12 + size]).unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries[0]["key"], "alpha");
    assert_eq!(entries[0]["revision"], 7);
    assert_eq!(entries[0]["value"], "a6");
    assert_eq!(entries[1]["key"], "beta");
    assert_eq!(entries[1]["revision"], 8);
    assert!(entries[1]["value"].is_null());
    let receipts = body["receipts"].as_array().unwrap();
    assert!(receipts
        .windows(2)
        .all(|x| x[0]["request_key"].as_str().unwrap() < x[1]["request_key"].as_str().unwrap()));
}

#[test]
fn empty_log_has_zero_state_and_valid_file() {
    let dir = private();
    let src = dir.path().join("empty.slog");
    let snap = dir.path().join("empty.snap");
    {
        let db = NativeLog::create(&src).unwrap();
        assert_eq!(db.sequence(), 0);
    }
    let planned = plan_backup(&src).unwrap();
    let report = create_snapshot(&src, &snap, &planned.source_sha256).unwrap();
    assert_eq!(report.sequence, 0);
    assert_eq!(report.receipts, 0);
    assert_eq!(report.entries, 0);
    assert_eq!(verify_snapshot(&snap).unwrap(), report);
}

#[test]
fn source_lock_change_and_torn_frame_all_fail_closed_without_destination() {
    use std::io::Write;
    let dir = private();
    let src = dir.path().join("src.slog");
    let dest = dir.path().join("snap.bin");
    let intact = seed(&src);
    let approved = plan_backup(&src).unwrap().source_sha256;
    assert!(create_snapshot(&src, &dest, &"0".repeat(64)).is_err());
    assert!(!dest.exists());
    let lock = dir.path().join("src.slog.lock");
    fs::write(&lock, b"active?").unwrap();
    assert!(create_snapshot(&src, &dest, &approved).is_err());
    assert!(!dest.exists());
    fs::remove_file(lock).unwrap();
    {
        let mut log = NativeLog::open(&src).unwrap();
        log.commit_batch("snapshot-after-plan-01", &[put("alpha", "updated", 7)])
            .unwrap();
    }
    assert!(create_snapshot(&src, &dest, &approved).is_err());
    assert!(!dest.exists());
    // Torn final frame is not a snapshot candidate.
    let mut f = fs::OpenOptions::new().append(true).open(&src).unwrap();
    f.write_all(&[0, 0]).unwrap();
    drop(f);
    let torn = plan_backup(&src).unwrap_err().to_string();
    assert!(torn.contains("corrupt") || torn.contains("frame"), "{torn}");
    assert!(create_snapshot(&src, &dest, &sha(&fs::read(&src).unwrap())).is_err());
    assert!(!dest.exists());
    fs::write(&src, intact).unwrap();
    let mut corrupt = fs::read(&src).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(&src, &corrupt).unwrap();
    assert!(create_snapshot(&src, &dest, &sha(&corrupt)).is_err());
    assert!(!dest.exists());
}

#[test]
fn no_overwrite_symlink_hardlink_or_public_output() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = private();
    let src = dir.path().join("src.slog");
    seed(&src);
    let approved = plan_backup(&src).unwrap().source_sha256;
    let dest = dir.path().join("dest.snap");
    fs::write(&dest, b"important!").unwrap();
    assert!(create_snapshot(&src, &dest, &approved).is_err());
    assert_eq!(fs::read(&dest).unwrap(), b"important!");
    fs::remove_file(&dest).unwrap();
    symlink("missing", &dest).unwrap();
    assert!(create_snapshot(&src, &dest, &approved).is_err());
    assert!(dest.is_symlink());
    fs::remove_file(&dest).unwrap();
    let alias = dir.path().join("alias.slog");
    fs::hard_link(&src, &alias).unwrap();
    assert!(create_snapshot(&src, &dest, &approved).is_err());
    fs::remove_file(alias).unwrap();
    let public = dir.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(create_snapshot(&src, &public.join("out"), &approved).is_err());
    assert!(!public.join("out").exists());
}

#[test]
fn snapshot_detects_partial_or_corrupt_or_extended_content() {
    use std::io::Write;
    let dir = private();
    let src = dir.path().join("src.slog");
    let output = dir.path().join("snapshot.dat");
    seed(&src);
    let approved = plan_backup(&src).unwrap().source_sha256;
    create_snapshot(&src, &output, &approved).unwrap();
    let clean = fs::read(&output).unwrap();
    for mutilated in [
        clean[..clean.len() - 1].to_vec(),
        {
            let mut bad = clean.clone();
            bad[12] ^= 1;
            bad
        },
        {
            let mut bad = clean.clone();
            bad.extend_from_slice(b"unexpected");
            bad
        },
    ] {
        fs::write(&output, &mutilated).unwrap();
        assert!(verify_snapshot(&output).is_err());
    }
    fs::write(&output, &clean).unwrap();
    assert!(verify_snapshot(&output).is_ok());
    // File size cap is enforced before allocating the snapshot body.
    let mut handle = fs::OpenOptions::new().write(true).open(&output).unwrap();
    handle.set_len(64 * 1024 * 1024 + 1).unwrap();
    handle.flush().unwrap();
    assert!(verify_snapshot(&output).is_err());
}
