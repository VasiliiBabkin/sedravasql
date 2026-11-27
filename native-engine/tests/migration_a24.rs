use sedravasql_native_engine::{
    migration::{self, Plan, Step},
    sql::{self, SqlResult},
    NativeChange, NativeLimits, NativeLog, NativeMutation, TransactionalStore,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

fn plan(sqls: &[&str]) -> Plan {
    Plan {
        version: 1,
        steps: sqls
            .iter()
            .enumerate()
            .map(|(i, sql)| Step {
                version: i as u32 + 1,
                name: format!("step_{}", i + 1),
                sql: sql.to_string(),
            })
            .collect(),
    }
}
const PARENT: &str = "CREATE TABLE Products (id TEXT PRIMARY KEY, title TEXT, price INT)";
const CHILD: &str = "CREATE TABLE Orders (id TEXT PRIMARY KEY, product_id TEXT, amount INT)";
const INDEX: &str = "CREATE INDEX products_title ON Products(title)";
const FK: &str = "ALTER TABLE Orders ADD FOREIGN KEY (product_id) REFERENCES Products(id)";
#[test]
fn applies_and_reopens_without_rewriting() {
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let initial = plan(&[PARENT, CHILD]);
    let status = migration::status(&db, &initial).unwrap();
    assert_eq!(status.current_version, 0);
    assert_eq!(status.pending, vec![1, 2]);
    assert_eq!(
        migration::apply(&mut db, &initial).unwrap().current_version,
        2
    );
    sql::execute(
        &mut db,
        "INSERT INTO Products (id,title,price) VALUES ('p','Tea',10)",
        Some("seed-product-001"),
    )
    .unwrap();
    sql::execute(
        &mut db,
        "INSERT INTO Orders (id,product_id,amount) VALUES ('o','p',20)",
        Some("seed-order-0001"),
    )
    .unwrap();
    let target = plan(&[PARENT, CHILD, INDEX, FK]);
    assert_eq!(migration::status(&db, &target).unwrap().pending, vec![3, 4]);
    let done = migration::apply(&mut db, &target).unwrap();
    assert_eq!(done.current_version, 4);
    assert_eq!(
        done.applied.iter().map(|x| x.sequence).collect::<Vec<_>>(),
        vec![1, 2, 5, 6]
    );
    assert!(done.pending.is_empty());
    assert!(sql::execute(
        &mut db,
        "INSERT INTO Orders (id,product_id,amount) VALUES ('bad','absent',10)",
        Some("bad-order-0001")
    )
    .is_err());
    assert!(
        matches!(sql::execute(&mut db,"SELECT id FROM Products WHERE title='Tea'",None).unwrap(),SqlResult::Rows{rows,..} if rows.len()==1)
    );
    db.verify().unwrap();
    let before = fs::read(&path).unwrap();
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(
        migration::apply(&mut db, &target).unwrap().current_version,
        4
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(migration::status(&db, &initial).is_err());
    let mut changed = target.clone();
    changed.steps[0].sql.push(' ');
    assert!(migration::apply(&mut db, &changed).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn refusal_is_atomic_per_version_and_repair_is_resumable() {
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let base = plan(&[PARENT, CHILD]);
    migration::apply(&mut db, &base).unwrap();
    sql::execute(
        &mut db,
        "INSERT INTO Orders (id,product_id,amount) VALUES ('o','p',20)",
        Some("orphan-order-001"),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let target = plan(&[PARENT, CHILD, FK]);
    assert!(migration::apply(&mut db, &target).is_err());
    assert_eq!(migration::status(&db, &target).unwrap().pending, vec![3]);
    assert_eq!(fs::read(&path).unwrap(), before);
    sql::execute(
        &mut db,
        "INSERT INTO Products (id,title,price) VALUES ('p','Tea',10)",
        Some("fix-parent-001"),
    )
    .unwrap();
    assert_eq!(
        migration::apply(&mut db, &target).unwrap().current_version,
        3
    );
    db.verify().unwrap();
}
#[test]
fn prechecks_entire_plan_and_refuses_capacity_and_drift() {
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let before = fs::read(&path).unwrap();
    let invalid = plan(&[PARENT, "DELETE FROM Products WHERE id='p'"]);
    assert!(migration::apply(&mut db, &invalid).is_err());
    let mut gap = plan(&[PARENT, CHILD]);
    gap.steps[1].version = 3;
    assert!(migration::apply(&mut db, &gap).is_err());
    let mut dup = plan(&[PARENT, CHILD]);
    dup.steps[1].name = dup.steps[0].name.clone();
    assert!(migration::apply(&mut db, &dup).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let target = plan(&[PARENT, CHILD]);
    db.set_limits(NativeLimits {
        max_bytes: 64 * 1024 * 1024,
        max_transactions: 1,
    })
    .unwrap();
    assert!(migration::apply(&mut db, &target).is_err());
    assert_eq!(migration::status(&db, &target).unwrap().pending, vec![2]);
    let before = fs::read(&path).unwrap();
    assert!(migration::apply(&mut db, &target).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    db.set_limits(NativeLimits::default()).unwrap();
    migration::apply(&mut db, &target).unwrap();
    let row = db
        .read("sedravasql_table_catalog", "Products")
        .unwrap()
        .unwrap();
    // Raw mutations are trusted-only; detect drift even when value remains identical.
    db.commit_batch(
        "test-drift-0001",
        &[NativeMutation {
            namespace: row.namespace,
            key: row.key,
            expected_revision: 1,
            change: NativeChange::Put(row.value.unwrap()),
        }],
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(migration::status(&db, &target).is_err());
    assert!(migration::apply(&mut db, &target).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[cfg(debug_assertions)]
#[test]
fn lost_response_after_sync_is_recoverable() {
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let file = dir.path().join("plan.json");
    let db = NativeLog::create(&path).unwrap();
    drop(db);
    fs::write(&file, serde_json::to_vec(&plan(&[PARENT, CHILD])).unwrap()).unwrap();
    // The existing debug fault hook is exercised in a subprocess (no external data).
    let bin = env!("CARGO_BIN_EXE_sedravasql-native-preview");
    let result = std::process::Command::new(bin)
        .args([
            "migration-apply",
            path.to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .env("SEDRAVASQL_NATIVE_TEST_FAULT", "after_sync")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(86), "expected injected exit");
    let lock = path.with_file_name("db.lock");
    if lock.exists() {
        fs::remove_file(lock).unwrap();
    }
    let mut db = NativeLog::open(&path).unwrap();
    let target = plan(&[PARENT, CHILD]);
    assert_eq!(migration::status(&db, &target).unwrap().current_version, 1);
    assert_eq!(
        migration::apply(&mut db, &target).unwrap().current_version,
        2
    );
    db.verify().unwrap();
}

#[test]
fn unmanaged_catalog_is_not_silently_adopted() {
    let dir = tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    sql::execute(&mut db, PARENT, Some("manual-create-001")).unwrap();
    let target = plan(&[PARENT, CHILD]);
    assert_eq!(migration::status(&db, &target).unwrap().current_version, 0);
    let before = fs::read(&path).unwrap();
    assert!(migration::apply(&mut db, &target).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(migration::status(&db, &target).unwrap().pending, vec![1, 2]);
}

#[test]
fn manifest_read_is_bounded_and_strict() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("manifest.json");
    fs::write(&file, serde_json::to_vec(&plan(&[PARENT])).unwrap()).unwrap();
    assert_eq!(Plan::read(&file).unwrap().steps.len(), 1);
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(Plan::read(&link).is_err());
    assert!(Plan::read(dir.path()).is_err());
    fs::write(&file, vec![b' '; 65537]).unwrap();
    assert!(Plan::read(&file).is_err());
    assert!(Plan::parse(br#"{"version":1,"version":1,"steps":[]}"#).is_err());
    assert!(Plan::parse(br#"{"version":1,"steps":[],"unknown":true}"#).is_err());
}
