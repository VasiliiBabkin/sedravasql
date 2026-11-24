#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, explain, SqlResult},
    table::{Cell, Row, Tables},
    NativeLog,
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn durable_definition_cached_lookup_invalidation_and_reopen() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let cmds = [
        "CREATE TABLE T (id TEXT PRIMARY KEY, cat TEXT, n INT)",
        "INSERT INTO T (id,cat,n) VALUES ('a','x',1)",
        "INSERT INTO T (id,cat,n) VALUES ('b','y',2)",
        "INSERT INTO T (id,cat,n) VALUES ('c','x',3)",
    ];
    for (i, q) in cmds.iter().enumerate() {
        execute(&mut db, q, Some(&format!("index-setup-{i:04}"))).unwrap();
    }
    let q = "SELECT id FROM T WHERE cat='x'";
    assert_eq!(
        explain(&mut db, q, &[]).unwrap().access,
        "bounded_table_scan"
    );
    let baseline = execute(&mut db, q, None).unwrap();
    let create = "CREATE INDEX by_cat ON T(cat)";
    execute(&mut db, create, Some("index-create-001")).unwrap();
    assert!(matches!(
        execute(&mut db, create, Some("index-create-001")).unwrap(),
        SqlResult::Write { replayed: true, .. }
    ));
    let plan = explain(&mut db, q, &[]).unwrap();
    assert_eq!(plan.access, "secondary_index_lookup");
    assert_eq!(plan.index_name.as_deref(), Some("by_cat"));
    assert_eq!(plan.index_cached, Some(false));
    assert_eq!(execute(&mut db, q, None).unwrap(), baseline);
    assert_eq!(explain(&mut db, q, &[]).unwrap().index_cached, Some(true));
    let before = fs::read(&path).unwrap();
    assert!(execute(
        &mut db,
        "CREATE INDEX by_cat ON T(n)",
        Some("index-create-002")
    )
    .is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(explain(&mut db, q, &[]).unwrap().index_cached, Some(true));
    execute(
        &mut db,
        "UPDATE T SET cat='x' WHERE id='b'",
        Some("index-update-001"),
    )
    .unwrap();
    assert_eq!(explain(&mut db, q, &[]).unwrap().index_cached, Some(false));
    assert_eq!(
        execute(&mut db, q, None).unwrap(),
        SqlResult::Rows {
            columns: vec!["id".into()],
            rows: ["a", "b", "c"].map(|s| vec![Cell::Text(s.into())]).to_vec()
        }
    );
    let mut t = Tables::new(&mut db);
    t.delete("index-delete-001", "T", "a", 1).unwrap();
    let row = Row::from([
        ("id".into(), Cell::Text("c".into())),
        ("cat".into(), Cell::Text("y".into())),
        ("n".into(), Cell::Int(3)),
    ]);
    t.put("index-table-0001", "T", 1, row).unwrap();
    let expected = SqlResult::Rows {
        columns: vec!["id".into()],
        rows: vec![vec![Cell::Text("b".into())]],
    };
    assert_eq!(execute(&mut db, q, None).unwrap(), expected);
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(explain(&mut db, q, &[]).unwrap().index_cached, Some(false));
    assert_eq!(execute(&mut db, q, None).unwrap(), expected);
}
#[test]
fn distinct_and_having() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let cmds = [
        "CREATE TABLE T (id TEXT PRIMARY KEY, cat TEXT, n INT)",
        "INSERT INTO T (id,cat,n) VALUES ('a','x',1)",
        "INSERT INTO T (id,cat,n) VALUES ('b','y',2)",
        "INSERT INTO T (id,cat,n) VALUES ('c','x',3)",
    ];
    for (i, q) in cmds.iter().enumerate() {
        execute(&mut db, q, Some(&format!("group-setup-{i:04}"))).unwrap();
    }
    assert_eq!(
        execute(
            &mut db,
            "SELECT DISTINCT cat FROM T ORDER BY cat DESC LIMIT 2",
            None
        )
        .unwrap(),
        SqlResult::Rows {
            columns: vec!["cat".into()],
            rows: vec![vec![Cell::Text("y".into())], vec![Cell::Text("x".into())]]
        }
    );
    assert_eq!(
        execute(
            &mut db,
            "SELECT cat,SUM(n) AS total FROM T GROUP BY cat HAVING total>=3 ORDER BY total",
            None
        )
        .unwrap(),
        SqlResult::AggregateRows {
            columns: vec!["cat".into(), "total".into()],
            rows: vec![vec![json!("x"), json!(4)]]
        }
    );
    assert_eq!(
        execute(
            &mut db,
            "SELECT SUM(n) AS total FROM T WHERE id='missing' HAVING total>0",
            None
        )
        .unwrap(),
        SqlResult::AggregateRows {
            columns: vec!["total".into()],
            rows: vec![]
        }
    );
    for q in [
        "SELECT DISTINCT cat FROM T ORDER BY n",
        "SELECT cat,COUNT(*) AS cnt FROM T GROUP BY cat HAVING cnt='x'",
        "SELECT COUNT(*) AS cnt FROM T HAVING missing>0",
        "SELECT DISTINCT COUNT(*) FROM T",
    ] {
        assert!(execute(&mut db, q, None).is_err(), "{q}");
    }
}
