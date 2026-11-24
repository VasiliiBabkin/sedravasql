use sedravasql::{managed_batch::ManagedPut, Change, Database, Entry, Error};
use std::path::Path;
use std::sync::{Arc, Barrier};
use tempfile::tempdir;

fn put(key: &str, revision: u64) -> ManagedPut {
    ManagedPut {
        namespace: "public_notes".into(),
        key: key.into(),
        value: format!("version-{revision}"),
        expected_revision: revision,
    }
}

fn batch(db: &mut Database, revision: u64) {
    let ops = [put("left", revision), put("right", revision)];
    let request_key = format!("b7-batch-write-{revision:08}");
    let result = db
        .commit_managed_batch(
            "b7_snapshot",
            &request_key,
            &ops,
            &format!(r#"{{"version":{}}}"#, revision + 1),
        )
        .unwrap();
    assert_eq!(result.revisions, vec![revision + 1, revision + 1]);
}

fn create(path: &Path) -> Database {
    let mut db = Database::init(path).unwrap();
    db.enable_managed_batches().unwrap();
    db
}

fn assert_same(versions: &[Option<Entry>]) {
    assert_eq!(versions.len(), 2);
    let a = versions[0].as_ref().unwrap();
    let b = versions[1].as_ref().unwrap();
    assert_eq!(a.revision, b.revision, "mixed revisions across keys");
    assert_eq!(a.value, b.value, "mixed values across keys");
    assert_eq!(
        a.value.as_deref(),
        Some(format!("version-{}", a.revision - 1).as_str())
    );
}

#[test]
fn snapshot_uses_same_sqlite_read_transaction_for_two_keys() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite");
    let mut writer = create(&path);
    batch(&mut writer, 0);

    let reader = Database::open(&path).unwrap();
    let before = reader
        .read_snapshot(&[("public_notes", "left"), ("public_notes", "right")])
        .unwrap();
    assert_same(&before);
    assert_eq!(before[0].as_ref().unwrap().revision, 1);

    let gates = Arc::new(Barrier::new(2));
    let writer_gate = gates.clone();
    let thread = std::thread::spawn(move || {
        writer_gate.wait();
        for revision in 1..=180 {
            batch(&mut writer, revision);
            if revision % 5 == 0 {
                std::thread::yield_now();
            }
        }
    });
    gates.wait();
    // Atomic batch writers can commit between SQL SELECT statements; the
    // read-only transaction must never combine two commit generations.
    for _ in 0..900 {
        let values = reader
            .read_snapshot(&[("public_notes", "left"), ("public_notes", "right")])
            .unwrap();
        assert_same(&values);
        std::thread::yield_now();
    }
    thread.join().unwrap();
    let result = reader
        .read_snapshot(&[("public_notes", "left"), ("public_notes", "right")])
        .unwrap();
    assert_same(&result);
    assert_eq!(result[0].as_ref().unwrap().revision, 181);
    reader.verify().unwrap();
}

#[test]
fn input_bounds_and_duplicate_keys_fail_before_sql_transaction() {
    let dir = tempdir().unwrap();
    let db = create(&dir.path().join("bounded.sqlite"));
    for inputs in [
        vec![],
        vec![("public_notes", "same"), ("public_notes", "same")],
        vec![("public_notes", "../../outside")],
        vec![("sedrava", "")],
    ] {
        assert!(matches!(db.read_snapshot(&inputs), Err(Error::Invalid(_))));
    }
    let big_keys: Vec<String> = (0..17).map(|i| format!("key_{i}")).collect();
    let big: Vec<(&str, &str)> = big_keys
        .iter()
        .map(|s| ("public_notes", s.as_str()))
        .collect();
    assert!(matches!(db.read_snapshot(&big), Err(Error::Invalid(_))));
    let empty = db.read_snapshot(&[("public_notes", "missing")]).unwrap();
    assert_eq!(empty, vec![None]);
    db.verify().unwrap();
}

#[test]
fn distinguishes_missing_from_tombstone_and_read_is_nonmutating() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tombstone.sqlite");
    let mut db = create(&path);
    db.mutate(
        "public_notes",
        "deleted",
        Change::Put("before"),
        0,
        "b7-put-delete-seed-0001",
    )
    .unwrap();
    db.mutate(
        "public_notes",
        "deleted",
        Change::Delete { confirm: true },
        1,
        "b7-remove-key-0000002",
    )
    .unwrap();
    let results = db
        .read_snapshot(&[("public_notes", "deleted"), ("public_notes", "missing")])
        .unwrap();
    assert_eq!(results[0].as_ref().unwrap().revision, 2);
    assert_eq!(results[0].as_ref().unwrap().value, None);
    assert_eq!(results[1], None);
    assert_eq!(db.audit(100).unwrap().len(), 2);
    db.verify().unwrap();
}
