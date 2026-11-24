#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, explain, SqlResult},
    table::Cell,
    NativeLog,
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn aggregates_groups_and_plan_are_readonly() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY, category TEXT, n INT, ok BOOL)",
        Some("create-table-001"),
    )
    .unwrap();
    for (i, cat, n, ok) in [
        (0, "a", 2147483647, true),
        (1, "a", 2147483647, false),
        (2, "b", -5, true),
        (3, "b", 7, true),
    ] {
        execute(
            &mut db,
            &format!("INSERT INTO T (id,category,n,ok) VALUES ('r{i}','{cat}',{n},{ok})"),
            Some(&format!("insert-table-{i:04}")),
        )
        .unwrap();
    }
    let before = fs::read(&path).unwrap();
    let result=execute(&mut db,"SELECT category,COUNT(*) AS cnt,SUM(n) AS total,MIN(n) AS lo,MAX(n) AS hi,AVG(n) AS mean FROM T GROUP BY category ORDER BY total DESC",None).unwrap();
    assert_eq!(
        result,
        SqlResult::AggregateRows {
            columns: ["category", "cnt", "total", "lo", "hi", "mean"]
                .map(String::from)
                .to_vec(),
            rows: vec![
                vec![
                    json!("a"),
                    json!(2),
                    json!(4294967294_i64),
                    json!(2147483647),
                    json!(2147483647),
                    json!(2147483647.0)
                ],
                vec![
                    json!("b"),
                    json!(2),
                    json!(2),
                    json!(-5),
                    json!(7),
                    json!(1.0)
                ]
            ]
        }
    );
    assert_eq!(
        execute(
            &mut db,
            "SELECT COUNT(*) AS cnt,SUM(n),AVG(n),MIN(category),MAX(n) FROM T WHERE id='missing'",
            None
        )
        .unwrap(),
        SqlResult::AggregateRows {
            columns: ["cnt", "SUM(n)", "AVG(n)", "MIN(category)", "MAX(n)"]
                .map(String::from)
                .to_vec(),
            rows: vec![vec![
                json!(0),
                json!(null),
                json!(null),
                json!(null),
                json!(null)
            ]]
        }
    );
    assert_eq!(
        execute(
            &mut db,
            "SELECT category,COUNT(*) FROM T WHERE id='missing' GROUP BY category",
            None
        )
        .unwrap(),
        SqlResult::AggregateRows {
            columns: vec!["category".into(), "COUNT(*)".into()],
            rows: vec![]
        }
    );
    for sql in [
        "SELECT category,COUNT(*) FROM T",
        "SELECT SUM(category) FROM T",
        "SELECT COUNT(absent) FROM T",
        "SELECT COUNT(*) AS x,SUM(n) AS x FROM T",
        "SELECT * FROM T GROUP BY category",
        "SELECT COUNT(*) FROM T GROUP BY absent",
        "SELECT COUNT(*) FROM T ORDER BY absent",
        "SELECT AVG(*) FROM T",
    ] {
        assert!(execute(&mut db, sql, None).is_err(), "{sql}");
    }
    let p = explain(
        &mut db,
        "SELECT COUNT(*) FROM T WHERE id=?",
        &[Cell::Text("r1".into())],
    )
    .unwrap();
    assert_eq!(p.access, "primary_key_lookup");
    assert!(p.aggregate);
    let p = explain(
        &mut db,
        "SELECT category,SUM(n) AS s FROM T GROUP BY category ORDER BY s DESC LIMIT 1",
        &[],
    )
    .unwrap();
    assert_eq!(p.access, "bounded_table_scan");
    assert_eq!(p.group_by.as_deref(), Some("category"));
    assert!(p.sort);
    assert_eq!(p.limit, 1);
    assert_eq!(fs::read(&path).unwrap(), before);
}
