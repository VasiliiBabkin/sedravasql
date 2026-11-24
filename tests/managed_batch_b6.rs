use rusqlite::Connection;
use sedravasql::{
    managed_batch::{ManagedBatchReceipt, ManagedPut},
    Change, Database,
};
use tempfile::tempdir;

fn put(key: &str, value: &str, expected: u64) -> ManagedPut {
    ManagedPut {
        namespace: "public".into(),
        key: key.into(),
        value: value.into(),
        expected_revision: expected,
    }
}
fn key_ops(rev: u64, state_rev: u64) -> Vec<ManagedPut> {
    vec![
        put("left", &format!("🍀-{rev}"), rev),
        put("right", &format!("🐱-{rev}"), rev),
        ManagedPut {
            namespace: "sedrava_native_a6".into(),
            key: "state_test".into(),
            value: format!("state-{state_rev}"),
            expected_revision: state_rev,
        },
    ]
}
fn no_receipts(connection: &Connection) -> (i64, i64, i64) {
    let records = connection
        .query_row("SELECT count(*) FROM records", [], |r| r.get(0))
        .unwrap();
    let audit = connection
        .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
        .unwrap();
    let batches = connection
        .query_row("SELECT count(*) FROM managed_batch_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    (records, audit, batches)
}
#[test]
fn three_key_transaction_persists_with_audit_and_replay() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("managed.sqlite");
    let mut db = Database::init(&path).unwrap();
    db.enable_managed_batches().unwrap();
    let items = key_ops(0, 0);
    let first = db
        .commit_managed_batch(
            "app_v1",
            "req-batch-000001",
            &items,
            r#"{"saved":true,"counter":1}"#,
        )
        .unwrap();
    assert_eq!(first.revisions, vec![1, 1, 1]);
    assert!(!first.replayed);
    assert_eq!(
        db.read("public", "left").unwrap().unwrap().value.as_deref(),
        Some("🍀-0")
    );
    assert_eq!(db.read("public", "right").unwrap().unwrap().revision, 1);
    assert_eq!(
        db.read("sedrava_native_a6", "state_test")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("state-0")
    );
    db.verify().unwrap();
    drop(db);
    let con = Connection::open(&path).unwrap();
    assert_eq!(no_receipts(&con), (3, 3, 1));
    assert_eq!(
        con.query_row("SELECT count(*) FROM receipts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(con);
    let mut db = Database::open(&path).unwrap();
    let same = db
        .commit_managed_batch(
            "app_v1",
            "req-batch-000001",
            &items,
            r#"{"saved":true,"counter":1}"#,
        )
        .unwrap();
    assert_eq!(same.revisions, first.revisions);
    assert!(same.replayed);
    assert!(db
        .commit_managed_batch("app_v1", "req-batch-000001", &items, r#"{"saved":false}"#)
        .is_err());
    assert!(db
        .commit_managed_batch(
            "app_v1",
            "req-batch-000001",
            &key_ops(1, 1),
            r#"{"saved":true,"counter":2}"#
        )
        .is_err());
    let second = db
        .commit_managed_batch(
            "app_v1",
            "req-batch-000002",
            &key_ops(1, 1),
            r#"{"saved":true,"counter":2}"#,
        )
        .unwrap();
    assert_eq!(second.revisions, vec![2, 2, 2]);
    db.verify().unwrap();
    let row = db
        .lookup_managed_batch("app_v1", "req-batch-000001")
        .unwrap()
        .unwrap();
    assert_eq!(row, first);
    drop(db);
    let con = Connection::open(&path).unwrap();
    assert_eq!(no_receipts(&con), (3, 6, 2));
    assert_eq!(
        con.query_row("SELECT count(*) FROM receipts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
}

#[test]
fn stale_any_key_rolls_back_every_key_and_receipt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.sqlite");
    let mut db = Database::init(&path).unwrap();
    db.enable_managed_batches().unwrap();
    let first = key_ops(0, 0);
    db.commit_managed_batch("app_v1", "req-batch-000001", &first, "{}")
        .unwrap();
    let original_left = db.read("public", "left").unwrap().unwrap();
    let original_state = db.read("sedrava_native_a6", "state_test").unwrap().unwrap();
    let mut stale = key_ops(1, 1);
    stale[1].expected_revision = 0;
    assert!(db
        .commit_managed_batch("app_v1", "req-batch-000002", &stale, "{}")
        .is_err());
    assert_eq!(db.read("public", "left").unwrap().unwrap(), original_left);
    assert_eq!(
        db.read("sedrava_native_a6", "state_test").unwrap().unwrap(),
        original_state
    );
    assert!(db
        .lookup_managed_batch("app_v1", "req-batch-000002")
        .unwrap()
        .is_none());
    db.verify().unwrap();
    drop(db);
    let con = Connection::open(path).unwrap();
    assert_eq!(no_receipts(&con), (3, 3, 1));
}
#[test]
fn invalid_inputs_preserve_empty_db_and_original_legacy_methods() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b.sqlite");
    let mut db = Database::init(&path).unwrap();
    // Default Database::init does NOT create the opt-in table.
    assert!(db
        .lookup_managed_batch("app_v1", "req-batch-000001")
        .is_err());
    db.enable_managed_batches().unwrap();
    let operations = key_ops(0, 0);
    for invalid in [
        vec![put("only", "x", 0)],
        vec![put("same", "x", 0), put("same", "y", 0)],
        vec![
            put("one", "a", 0),
            put("two", "b", 0),
            put("three", "c", 0),
            put("four", "d", 0),
            put("five", "e", 0),
        ],
        vec![put("one", "a", i64::MAX as u64), put("two", "b", 0)],
        vec![put("../../path", "a", 0), put("safe", "b", 0)],
        vec![put("nul", "a\0b", 0), put("other", "b", 0)],
    ] {
        assert!(db
            .commit_managed_batch("app_v1", "req-batch-000001", &invalid, "{}")
            .is_err());
    }
    assert!(db
        .commit_managed_batch("app_v1", "req-batch-000001", &operations, "not-json")
        .is_err());
    assert!(db
        .commit_managed_batch("app_v1", "bad", &operations, "{}")
        .is_err());
    db.verify().unwrap();
    let legacy = db
        .mutate(
            "public",
            "legacy",
            Change::Put("unchanged"),
            0,
            "legacy-req-000001",
        )
        .unwrap();
    assert_eq!(legacy.revision, 1);
    db.verify().unwrap();
    drop(db);
    let conn = Connection::open(path).unwrap();
    assert_eq!(no_receipts(&conn), (1, 1, 0));
}

#[test]
fn independent_writers_cas_no_partial_updates() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c.sqlite");
    let mut a = Database::init(&path).unwrap();
    a.enable_managed_batches().unwrap();
    let mut b = Database::open(&path).unwrap();
    a.commit_managed_batch("app_v1", "req-batch-000001", &key_ops(0, 0), "{}")
        .unwrap();
    let attempted = b.commit_managed_batch("app_v1", "req-batch-000002", &key_ops(0, 0), "{}");
    assert!(attempted.is_err());
    let replay = b
        .commit_managed_batch("app_v1", "req-batch-000001", &key_ops(0, 0), "{}")
        .unwrap();
    assert!(replay.replayed);
    a.verify().unwrap();
}
#[test]
fn malicious_receipt_edit_detected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("d.sqlite");
    let mut db = Database::init(&path).unwrap();
    db.enable_managed_batches().unwrap();
    db.commit_managed_batch("app_v1", "req-batch-000001", &key_ops(0, 0), "{}")
        .unwrap();
    drop(db);
    let con = Connection::open(&path).unwrap();
    let original: String = con
        .query_row("SELECT result_json FROM managed_batch_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut receipt: ManagedBatchReceipt = serde_json::from_str(&original).unwrap();
    receipt.replayed = true;
    con.execute(
        "UPDATE managed_batch_receipts SET result_json=?1",
        [serde_json::to_string(&receipt).unwrap()],
    )
    .unwrap();
    drop(con);
    let db = Database::open(&path).unwrap();
    assert!(db
        .lookup_managed_batch("app_v1", "req-batch-000001")
        .is_err());
}
