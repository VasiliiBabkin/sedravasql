#![cfg(unix)]
use sedravasql_native_engine::{
    cow_store::CowStore,
    sql::{execute, execute_sequential, BoundStatement},
    table::{Cell, Tables},
    NativeLog, SqlStore,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn step(a: &mut dyn SqlStore, b: &mut dyn SqlStore, q: &str, key: Option<&str>) {
    let x = execute(a, q, key);
    let y = execute(b, q, key);
    match (x, y) {
        (Ok(x), Ok(y)) => assert_eq!(x, y, "{q}"),
        (Err(x), Err(y)) => assert_eq!(
            std::mem::discriminant(&x),
            std::mem::discriminant(&y),
            "{q}: {x} / {y}"
        ),
        (x, y) => panic!("parity {q}: {x:?} / {y:?}"),
    }
}
#[test]
fn sql_parity_and_historical_replay() {
    let d = dir();
    let p = d.path().join("cow");
    let q = d.path().join("log");
    let mut a = CowStore::create(&p).unwrap();
    let mut b = NativeLog::create(&q).unwrap();
    for (i, sql) in [
        "CREATE TABLE Products (id TEXT PRIMARY KEY,category TEXT,price INT)",
        "INSERT INTO Products (id,category,price) VALUES ('p1','food',10)",
        "INSERT INTO Products (id,category,price) VALUES ('p2','food',20)",
        "INSERT INTO Products (id,category,price) VALUES ('p3','other',40)",
        "CREATE INDEX category ON Products (category)",
        "CREATE TABLE Orders (id TEXT PRIMARY KEY,product TEXT,amount INT)",
        "INSERT INTO Orders (id,product,amount) VALUES ('o1','p1',2)",
    ]
    .iter()
    .enumerate()
    {
        let key = format!("sql-b35b-{i:05}");
        let x = execute(&mut a, sql, Some(&key)).unwrap();
        let y = execute(&mut b, sql, Some(&key)).unwrap();
        assert_eq!(x, y);
    }
    for sql in [
        "SELECT id,price FROM Products WHERE id >= 'p1' AND id < 'p3' ORDER BY price DESC",
        "SELECT id FROM Products WHERE category='food' ORDER BY id",
        "SELECT category,COUNT(*) AS cnt,SUM(price) AS total FROM Products GROUP BY category",
        "SELECT Products.id,Orders.amount FROM Products JOIN Orders ON Products.id=Orders.product",
    ] {
        assert_eq!(
            execute(&mut a, sql, None).unwrap(),
            execute(&mut b, sql, None).unwrap(),
            "{sql}"
        );
    }
    assert!(Tables::new(&mut a).index_cached("Products", "category"));
    let tx = [
        BoundStatement {
            sql: "SELECT price FROM Products WHERE id='p1'".into(),
            params: vec![],
        },
        BoundStatement {
            sql: "UPDATE Products SET price=11 WHERE id='p1'".into(),
            params: vec![],
        },
        BoundStatement {
            sql: "SELECT price FROM Products WHERE id='p1'".into(),
            params: vec![],
        },
    ];
    let key = "sql-seq-b35b-001";
    let x = execute_sequential(&mut a, &tx, key).unwrap();
    let y = execute_sequential(&mut b, &tx, key).unwrap();
    assert_eq!(x, y);
    assert!(!Tables::new(&mut a).index_cached("Products", "category"));
    drop(a);
    let mut a = CowStore::open(&p).unwrap();
    assert_eq!(
        execute_sequential(&mut a, &tx, key).unwrap(),
        execute_sequential(&mut b, &tx, key).unwrap()
    );
    assert_eq!(
        execute(&mut a, "SELECT price FROM Products WHERE id='p1'", None).unwrap(),
        execute(&mut b, "SELECT price FROM Products WHERE id='p1'", None).unwrap()
    );
    step(
        &mut a,
        &mut b,
        "UPDATE Products SET price=99 WHERE id='p1'",
        Some(key),
    );
    let rows = Tables::new(&mut a)
        .get("Products", "p1")
        .unwrap()
        .unwrap()
        .values
        .unwrap();
    assert_eq!(rows["price"], Cell::Int(11));
    a.verify().unwrap();
}
#[test]
fn explicit_cli_backend_roundtrip() {
    use std::process::Command;
    let d = dir();
    let p = d.path().join("db");
    let p = p.to_str().unwrap();
    let run = |args: &[&str]| {
        let r = Command::new(env!("CARGO_BIN_EXE_sedravasql-native-preview"))
            .args(args)
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        serde_json::from_slice::<serde_json::Value>(&r.stdout).unwrap()
    };
    run(&["cow-init", p]);
    run(&[
        "cow-sql",
        p,
        "CREATE TABLE T (id TEXT PRIMARY KEY,n INT)",
        "cow-create-00001",
    ]);
    run(&[
        "cow-sql",
        p,
        "INSERT INTO T (id,n) VALUES ('a',1)",
        "cow-insert-00001",
    ]);
    assert_eq!(
        run(&["cow-sql", p, "SELECT n FROM T WHERE id='a'"])["rows"],
        serde_json::json!([[1]])
    );
    assert_eq!(
        run(&[
            "cow-sql",
            p,
            "INSERT INTO T (id,n) VALUES ('a',1)",
            "cow-insert-00001"
        ])["replayed"],
        true
    );
    run(&["cow-verify", p]);
    let r = Command::new(env!("CARGO_BIN_EXE_sedravasql-native-preview"))
        .args(["sql", p, "SELECT n FROM T"])
        .output()
        .unwrap();
    assert!(!r.status.success());
}
