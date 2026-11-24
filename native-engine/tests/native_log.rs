use sedravasql_native_engine::{
    NativeChange, NativeLog, NativeMutation, TransactionalStore, MAX_NATIVE_BATCH,
};
use tempfile::tempdir;

#[cfg(unix)]
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
#[cfg(not(unix))]
fn private_tempdir() -> tempfile::TempDir {
    tempdir().unwrap()
}

fn put(namespace: &str, key: &str, rev: u64, value: &str) -> NativeMutation {
    NativeMutation {
        namespace: namespace.into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Put(value.into()),
    }
}
fn delete(namespace: &str, key: &str, rev: u64) -> NativeMutation {
    NativeMutation {
        namespace: namespace.into(),
        key: key.into(),
        expected_revision: rev,
        change: NativeChange::Delete,
    }
}

#[test]
fn independent_batch_atomic_restart_and_idempotency() {
    let temp = private_tempdir();
    let file = temp.path().join("native.slog");
    let mut db = NativeLog::create(&file).unwrap();
    let ops = vec![
        put("accounts", "checking", 0, "300"),
        put("accounts", "savings", 0, "700"),
    ];
    let done = db.commit_batch("native-batch-00001", &ops).unwrap();
    assert_eq!(done.sequence, 1);
    assert_eq!(done.entries.len(), 2);
    assert_eq!(
        db.read("accounts", "checking").unwrap().unwrap().revision,
        1
    );
    assert_eq!(
        db.read("accounts", "savings")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("700")
    );
    db.verify().unwrap();
    drop(db);
    let mut db = NativeLog::open(&file).unwrap();
    let replay = db.commit_batch("native-batch-00001", &ops).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.sequence, 1);
    assert_eq!(db.sequence(), 1);
    assert!(db
        .commit_batch(
            "native-batch-00001",
            &[put("accounts", "checking", 0, "999")]
        )
        .is_err());
    assert_eq!(
        db.read("accounts", "checking")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("300")
    );
    assert_eq!(db.sequence(), 1);
}

#[test]
fn native_batch_rejects_stale_or_duplicate_without_partial_effect() {
    let temp = private_tempdir();
    let path = temp.path().join("rollback.log");
    let mut db = NativeLog::create(&path).unwrap();
    db.commit_batch("native-req-0000001", &[put("x", "a", 0, "one")])
        .unwrap();
    let before = std::fs::metadata(&path).unwrap().len();
    let invalid = [
        vec![put("x", "b", 0, "good"), put("x", "a", 0, "stale")],
        vec![put("x", "b", 0, "good"), put("x", "b", 0, "duplicate")],
        vec![put("x", "b", 0, "good"), delete("x", "absent", 0)],
        vec![
            put("x", "b", 0, "good"),
            put("x", "a", u64::MAX, "too-large-revision"),
        ],
    ];
    for (i, ops) in invalid.iter().enumerate() {
        assert!(db
            .commit_batch(&format!("native-invalid-{i:08}"), ops)
            .is_err());
        assert_eq!(db.sequence(), 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
        assert!(db.read("x", "b").unwrap().is_none());
    }
    assert!(db.commit_batch("native-empty-00001", &[]).is_err());
    let huge = vec![put("x", "a", 1, &"v".repeat(65_537))];
    assert!(db.commit_batch("native-huge-000001", &huge).is_err());
    assert!(db
        .commit_batch(
            "native-many-000001",
            &vec![put("x", "b", 0, "ok"); MAX_NATIVE_BATCH + 1]
        )
        .is_err());
    db.verify().unwrap();
}

#[test]
fn deletion_tombstone_prevents_aba_and_multikey_reopen_preserves_revision() {
    let temp = private_tempdir();
    let path = temp.path().join("tombstone.log");
    let mut db = NativeLog::create(&path).unwrap();
    db.commit_batch(
        "native-req-0000001",
        &[put("n", "a", 0, "one"), put("n", "b", 0, "two")],
    )
    .unwrap();
    db.commit_batch(
        "native-req-0000002",
        &[delete("n", "a", 1), put("n", "b", 1, "three")],
    )
    .unwrap();
    let tomb = db.read("n", "a").unwrap().unwrap();
    assert_eq!(tomb.revision, 2);
    assert_eq!(tomb.value, None);
    assert!(db
        .commit_batch("native-req-0000003", &[put("n", "a", 0, "old")])
        .is_err());
    db.commit_batch("native-req-0000004", &[put("n", "a", 2, "new")])
        .unwrap();
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(db.sequence(), 3);
    assert_eq!(db.read("n", "a").unwrap().unwrap().revision, 3);
    assert_eq!(
        db.read("n", "b").unwrap().unwrap().value.as_deref(),
        Some("three")
    );
    db.verify().unwrap();
}

#[test]
fn incomplete_tail_fails_closed_without_autotruncation() {
    use std::io::Write;
    let tmp = private_tempdir();
    let file = tmp.path().join("tail.log");
    let mut db = NativeLog::create(&file).unwrap();
    db.commit_batch("native-req-0000001", &[put("n", "k", 0, "saved")])
        .unwrap();
    drop(db);
    let mut out = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    out.write_all(&70u32.to_be_bytes()).unwrap();
    out.write_all(b"{\"seq\":2").unwrap();
    out.sync_all().unwrap();
    drop(out);
    let old = std::fs::read(&file).unwrap();
    assert!(NativeLog::open(&file).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), old);
    assert!(!file.with_file_name("tail.log.lock").exists());
}

#[test]
fn tampered_checksum_and_header_fail_closed() {
    for change_header in [true, false] {
        let tmp = private_tempdir();
        let file = tmp.path().join("tamper.log");
        let mut db = NativeLog::create(&file).unwrap();
        db.commit_batch("native-req-0000001", &[put("n", "k", 0, "saved")])
            .unwrap();
        drop(db);
        let mut b = std::fs::read(&file).unwrap();
        if change_header {
            b[0] ^= 1;
        } else {
            *b.last_mut().unwrap() ^= 1;
        }
        std::fs::write(&file, &b).unwrap();
        assert!(NativeLog::open(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b);
    }
}

#[test]
fn concurrent_handles_fail_closed_with_exclusive_lock() {
    let temp = private_tempdir();
    let file = temp.path().join("exclusive.log");
    let first = NativeLog::create(&file).unwrap();
    let lock = file.with_file_name("exclusive.log.lock");
    assert!(lock.exists());
    assert!(NativeLog::open(&file).is_err());
    assert!(NativeLog::create(&file).is_err());
    assert!(NativeLog::open(&lock).is_err());
    drop(first);
    assert!(!lock.exists());
    let second = NativeLog::open(&file).unwrap();
    assert!(NativeLog::open(&file).is_err());
    drop(second);
    assert!(NativeLog::open(&file).is_ok());
}

#[test]
fn fifty_commits_and_cas_replay_survive_reopen() {
    let temp = private_tempdir();
    let file = temp.path().join("long.log");
    let mut db = NativeLog::create(&file).unwrap();
    for n in 0..50 {
        let ops = [
            put("docs", "shared", n, &format!("record-{n:03}")),
            put("stats", "count", n, &format!("{n}")),
        ];
        assert_eq!(
            db.commit_batch(&format!("native-req-{n:08}"), &ops)
                .unwrap()
                .sequence,
            n + 1
        );
    }
    db.verify().unwrap();
    drop(db);
    let mut db = NativeLog::open(&file).unwrap();
    assert_eq!(db.sequence(), 50);
    assert_eq!(db.read("docs", "shared").unwrap().unwrap().revision, 50);
    let old = db
        .commit_batch(
            "native-req-00000000",
            &[
                put("docs", "shared", 0, "record-000"),
                put("stats", "count", 0, "0"),
            ],
        )
        .unwrap();
    assert!(old.replayed);
    assert_eq!(old.sequence, 1);
    assert_eq!(db.sequence(), 50);
}

#[cfg(unix)]
#[test]
fn reject_public_parent_file_symlink_and_hardlink() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = private_tempdir();
    let d = tmp.path().join("private");
    std::fs::create_dir(&d).unwrap();
    let file = d.join("native.log");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(NativeLog::create(&file).is_err());
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = NativeLog::create(&file).unwrap();
    drop(db);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(NativeLog::open(&file).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let hard = d.join("hard");
    std::fs::hard_link(&file, &hard).unwrap();
    assert!(NativeLog::open(&file).is_err());
    std::fs::remove_file(hard).unwrap();
    let link = d.join("link.log");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(NativeLog::open(&link).is_err());
    assert!(NativeLog::create(&link).is_err());
    assert!(NativeLog::open(&file).is_ok());
}

#[test]
fn b13_snapshot_order_tombstone_generation_and_receipts() {
    let temp = private_tempdir();
    let path = temp.path().join("b13.log");
    let mut db = NativeLog::create(&path).unwrap();
    let keys = [("n", "b"), ("n", "a"), ("n", "missing"), ("n", "b")];
    let empty = db.read_snapshot(&keys).unwrap();
    assert_eq!(empty.sequence, 0);
    assert!(empty.rows.iter().all(Option::is_none));
    let first = db
        .commit_batch(
            "b13-request-0001",
            &[put("n", "a", 0, "one"), put("n", "b", 0, "two")],
        )
        .unwrap();
    let view = db.read_snapshot(&keys).unwrap();
    assert_eq!(view.sequence, 1);
    assert_eq!(view.rows[0], view.rows[3]);
    assert_eq!(view.rows[1].as_ref().unwrap().value.as_deref(), Some("one"));
    assert!(view.rows[2].is_none());
    assert!(db
        .commit_batch(
            "b13-request-bad1",
            &[put("n", "a", 1, "new"), put("n", "b", 0, "bad")]
        )
        .is_err());
    assert_eq!(db.read_snapshot(&keys).unwrap(), view);
    db.commit_batch(
        "b13-request-0002",
        &[delete("n", "b", 1), put("n", "a", 1, "new")],
    )
    .unwrap();
    assert_eq!(
        db.lookup_receipt("b13-request-0001").unwrap(),
        Some(first.clone())
    );
    assert!(db.lookup_receipt("b13-request-bad1").unwrap().is_none());
    assert!(db.lookup_receipt("bad").is_err());
    assert!(db.read_snapshot(&[]).is_err());
    assert!(db.read_snapshot(&[("n", "a"); 17]).is_err());
    assert!(db.read_snapshot(&[("n", "a"), ("n", "../bad")]).is_err());
    drop(db);
    let db = NativeLog::open(&path).unwrap();
    let view = db.read_snapshot(&keys).unwrap();
    assert_eq!(view.sequence, 2);
    assert_eq!(view.rows[0].as_ref().unwrap().revision, 2);
    assert!(view.rows[0].as_ref().unwrap().value.is_none());
    assert_eq!(view.rows[1].as_ref().unwrap().value.as_deref(), Some("new"));
    assert_eq!(db.lookup_receipt("b13-request-0001").unwrap(), Some(first));
}

#[test]
fn b14_large_legacy_journal_replay_receipts_and_atomic_rejection() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let temp = private_tempdir();
    let path = temp.path().join("legacy.slog");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    let magic = b"SEDRLOG1";
    f.write_all(magic).unwrap();
    let mut head: [u8; 32] = Sha256::digest(magic).into();
    // Independent wire fixture, matching the pre-B14 frame contract.
    for i in 1..=2048u64 {
        let ops = [
            put("n", "counter", i - 1, &i.to_string()),
            put("n", &format!("k{i:04}"), 0, "value"),
        ];
        let payload = serde_json::to_vec(
            &serde_json::json!({"seq":i,"request_key":format!("legacy-{i:012}"),"operations":ops}),
        )
        .unwrap();
        let len = (payload.len() as u32).to_be_bytes();
        let mut h = Sha256::new();
        h.update(head);
        h.update(len);
        h.update(&payload);
        head = h.finalize().into();
        f.write_all(&len).unwrap();
        f.write_all(&payload).unwrap();
        f.write_all(&head).unwrap();
    }
    f.sync_all().unwrap();
    drop(f);
    let original = std::fs::read(&path).unwrap();
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(db.sequence(), 2048);
    assert_eq!(db.read("n", "counter").unwrap().unwrap().revision, 2048);
    let first = db.lookup_receipt("legacy-000000000001").unwrap().unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(first.entries[0].value.as_deref(), Some("1"));
    assert!(db
        .commit_batch(
            "legacy-000000002049",
            &[put("n", "new", 0, "x"), put("n", "counter", 0, "stale")]
        )
        .is_err());
    assert!(db.read("n", "new").unwrap().is_none());
    assert!(db.lookup_receipt("legacy-000000002049").unwrap().is_none());
    db.verify().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    db.commit_batch(
        "legacy-000000002049",
        &[put("n", "counter", 2048, "2049"), put("n", "new", 0, "x")],
    )
    .unwrap();
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(db.sequence(), 2049);
    assert_eq!(
        db.lookup_receipt("legacy-000000000001").unwrap().unwrap(),
        first
    );
    db.verify().unwrap();
}
