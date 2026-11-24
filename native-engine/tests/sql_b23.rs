#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, execute_sequential, BoundStatement, SqlResult},
    table::{Cell, Tables},
    NativeLog,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn b(sql: &str) -> BoundStatement {
    BoundStatement {
        sql: sql.into(),
        params: vec![],
    }
}
fn rows(v: Vec<Vec<Cell>>) -> SqlResult {
    SqlResult::Rows {
        columns: vec!["n".into()],
        rows: v,
    }
}
#[test]
fn sequential_reads_coalesce_and_replay() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY,n INT)",
        Some("seq-create-0001"),
    )
    .unwrap();
    let tx = [
        b("INSERT INTO T (id,n) VALUES ('a',1)"),
        b("SELECT n FROM T WHERE id='a'"),
        b("UPDATE T SET n=2 WHERE id='a' AND n=1"),
        b("SELECT n FROM T WHERE id='a'"),
        b("UPDATE T SET n=3 WHERE id='a'"),
    ];
    let first = execute_sequential(&mut db, &tx, "seq-transaction-001").unwrap();
    assert_eq!(
        first,
        SqlResult::Transaction {
            affected: 1,
            sequence: 2,
            replayed: false,
            reads: vec![
                rows(vec![vec![Cell::Int(1)]]),
                rows(vec![vec![Cell::Int(2)]])
            ]
        }
    );
    assert_eq!(
        Tables::new(&mut db)
            .get("T", "a")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    let before = fs::read(&path).unwrap();
    for tx in [
        vec![
            b("UPDATE T SET n=4 WHERE id='a'"),
            b("UPDATE T SET n=5 WHERE id='a' AND n=3"),
        ],
        vec![b("SELECT n FROM T WHERE n=3")],
        vec![b("SELECT n FROM T WHERE id='a'")],
        vec![b("INSERT INTO T (id,n) VALUES ('a',9)")],
    ] {
        assert!(execute_sequential(&mut db, &tx, "seq-bad-000001").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let tx2 = [
        b("DELETE FROM T WHERE id='a'"),
        b("SELECT n FROM T WHERE id='a'"),
        b("INSERT INTO T (id,n) VALUES ('a',9)"),
        b("SELECT n FROM T WHERE id='a'"),
    ];
    assert!(matches!(
        execute_sequential(&mut db, &tx2, "seq-transaction-002").unwrap(),
        SqlResult::Transaction { affected: 1, .. }
    ));
    assert_eq!(
        Tables::new(&mut db)
            .get("T", "a")
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    let net = [
        b("INSERT INTO T (id,n) VALUES ('b',1)"),
        b("DELETE FROM T WHERE id='b'"),
    ];
    assert!(matches!(
        execute_sequential(&mut db, &net, "seq-transaction-003").unwrap(),
        SqlResult::Transaction { affected: 0, .. }
    ));
    assert!(Tables::new(&mut db).get("T", "b").unwrap().is_none());
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(
        execute_sequential(&mut db, &tx, "seq-transaction-001").unwrap(),
        SqlResult::Transaction {
            affected: 1,
            sequence: 2,
            replayed: true,
            reads: vec![
                rows(vec![vec![Cell::Int(1)]]),
                rows(vec![vec![Cell::Int(2)]])
            ]
        }
    );
    assert_eq!(before, fs::read(&path).unwrap());
    assert!(execute_sequential(&mut db, &tx2, "seq-transaction-001").is_err());
}
#[test]
fn sequential_fk_final_state_and_rejection() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    for (i, q) in [
        "CREATE TABLE P (id TEXT PRIMARY KEY)",
        "CREATE TABLE C (id TEXT PRIMARY KEY,p TEXT)",
        "ALTER TABLE C ADD FOREIGN KEY (p) REFERENCES P(id)",
    ]
    .iter()
    .enumerate()
    {
        execute(&mut db, q, Some(&format!("seq-schema-{i:04}"))).unwrap();
    }
    let tx = [
        b("INSERT INTO C (id,p) VALUES ('c','p')"),
        b("INSERT INTO P (id) VALUES ('p')"),
        b("SELECT p FROM C WHERE id='c'"),
    ];
    execute_sequential(&mut db, &tx, "seq-fk-0000001").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(execute_sequential(
        &mut db,
        &[
            b("UPDATE C SET p='missing' WHERE id='c'"),
            b("SELECT p FROM C WHERE id='c'")
        ],
        "seq-fk-0000002"
    )
    .is_err());
    assert_eq!(before, fs::read(&path).unwrap());
    db.verify().unwrap();
}
