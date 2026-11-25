#![cfg(unix)]
use sedravasql_native_engine::{
    cow_migration::{migrate_copy, MigrationImage},
    sql::execute,
    NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn put(ns: &str, key: &str, revision: u64, value: &str) -> NativeMutation {
    NativeMutation {
        namespace: ns.into(),
        key: key.into(),
        expected_revision: revision,
        change: NativeChange::Put(value.into()),
    }
}
#[test]
fn migrate_full_native_state_historical_receipts_and_sql_catalog() {
    let d = dir();
    let src = d.path().join("source");
    let dst = d.path().join("image");
    let mut db = NativeLog::create(&src).unwrap();
    // Full-sized value and JSON escaping force multi-chunk receipt payloads.
    let v = "Ж".repeat(32768);
    let escaped = "\\".repeat(65536);
    let ops = [put("ns", "same", 0, &v), put("ns2", "same", 0, &escaped)];
    let first = db.commit_batch("native-req-000001", &ops).unwrap();
    db.commit_batch("native-req-000002", &[put("ns", "same", 1, "updated")])
        .unwrap();
    db.commit_batch(
        "native-req-000003",
        &[NativeMutation {
            namespace: "ns2".into(),
            key: "same".into(),
            expected_revision: 1,
            change: NativeChange::Delete,
        }],
    )
    .unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY,n INT)",
        Some("sql-create-00001"),
    )
    .unwrap();
    execute(
        &mut db,
        "INSERT INTO T (id,n) VALUES ('a',1)",
        Some("sql-insert-00001"),
    )
    .unwrap();
    execute(
        &mut db,
        "UPDATE T SET n=2 WHERE id='a'",
        Some("sql-update-00001"),
    )
    .unwrap();
    let expected = [
        db.read("ns", "same").unwrap().unwrap(),
        db.read("ns2", "same").unwrap().unwrap(),
        db.read("sedravasql_table_catalog", "T").unwrap().unwrap(),
    ];
    let seq = db.sequence();
    let usage = db.usage().unwrap();
    let sql_receipts = db
        .scan_namespace("sedravasql_sql_receipts", None, 256)
        .unwrap();
    drop(db);
    let before = fs::read(&src).unwrap();
    let manifest = migrate_copy(&src, &dst).unwrap();
    assert_eq!(manifest.source_sequence, seq);
    assert_eq!(manifest.entries, usage.keys_including_tombstones);
    assert_eq!(manifest.receipts, usage.receipts);
    assert_eq!(fs::read(&src).unwrap(), before);
    let mut image = MigrationImage::open(&dst).unwrap();
    assert_eq!(image.manifest(), &manifest);
    assert!(image.matches_source(&src).unwrap());
    assert_eq!(
        image
            .entry("sedravasql_table.T", "a")
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    for row in expected.iter().chain(sql_receipts.iter()) {
        assert_eq!(
            image.entry(&row.namespace, &row.key).unwrap().as_ref(),
            Some(row)
        );
    }
    let historic = image.receipt("native-req-000001").unwrap().unwrap();
    assert_eq!(historic.result, first);
    let replay = image.replay("native-req-000001", &ops).unwrap().unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.sequence, first.sequence);
    assert_eq!(replay.entries, first.entries);
    assert!(image
        .replay("native-req-000001", &[put("ns", "same", 0, "different")])
        .is_err());
    assert!(image.entry("ns", "missing").unwrap().is_none());
    assert!(image.receipt("missing-req-00001").unwrap().is_none());
    let mut db = NativeLog::open(&src).unwrap();
    db.commit_batch("native-req-000004", &[put("ns", "same", 2, "later")])
        .unwrap();
    drop(db);
    assert!(!image.matches_source(&src).unwrap());
    let after = fs::read(&src).unwrap();
    drop(image);
    assert!(migrate_copy(&src, &dst).is_err());
    assert_eq!(fs::read(&src).unwrap(), after);
}
#[test]
fn empty_source_and_alias_rejection() {
    let d = dir();
    let src = d.path().join("source");
    let dst = d.path().join("image");
    drop(NativeLog::create(&src).unwrap());
    let before = fs::read(&src).unwrap();
    for p in [&src, d.path().join("source.lock").as_path()] {
        assert!(migrate_copy(&src, p).is_err());
    }
    let m = migrate_copy(&src, &dst).unwrap();
    assert_eq!(m.source_sequence, 0);
    assert_eq!(m.entries, 0);
    assert_eq!(m.receipts, 0);
    assert_eq!(MigrationImage::open(&dst).unwrap().manifest(), &m);
    assert_eq!(fs::read(&src).unwrap(), before);
}
#[test]
fn corrupt_source_refused_and_no_destination() {
    let d = dir();
    let src = d.path().join("source");
    let dst = d.path().join("image");
    let mut db = NativeLog::create(&src).unwrap();
    db.commit_batch("native-req-000001", &[put("ns", "a", 0, "a")])
        .unwrap();
    drop(db);
    let mut data = fs::read(&src).unwrap();
    let n = data.len();
    data[n - 1] ^= 1;
    fs::write(&src, &data).unwrap();
    assert!(migrate_copy(&src, &dst).is_err());
    assert!(!dst.exists());
    assert_eq!(fs::read(&src).unwrap(), data);
}

#[test]
fn migration_cli_round_trip() {
    use std::process::Command;
    let d = dir();
    let src = d.path().join("source");
    let dst = d.path().join("image");
    let mut db = NativeLog::create(&src).unwrap();
    db.commit_batch("native-req-000001", &[put("ns", "a", 0, "hello")])
        .unwrap();
    drop(db);
    let cli = env!("CARGO_BIN_EXE_sedravasql-native-preview");
    let run = |args: &[&str]| {
        let r = Command::new(cli).args(args).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        serde_json::from_slice::<serde_json::Value>(&r.stdout).unwrap()
    };
    let s = src.to_str().unwrap();
    let t = dst.to_str().unwrap();
    assert_eq!(run(&["migration-copy", s, t])["source_sequence"], 1);
    assert_eq!(run(&["migration-info", t])["entries"], 1);
    assert_eq!(run(&["migration-read", t, "ns", "a"])["value"], "hello");
    assert_eq!(
        run(&["migration-receipt", t, "native-req-000001"])["result"]["sequence"],
        1
    );
    assert_eq!(run(&["migration-match-log", t, s])["matches"], true);
}
