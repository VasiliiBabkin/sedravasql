//! Identical transaction corpus for two unrelated storage engines.
//! The native crate has zero SQLite dependencies; the SQLite adapter is
//! isolated behind explicit init and separate tables. Neither is production.
use sedravasql::native_log::{
    NativeChange, NativeCommit, NativeLog, NativeMutation, TransactionalStore, MAX_NATIVE_BATCH,
};
use sedravasql::sqlite_batch::SqliteBatchStore;
use std::path::Path;
use tempfile::tempdir;

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
fn private_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
fn exercise<S: TransactionalStore>(db: &mut S) -> Vec<NativeCommit> {
    let mut receipts = Vec::new();
    assert!(db.read("n", "left").unwrap().is_none());
    let first_ops = [put("n", "left", 0, "100"), put("n", "right", 0, "250")];
    let one = db.commit_batch("batch-shared-000001", &first_ops).unwrap();
    assert_eq!(one.sequence, 1);
    assert_eq!(one.entries.len(), 2);
    assert!(!one.replayed);
    receipts.push(one.clone());
    assert_eq!(
        db.read("n", "left").unwrap().unwrap().value.as_deref(),
        Some("100")
    );
    let repeated = db.commit_batch("batch-shared-000001", &first_ops).unwrap();
    assert!(repeated.replayed);
    assert_eq!(repeated.sequence, 1);
    assert_eq!(repeated.entries, one.entries);
    assert!(db
        .commit_batch(
            "batch-shared-000001",
            &[put("n", "left", 0, "101"), put("n", "right", 0, "250")]
        )
        .is_err());
    // Entire batch must fail if only one CAS fails, keeping the other row.
    assert!(db
        .commit_batch(
            "batch-shared-000002",
            &[put("n", "left", 1, "200"), put("n", "right", 0, "BAD")]
        )
        .is_err());
    assert_eq!(
        db.read("n", "left").unwrap().unwrap().value.as_deref(),
        Some("100")
    );
    let two = db
        .commit_batch(
            "batch-shared-000003",
            &[delete("n", "left", 1), put("n", "right", 1, "300")],
        )
        .unwrap();
    assert_eq!(two.sequence, 2);
    receipts.push(two);
    assert_eq!(db.read("n", "left").unwrap().unwrap().revision, 2);
    assert_eq!(db.read("n", "left").unwrap().unwrap().value, None);
    assert!(db
        .commit_batch("batch-shared-000004", &[put("n", "left", 0, "stale-ABA"),])
        .is_err());
    let three = db
        .commit_batch(
            "batch-shared-000005",
            &[
                put("n", "left", 2, "reborn"),
                put("other", "key", 0, "UTF-8: Привет"),
            ],
        )
        .unwrap();
    receipts.push(three);
    assert_eq!(db.read("n", "left").unwrap().unwrap().revision, 3);
    assert!(db
        .commit_batch(
            "batch-shared-000006",
            &[put("same", "dup", 0, "A"), put("same", "dup", 0, "B")]
        )
        .is_err());
    assert!(db.commit_batch("batch-shared-000007", &[]).is_err());
    assert!(db
        .commit_batch(
            "batch-shared-000008",
            &[put("n", "left", 3, &"X".repeat(65_537))]
        )
        .is_err());
    assert!(db
        .commit_batch(
            "batch-shared-000009",
            &vec![put("other", "duplicated", 0, "x"); MAX_NATIVE_BATCH + 1]
        )
        .is_err());
    // A few dozen independent transactions must not hit the legacy A3
    // 32-receipt limit. Both engines use the same unchanged receipt contract.
    for revision in 3..46 {
        let ops = [put("n", "left", revision, &format!("v-{revision:04}"))];
        let response = db
            .commit_batch(&format!("batch-loop-{revision:06}"), &ops)
            .unwrap();
        assert!(!response.replayed);
        receipts.push(response);
    }
    assert_eq!(db.read("n", "left").unwrap().unwrap().revision, 46);
    assert_eq!(receipts.len(), 46);
    receipts
}

#[test]
fn same_contract_both_engines_and_replay_survives_restart() {
    let tmp = tempdir().unwrap();
    private_dir(tmp.path());
    let sqlite_file = tmp.path().join("compat.sqlite");
    let native_file = tmp.path().join("native.slog");
    let mut sqlite = SqliteBatchStore::init(&sqlite_file).unwrap();
    let mut native = NativeLog::create(&native_file).unwrap();
    let sqlite_receipts = exercise(&mut sqlite);
    let native_receipts = exercise(&mut native);
    assert_eq!(sqlite_receipts, native_receipts);
    assert_eq!(sqlite.sequence().unwrap(), 46);
    assert_eq!(native.sequence(), 46);
    sqlite.verify().unwrap();
    native.verify().unwrap();
    drop(sqlite);
    drop(native);
    let mut sqlite = SqliteBatchStore::open(&sqlite_file).unwrap();
    let mut native = NativeLog::open(&native_file).unwrap();
    assert_eq!(sqlite.sequence().unwrap(), 46);
    assert_eq!(native.sequence(), 46);
    for (key, ops) in [
        (
            "batch-shared-000001",
            vec![put("n", "left", 0, "100"), put("n", "right", 0, "250")],
        ),
        (
            "batch-shared-000005",
            vec![
                put("n", "left", 2, "reborn"),
                put("other", "key", 0, "UTF-8: Привет"),
            ],
        ),
        ("batch-loop-000045", vec![put("n", "left", 45, "v-0045")]),
    ] {
        let a = sqlite.commit_batch(key, &ops).unwrap();
        let b = native.commit_batch(key, &ops).unwrap();
        assert!(a.replayed && b.replayed);
        assert_eq!(a, b);
    }
    assert_eq!(sqlite.sequence().unwrap(), 46);
    assert_eq!(native.sequence(), 46);
    sqlite.verify().unwrap();
    native.verify().unwrap();
}

#[test]
fn sqlite_batch_tables_are_explicit_and_legacy_state_is_unchanged() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("legacy.sqlite");
    let mut db = sedravasql::Database::init(&file).unwrap();
    db.mutate(
        "legacy",
        "state",
        sedravasql::Change::Put("safe"),
        0,
        "legacy-request-0001",
    )
    .unwrap();
    assert!(
        SqliteBatchStore::open(&file).is_err(),
        "must not silently migrate v1"
    );
    drop(db);
    let mut batch = SqliteBatchStore::init(&file).unwrap();
    batch
        .commit_batch(
            "batch-explicit-0001",
            &[put("n", "a", 0, "A"), put("n", "b", 0, "B")],
        )
        .unwrap();
    drop(batch);
    let legacy = sedravasql::Database::open(&file).unwrap();
    legacy.verify().unwrap();
    assert_eq!(
        legacy
            .read("legacy", "state")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("safe")
    );
    assert_eq!(legacy.audit(5).unwrap().len(), 1);
    assert_eq!(
        legacy
            .lookup_receipt("legacy", "legacy-request-0001")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    drop(legacy);
    let batch = SqliteBatchStore::open(&file).unwrap();
    assert_eq!(
        batch.read("n", "a").unwrap().unwrap().value.as_deref(),
        Some("A")
    );
    batch.verify().unwrap();
}

#[test]
fn sqlite_batch_refuses_corrupted_receipt_counter_and_foreign_schema() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("counters.sqlite");
    let mut batch = SqliteBatchStore::init(&file).unwrap();
    batch
        .commit_batch("batch-corrupt-0001", &[put("n", "k", 0, "value")])
        .unwrap();
    drop(batch);
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute(
        "DELETE FROM engine_batch_receipts WHERE request_key='batch-corrupt-0001'",
        [],
    )
    .unwrap_err();
    conn.execute("DELETE FROM engine_batch_audit WHERE sequence=1", [])
        .unwrap();
    conn.execute(
        "DELETE FROM engine_batch_receipts WHERE request_key='batch-corrupt-0001'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(SqliteBatchStore::open(&file).is_err());
    let untouched = sedravasql::Database::open(&file).unwrap();
    untouched.verify().unwrap();
}

#[test]
fn two_sqlite_batch_writers_are_serialized_and_cas_prevents_lost_updates() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("parallel.sqlite");
    drop(SqliteBatchStore::init(&path).unwrap());
    let mut workers = Vec::new();
    for index in 0..8 {
        let path = path.clone();
        workers.push(std::thread::spawn(move || {
            let mut batch = SqliteBatchStore::open(&path).unwrap();
            batch.commit_batch(
                &format!("batch-parallel-{index:05}"),
                &[
                    put("accounts", "left", 0, &format!("writer-{index}")),
                    put("accounts", "right", 0, &format!("writer-{index}")),
                ],
            )
        }));
    }
    let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|v| v.is_ok()).count(), 1);
    assert_eq!(outcomes.iter().filter(|v| v.is_err()).count(), 7);
    let store = SqliteBatchStore::open(&path).unwrap();
    assert_eq!(store.sequence().unwrap(), 1);
    let a = store.read("accounts", "left").unwrap().unwrap();
    let b = store.read("accounts", "right").unwrap().unwrap();
    assert_eq!(a.revision, 1);
    assert_eq!(b.revision, 1);
    assert_eq!(a.value, b.value);
    store.verify().unwrap();
}

#[test]
fn sqlite_batch_uses_consistent_online_backup_api_without_changing_legacy_db() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("live.sqlite");
    let backup = tmp.path().join("backup.sqlite");
    let mut batch = SqliteBatchStore::init(&file).unwrap();
    batch
        .commit_batch(
            "batch-backup-00001",
            &[put("ns", "x", 0, "first"), put("ns", "y", 0, "second")],
        )
        .unwrap();
    let db = sedravasql::Database::open(&file).unwrap();
    db.backup(&backup).unwrap();
    batch
        .commit_batch(
            "batch-backup-00002",
            &[
                put("ns", "x", 1, "new-first"),
                put("ns", "y", 1, "new-second"),
            ],
        )
        .unwrap();
    drop(batch);
    drop(db);
    let restored = SqliteBatchStore::open(&backup).unwrap();
    assert_eq!(restored.sequence().unwrap(), 1);
    assert_eq!(
        restored.read("ns", "y").unwrap().unwrap().value.as_deref(),
        Some("second")
    );
    restored.verify().unwrap();
    let live = SqliteBatchStore::open(&file).unwrap();
    assert_eq!(live.sequence().unwrap(), 2);
    assert_eq!(
        live.read("ns", "x").unwrap().unwrap().value.as_deref(),
        Some("new-first")
    );
    live.verify().unwrap();
}
#[test]
fn sqlite_batch_open_refuses_missing_audit_row_even_if_sqlite_integrity_is_ok() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("tampered.sqlite");
    let mut store = SqliteBatchStore::init(&file).unwrap();
    store
        .commit_batch(
            "batch-audit-000001",
            &[put("ns", "x", 0, "first"), put("ns", "y", 0, "second")],
        )
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute(
        "DELETE FROM engine_batch_audit WHERE sequence=1 AND op_index=1",
        [],
    )
    .unwrap();
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(conn);
    assert!(SqliteBatchStore::open(&file).is_err());
}
