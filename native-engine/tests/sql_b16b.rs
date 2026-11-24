#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, SqlResult},
    table::Cell,
    Error, NativeLog,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn db() -> (tempfile::TempDir, NativeLog) {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let db = NativeLog::create(&d.path().join("db")).unwrap();
    (d, db)
}
fn write(db: &mut NativeLog, sql: &str, key: &str) -> SqlResult {
    execute(db, sql, Some(key)).unwrap()
}
fn select(db: &mut NativeLog, sql: &str) -> SqlResult {
    execute(db, sql, None).unwrap()
}
#[test]
fn sql_crud_order_filters_and_replay() {
    let (d, mut db) = db();
    write(
        &mut db,
        "CREATE TABLE Orders (id TEXT PRIMARY KEY, amount INT, paid BOOL, note TEXT)",
        "create-orders-01",
    );
    for (i, key) in ["b", "a", "c"].iter().enumerate() {
        write(
            &mut db,
            &format!(
                "INSERT INTO Orders (id,amount,paid,note) VALUES ('{key}',{},false,'it''s ok')",
                i + 1
            ),
            &format!("insert-order-{i:04}"),
        );
    }
    let q="SELECT id,amount FROM Orders WHERE amount >= 1 AND paid = false ORDER BY amount DESC LIMIT 2";
    assert_eq!(
        select(&mut db, q),
        SqlResult::Rows {
            columns: vec!["id".into(), "amount".into()],
            rows: vec![
                vec![Cell::Text("c".into()), Cell::Int(3)],
                vec![Cell::Text("a".into()), Cell::Int(2)]
            ]
        }
    );
    let update = "UPDATE Orders SET paid = true, amount = -2 WHERE amount >= 2";
    let receipt = write(&mut db, update, "update-order-001");
    assert_eq!(
        receipt,
        SqlResult::Write {
            affected: 2,
            sequence: 5,
            replayed: false
        }
    );
    let del = "DELETE FROM Orders WHERE id = 'a'";
    write(&mut db, del, "delete-order-001");
    let before = fs::read(d.path().join("db")).unwrap();
    assert_eq!(
        write(&mut db, update, "update-order-001"),
        SqlResult::Write {
            affected: 2,
            sequence: 5,
            replayed: true
        }
    );
    assert!(matches!(
        execute(
            &mut db,
            "UPDATE Orders SET paid = false WHERE id = 'b'",
            Some("update-order-001")
        ),
        Err(Error::Conflict(_))
    ));
    assert_eq!(fs::read(d.path().join("db")).unwrap(), before);
    let empty = write(
        &mut db,
        "DELETE FROM Orders WHERE id = 'not-found'",
        "delete-empty-001",
    );
    assert_eq!(
        empty,
        SqlResult::Write {
            affected: 0,
            sequence: 7,
            replayed: false
        }
    );
    drop(db);
    let mut db = NativeLog::open(&d.path().join("db")).unwrap();
    assert_eq!(
        write(&mut db, del, "delete-order-001"),
        SqlResult::Write {
            affected: 1,
            sequence: 6,
            replayed: true
        }
    );
    assert_eq!(
        select(&mut db, "SELECT id FROM Orders ORDER BY amount ASC"),
        SqlResult::Rows {
            columns: vec!["id".into()],
            rows: vec![vec![Cell::Text("c".into())], vec![Cell::Text("b".into())]]
        }
    );
}
#[test]
fn sql_errors_leave_bytes_unchanged_and_limit_atomicity() {
    let (d, mut db) = db();
    write(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY, n INT, ok BOOL)",
        "create-table-001",
    );
    for i in 0..16 {
        write(
            &mut db,
            &format!("INSERT INTO T (id,n,ok) VALUES ('r{i:02}',{i},true)"),
            &format!("insert-table-{i:04}"),
        );
    }
    let path = d.path().join("db");
    let before = fs::read(&path).unwrap();
    for q in [
        "UPDATE T SET n=0 WHERE ok=true",
        "DELETE FROM T WHERE n>=0",
        "UPDATE T SET n=0",
        "DELETE FROM T",
        "UPDATE T SET id='z' WHERE id='r00'",
        "UPDATE T SET n='bad' WHERE n=999",
        "INSERT INTO T (id,n,ok) VALUES ('r00',3,false)",
        "INSERT INTO T (id,n,ok) VALUES ('x',2147483648,false)",
        "CREATE TABLE T (id TEXT PRIMARY KEY)",
        "UPDATE T SET n=2 WHERE id='r01'; DELETE FROM T WHERE id='r02'",
        "UPDATE T SET n=2 WHERE n='1'",
        "UPDATE T SET missing=2 WHERE id='absent'",
        "INSERT INTO T (id,id,ok) VALUES ('x','x',true)",
    ] {
        assert!(
            execute(&mut db, q, Some("invalid-write-01")).is_err(),
            "{q}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    for q in [
        "SELECT missing FROM T WHERE id='none'",
        "SELECT n FROM T WHERE ok > false",
        "SELECT * FROM T LIMIT 257",
        "SELECT * FROM T WHERE n=0 OR n=1",
        "SELECT * FROM T WHERE id='unterminated",
        "SELECT * FROM T WHERE n=0; SELECT * FROM T",
        "SELECT * FROM T ORDER BY absent",
    ] {
        assert!(execute(&mut db, q, None).is_err(), "{q}");
    }
    assert!(execute(&mut db, "SELECT * FROM T", Some("select-key-0001")).is_err());
    assert!(execute(&mut db, "DELETE FROM T WHERE id='r00'", None).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        write(&mut db, "UPDATE T SET n=100 WHERE n<15", "update-table-001"),
        SqlResult::Write {
            affected: 15,
            sequence: 18,
            replayed: false
        }
    );
    db.verify().unwrap();
}

#[test]
fn positional_bindings_are_scalar_only_and_fingerprinted() {
    use sedravasql_native_engine::sql::{check, execute_bound, SqlKind};
    let (d, mut db) = db();
    write(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY, note TEXT, n INT)",
        "create-bound-001",
    );
    let sql = "INSERT INTO T (id,note,n) VALUES (?,?,?)";
    let payload = "x'); DELETE FROM T WHERE id='a'; --";
    let params = [
        Cell::Text("a".into()),
        Cell::Text(payload.into()),
        Cell::Int(5),
    ];
    assert_eq!(check(sql, &params).unwrap(), SqlKind::Write);
    let first = execute_bound(&mut db, sql, &params, Some("bound-insert-001")).unwrap();
    assert_eq!(
        first,
        SqlResult::Write {
            affected: 1,
            sequence: 2,
            replayed: false
        }
    );
    let q = "SELECT note,n FROM T WHERE id=?";
    assert_eq!(
        execute_bound(&mut db, q, &[Cell::Text("a".into())], None).unwrap(),
        SqlResult::Rows {
            columns: vec!["note".into(), "n".into()],
            rows: vec![vec![Cell::Text(payload.into()), Cell::Int(5)]]
        }
    );
    let before = fs::read(d.path().join("db")).unwrap();
    let mut changed = params.clone();
    changed[2] = Cell::Int(6);
    assert!(matches!(
        execute_bound(&mut db, sql, &changed, Some("bound-insert-001")),
        Err(Error::Conflict(_))
    ));
    assert!(execute_bound(&mut db, sql, &params[..2], Some("bound-insert-002")).is_err());
    assert!(execute_bound(&mut db, "SELECT * FROM ?", &[Cell::Text("T".into())], None).is_err());
    assert!(execute_bound(&mut db, "SELECT * FROM T LIMIT ?", &[Cell::Int(1)], None).is_err());
    assert!(execute_bound(&mut db, "SELECT * FROM T", &[Cell::Bool(true)], None).is_err());
    assert!(execute_bound(
        &mut db,
        "SELECT * FROM T WHERE id=?",
        &[Cell::Text("x\n".into())],
        None
    )
    .is_err());
    assert_eq!(fs::read(d.path().join("db")).unwrap(), before);
    assert_eq!(
        execute_bound(&mut db, sql, &params, Some("bound-insert-001")).unwrap(),
        SqlResult::Write {
            affected: 1,
            sequence: 2,
            replayed: true
        }
    );
}
