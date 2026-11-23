use sedravasql::{Change, Database};
use tempfile::tempdir;

#[test]
fn sqlite_idempotency_cas_audit_preview_and_tombstones() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("data.sqlite");
    let mut db = Database::init(&path).unwrap();
    let plan = db.preview("accounts", "alpha", Change::Put("one")).unwrap();
    assert_eq!(plan.expected_revision, 0);
    assert!(!plan.existing);
    let done = db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("one"),
            0,
            "request-00000001",
        )
        .unwrap();
    assert_eq!(done.revision, 1);
    assert!(!done.replayed);
    let repeat = db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("one"),
            0,
            "request-00000001",
        )
        .unwrap();
    assert_eq!(repeat.audit_id, done.audit_id);
    assert!(repeat.replayed);
    let err = db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("two"),
            0,
            "request-00000001",
        )
        .unwrap_err();
    assert!(err.to_string().contains("CONFLICT"));
    assert!(db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("two"),
            0,
            "request-00000002"
        )
        .is_err());
    let second = db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("two"),
            1,
            "request-00000002",
        )
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        db.read("accounts", "alpha")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("two")
    );
    assert!(db
        .mutate(
            "accounts",
            "alpha",
            Change::Delete { confirm: false },
            2,
            "request-00000003"
        )
        .is_err());
    let deletion = db
        .mutate(
            "accounts",
            "alpha",
            Change::Delete { confirm: true },
            2,
            "request-00000003",
        )
        .unwrap();
    assert_eq!(deletion.revision, 3);
    assert_eq!(db.read("accounts", "alpha").unwrap().unwrap().value, None);
    assert!(db.list("accounts").unwrap().is_empty());
    assert!(db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("old"),
            0,
            "request-00000004"
        )
        .is_err());
    let recreated = db
        .mutate(
            "accounts",
            "alpha",
            Change::Put("fresh"),
            3,
            "request-00000004",
        )
        .unwrap();
    assert_eq!(recreated.revision, 4);
    let audit = db.audit(10).unwrap();
    assert_eq!(audit.len(), 4);
    assert_eq!(audit[0].after_revision, 4);
    assert_eq!(audit[3].before_revision, 0);
    db.verify().unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.read("accounts", "alpha").unwrap().unwrap().revision, 4);
}
#[test]
fn sqlite_atomic_backup_is_reopenable_with_wal_enabled() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("active.sqlite");
    let backup = tmp.path().join("backup.sqlite");
    let mut db = Database::init(&path).unwrap();
    db.mutate("n", "k", Change::Put("данные"), 0, "backup-request-001")
        .unwrap();
    db.backup(&backup).unwrap();
    assert!(db.backup(&backup).is_err()); // no silent overwrite
    db.mutate("n", "k", Change::Put("new"), 1, "backup-request-002")
        .unwrap();
    let restored = Database::open(&backup).unwrap();
    restored.verify().unwrap();
    assert_eq!(
        restored.read("n", "k").unwrap().unwrap().value.as_deref(),
        Some("данные")
    );
    assert_eq!(restored.audit(20).unwrap().len(), 1);
    assert_eq!(db.audit(20).unwrap().len(), 2);
}
#[test]
fn sqlite_concurrent_writers_serialized_and_stale_preview_rejected() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("parallel.sqlite");
    let initial = Database::init(&file).unwrap();
    let plan = initial.preview("org", "item", Change::Put("A")).unwrap();
    drop(initial);
    let mut threads = Vec::new();
    for i in 0..8 {
        let file = file.clone();
        threads.push(std::thread::spawn(move || {
            let mut db = Database::open(&file).unwrap();
            db.mutate(
                "org",
                "item",
                Change::Put(if i == 0 { "A" } else { "B" }),
                0,
                &format!("parallel-request-{i:03}"),
            )
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|x| x.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|x| x.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|x| x.is_err()).count(), 7);
    let db = Database::open(&file).unwrap();
    assert_eq!(db.audit(20).unwrap().len(), 1);
    assert_eq!(db.read("org", "item").unwrap().unwrap().revision, 1);
    assert_eq!(plan.expected_revision, 0);
}
#[test]
fn sqlite_foreign_schema_rejected_without_overwrite() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("foreign.sqlite");
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute_batch(
        "CREATE TABLE private_data (secret TEXT);INSERT INTO private_data VALUES('keep');",
    )
    .unwrap();
    drop(conn);
    assert!(Database::init(&file).is_err());
    let conn = rusqlite::Connection::open(&file).unwrap();
    let s: String = conn
        .query_row("SELECT secret FROM private_data", [], |r| r.get(0))
        .unwrap();
    assert_eq!(s, "keep");
}

#[cfg(unix)]
#[test]
fn sqlite_db_file_rejects_public_modes_and_hardlinks() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let file = temp.path().join("secure.sqlite");
    let _db = Database::init(&file).unwrap();
    let meta = std::fs::metadata(&file).unwrap();
    assert_eq!(meta.permissions().mode() & 0o077, 0);
    drop(_db);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Database::open(&file).is_err());
    assert!(Database::init(&file).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = temp.path().join("hardlink.sqlite");
    std::fs::hard_link(&file, &link).unwrap();
    assert!(Database::open(&file).is_err());
    assert!(Database::init(&file).is_err());
    std::fs::remove_file(link).unwrap();
    assert!(Database::open(&file).is_ok());
}

#[test]
fn durable_application_responses_survive_restart_and_do_not_evict_at_32() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("replay.sqlite");
    let mut db = Database::init(&file).unwrap();
    for index in 0..45 {
        let key = format!("request-app-{index:06}");
        let body = format!(r#"{{"count":{}}}"#, index + 1);
        let result = db
            .mutate_with_response("server_a6", "state", Change::Put(&body), index, &key, &body)
            .unwrap();
        assert_eq!(result.revision, index + 1);
        assert_eq!(result.application_response.as_deref(), Some(body.as_str()));
        let previous = db.lookup_receipt("server_a6", &key).unwrap().unwrap();
        assert_eq!(
            previous.application_response.as_deref(),
            Some(body.as_str())
        );
        let duplicate = db
            .mutate_with_response("server_a6", "state", Change::Put(&body), index, &key, &body)
            .unwrap();
        assert!(duplicate.replayed);
        assert!(db
            .mutate_with_response(
                "server_a6",
                "state",
                Change::Put("different"),
                index,
                &key,
                &body
            )
            .is_err());
    }
    db.verify().unwrap();
    drop(db);
    let db = Database::open(&file).unwrap();
    assert_eq!(db.read("server_a6", "state").unwrap().unwrap().revision, 45);
    assert!(db
        .lookup_receipt("server_a6", "request-app-000000")
        .unwrap()
        .is_some());
    assert!(db
        .lookup_receipt("server_a6", "request-app-000044")
        .unwrap()
        .is_some());
    assert!(db
        .lookup_receipt("server_a6", "missing-key-000000")
        .unwrap()
        .is_none());
}
