#![cfg(unix)]
use sedravasql_native_engine::{
    cow_migration::{migrate_copy, restore_log_copy, MigrationImage},
    sql::{execute, execute_sequential, BoundStatement},
    NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
use sha2::{Digest, Sha256};
#[cfg(debug_assertions)]
use std::process::Command;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn side(p: &Path, s: &str) -> PathBuf {
    let mut n = p.file_name().unwrap().to_os_string();
    n.push(s);
    p.with_file_name(n)
}
fn op(v: &str) -> NativeMutation {
    NativeMutation {
        namespace: "ns".into(),
        key: "a".into(),
        expected_revision: 0,
        change: NativeChange::Put(v.into()),
    }
}
#[test]
fn exact_round_trip_sql_and_native_receipts() {
    let d = dir();
    let src = d.path().join("source");
    let image = d.path().join("image");
    let restored = d.path().join("restored");
    let mut db = NativeLog::create(&src).unwrap();
    let ops = [op(&"Ж".repeat(32768))];
    db.commit_batch("native-req-000001", &ops).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY,n INT)",
        Some("sql-create-00001"),
    )
    .unwrap();
    let tx = [
        "INSERT INTO T (id,n) VALUES ('a',1)",
        "SELECT n FROM T WHERE id='a'",
        "UPDATE T SET n=2 WHERE id='a'",
    ]
    .map(|sql| BoundStatement {
        sql: sql.into(),
        params: vec![],
    });
    let result = execute_sequential(&mut db, &tx, "seq-transaction-001").unwrap();
    let expected = execute(&mut db, "SELECT n FROM T WHERE id='a'", None).unwrap();
    drop(db);
    let raw = fs::read(&src).unwrap();
    let m = migrate_copy(&src, &image).unwrap();
    let before = (
        fs::read(&image).unwrap(),
        fs::read(side(&image, ".head")).unwrap(),
    );
    assert_eq!(restore_log_copy(&image, &restored).unwrap(), m);
    assert_eq!(fs::read(&restored).unwrap(), raw);
    assert_eq!(fs::read(&src).unwrap(), raw);
    assert_eq!(
        before,
        (
            fs::read(&image).unwrap(),
            fs::read(side(&image, ".head")).unwrap()
        )
    );
    let mut db = NativeLog::open(&restored).unwrap();
    assert!(db.commit_batch("native-req-000001", &ops).unwrap().replayed);
    let replay = execute_sequential(&mut db, &tx, "seq-transaction-001").unwrap();
    assert_eq!(serde_json::to_value(replay).unwrap(), {
        let mut v = serde_json::to_value(result).unwrap();
        v["replayed"] = true.into();
        v
    });
    assert_eq!(
        execute(&mut db, "SELECT n FROM T WHERE id='a'", None).unwrap(),
        expected
    );
    drop(db);
    assert!(MigrationImage::open(&image)
        .unwrap()
        .matches_source(&restored)
        .unwrap());
    assert!(restore_log_copy(&image, &restored).is_err());
    assert_eq!(fs::read(&restored).unwrap(), raw);
}
#[test]
fn empty_and_noncanonical_source() {
    let d = dir();
    let src = d.path().join("source");
    let image = d.path().join("image");
    let restored = d.path().join("restored");
    drop(NativeLog::create(&src).unwrap());
    migrate_copy(&src, &image).unwrap();
    restore_log_copy(&image, &restored).unwrap();
    assert_eq!(fs::read(&restored).unwrap(), fs::read(&src).unwrap());
    let p = d.path().join("noncanonical");
    let mut db = NativeLog::create(&p).unwrap();
    db.commit_batch("native-req-000001", &[op("a")]).unwrap();
    drop(db);
    let original = fs::read(&p).unwrap();
    let n = u32::from_be_bytes(original[8..12].try_into().unwrap()) as usize;
    let mut payload = original[12..12 + n].to_vec();
    payload.push(b' ');
    let len = (payload.len() as u32).to_be_bytes();
    let mut hash = Sha256::new();
    hash.update(Sha256::digest(b"SEDRLOG1"));
    hash.update(len);
    hash.update(&payload);
    let mut modified = b"SEDRLOG1".to_vec();
    modified.extend(len);
    modified.extend(payload);
    modified.extend(hash.finalize());
    fs::write(&p, &modified).unwrap();
    assert!(NativeLog::open(&p).is_ok());
    let q = d.path().join("noncanonical-image");
    let dest = d.path().join("no-output");
    migrate_copy(&p, &q).unwrap();
    assert!(restore_log_copy(&q, &dest).is_err());
    assert!(!dest.exists());
    assert!(!side(&dest, ".migration.tmp").exists());
}
#[test]
fn restore_child() {
    let Ok(image) = std::env::var("B34B_IMAGE") else {
        return;
    };
    let dst = std::env::var("B34B_DST").unwrap();
    restore_log_copy(Path::new(&image), Path::new(&dst)).unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn crash_at_stage_and_link_fails_closed() {
    for point in [
        "migration_restore_after_stage",
        "migration_restore_after_link",
    ] {
        let d = dir();
        let src = d.path().join("source");
        let image = d.path().join("image");
        let dst = d.path().join("restored");
        let mut db = NativeLog::create(&src).unwrap();
        db.commit_batch("native-req-000001", &[op("x")]).unwrap();
        drop(db);
        migrate_copy(&src, &image).unwrap();
        let raw = fs::read(&src).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "restore_child", "--nocapture"])
            .env("B34B_IMAGE", &image)
            .env("B34B_DST", &dst)
            .env("SEDRAVASQL_NATIVE_TEST_FAULT", point)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert_eq!(child.wait().unwrap().code(), Some(86));
        for lock in [side(&image, ".lock"), side(&dst, ".lock")] {
            assert_eq!(
                fs::read_to_string(&lock).unwrap().trim(),
                format!("pid={pid}")
            );
            fs::remove_file(lock).unwrap();
        }
        let staging = side(&dst, ".migration.tmp");
        assert_eq!(fs::read(&staging).unwrap(), raw);
        assert!(NativeLog::open(&dst).is_err());
        assert!(restore_log_copy(&image, &dst).is_err());
        if point == "migration_restore_after_link" {
            assert_eq!(fs::read(&dst).unwrap(), raw);
            fs::remove_file(&staging).unwrap();
        } else {
            assert!(!dst.exists());
            fs::remove_file(&staging).unwrap();
            restore_log_copy(&image, &dst).unwrap();
        }
        assert!(MigrationImage::open(&image)
            .unwrap()
            .matches_source(&dst)
            .unwrap());
    }
}
