#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, execute_transaction, BoundStatement, SqlResult},
    table::{Cell, Tables},
    Error, NativeLog,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn b(sql: &str) -> BoundStatement {
    BoundStatement {
        sql: sql.into(),
        params: vec![],
    }
}
#[test]
fn foreign_keys_and_atomic_transactions() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE P (id TEXT PRIMARY KEY, name TEXT)",
        Some("setup-parent-001"),
    )
    .unwrap();
    execute(
        &mut db,
        "CREATE TABLE C (id TEXT PRIMARY KEY, parent TEXT)",
        Some("setup-child-0001"),
    )
    .unwrap();
    let fk = "ALTER TABLE C ADD FOREIGN KEY (parent) REFERENCES P(id)";
    execute(
        &mut db,
        "INSERT INTO C (id,parent) VALUES ('orphan','p0')",
        Some("orphan-create-01"),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        execute(&mut db, fk, Some("foreign-key-0001")),
        Err(Error::Conflict(_))
    ));
    assert_eq!(before, fs::read(&path).unwrap());
    execute(
        &mut db,
        "DELETE FROM C WHERE id='orphan'",
        Some("orphan-delete-01"),
    )
    .unwrap();
    execute(&mut db, fk, Some("foreign-key-0001")).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        execute(
            &mut db,
            "INSERT INTO C (id,parent) VALUES ('c1','p1')",
            Some("insert-child-001")
        ),
        Err(Error::Conflict(_))
    ));
    assert_eq!(before, fs::read(&path).unwrap());
    let tx = [
        b("INSERT INTO C (id,parent) VALUES ('c1','p1')"),
        b("INSERT INTO P (id,name) VALUES ('p1','parent')"),
    ];
    let first = execute_transaction(&mut db, &tx, "tx-insert-00001").unwrap();
    assert!(matches!(
        first,
        SqlResult::Write {
            affected: 2,
            replayed: false,
            ..
        }
    ));
    let before = fs::read(&path).unwrap();
    assert!(execute(
        &mut db,
        "DELETE FROM P WHERE id='p1'",
        Some("parent-delete-01")
    )
    .is_err());
    assert!(Tables::new(&mut db)
        .delete("typed-delete-001", "P", "p1", 1)
        .is_err());
    assert!(execute_transaction(
        &mut db,
        &[
            b("INSERT INTO P (id,name) VALUES ('p2','test')"),
            b("INSERT INTO C (id,parent) VALUES ('c2','absent')")
        ],
        "tx-fail-000001"
    )
    .is_err());
    assert!(execute_transaction(
        &mut db,
        &[
            b("UPDATE P SET name='a' WHERE id='p1'"),
            b("UPDATE P SET name='b' WHERE id='p1'")
        ],
        "tx-repeat-00001"
    )
    .is_err());
    assert!(execute_transaction(
        &mut db,
        &[b("UPDATE P SET name='a' WHERE name='parent'")],
        "tx-nonpk-000001"
    )
    .is_err());
    assert!(execute_transaction(
        &mut db,
        &[b("DELETE FROM C WHERE id='absent'")],
        "tx-missing-0001"
    )
    .is_err());
    assert!(execute_transaction(&mut db, &[b("SELECT * FROM P")], "tx-select-00001").is_err());
    assert!(execute_transaction(
        &mut db,
        &[b("DELETE FROM C WHERE id='c1'")],
        "tx-insert-00001"
    )
    .is_err());
    assert_eq!(before, fs::read(&path).unwrap());
    assert_eq!(
        execute(&mut db, "SELECT id FROM P ORDER BY id", None).unwrap(),
        SqlResult::Rows {
            columns: vec!["id".into()],
            rows: vec![vec![Cell::Text("p1".into())]]
        }
    );
    let tx_delete = [
        b("DELETE FROM P WHERE id='p1'"),
        b("DELETE FROM C WHERE id='c1'"),
    ];
    execute_transaction(&mut db, &tx_delete, "tx-delete-00001").unwrap();
    db.verify().unwrap();
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        execute_transaction(&mut db, &tx, "tx-insert-00001").unwrap(),
        SqlResult::Write {
            affected: 2,
            replayed: true,
            ..
        }
    ));
    assert!(matches!(
        execute_transaction(&mut db, &tx_delete, "tx-delete-00001").unwrap(),
        SqlResult::Write {
            affected: 2,
            replayed: true,
            ..
        }
    ));
    assert_eq!(before, fs::read(&path).unwrap());
}
#[test]
fn foreign_key_typed_writes_and_bound_transaction() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    for (i, q) in [
        "CREATE TABLE P (id TEXT PRIMARY KEY)",
        "CREATE TABLE C (id TEXT PRIMARY KEY, parent TEXT)",
        "ALTER TABLE C ADD FOREIGN KEY (parent) REFERENCES P(id)",
    ]
    .iter()
    .enumerate()
    {
        execute(&mut db, q, Some(&format!("setup-fk-{i:06}"))).unwrap();
    }
    let tx = [
        BoundStatement {
            sql: "INSERT INTO P (id) VALUES (?)".into(),
            params: vec![Cell::Text("p1".into())],
        },
        BoundStatement {
            sql: "INSERT INTO C (id,parent) VALUES (?,?)".into(),
            params: vec![Cell::Text("c1".into()), Cell::Text("p1".into())],
        },
    ];
    execute_transaction(&mut db, &tx, "tx-parameter-001").unwrap();
    let before = fs::read(&path).unwrap();
    let invalid = std::collections::BTreeMap::from([
        ("id".into(), Cell::Text("c1".into())),
        ("parent".into(), Cell::Text("not-found".into())),
    ]);
    assert!(Tables::new(&mut db)
        .put("typed-update-001", "C", 1, invalid)
        .is_err());
    assert!(execute(
        &mut db,
        "ALTER TABLE C ADD FOREIGN KEY (parent) REFERENCES P(id)",
        Some("duplicate-fk-001")
    )
    .is_err());
    assert!(execute(
        &mut db,
        "ALTER TABLE C ADD FOREIGN KEY (id) REFERENCES C(id)",
        Some("self-fk-0000001")
    )
    .is_err());
    assert_eq!(before, fs::read(&path).unwrap());
}
