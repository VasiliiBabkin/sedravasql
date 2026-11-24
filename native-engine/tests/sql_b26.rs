#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, SqlResult},
    table::{Cell, Tables},
    NativeLog,
};
use serde_json::json;

#[test]
fn multipage_scan_index_and_reopen_agree() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("rows.log");
    let mut db = NativeLog::create(&path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY, cat TEXT, n INT)",
        Some("b26-schema-0001"),
    )
    .unwrap();
    for i in 0..260 {
        execute(
            &mut db,
            &format!(
                "INSERT INTO T (id,cat,n) VALUES ('k{i:04}','c{}',{i})",
                i % 4
            ),
            Some(&format!("b26-insert-{i:04}")),
        )
        .unwrap();
    }
    execute(
        &mut db,
        "DELETE FROM T WHERE id='k0000'",
        Some("b26-delete-0001"),
    )
    .unwrap();
    let q = "SELECT id,n FROM T WHERE cat='c0' ORDER BY id";
    let agg = "SELECT COUNT(*) AS n,SUM(n) AS total FROM T WHERE cat='c0'";
    let scan = execute(&mut db, q, None).unwrap();
    let aggregate = execute(&mut db, agg, None).unwrap();
    assert_eq!(
        aggregate,
        SqlResult::AggregateRows {
            columns: vec!["n".into(), "total".into()],
            rows: vec![vec![json!(64), json!(8320)]]
        }
    );
    assert!(
        matches!(&scan,SqlResult::Rows{rows,..} if rows.len()==64 && rows[0][0]==Cell::Text("k0004".into()))
    );
    execute(
        &mut db,
        "CREATE INDEX by_cat ON T(cat)",
        Some("b26-index-0001"),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    for _ in 0..3 {
        assert_eq!(execute(&mut db, q, None).unwrap(), scan);
        assert_eq!(execute(&mut db, agg, None).unwrap(), aggregate);
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(Tables::new(&mut db).index_cached("T", "cat"));
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert!(!Tables::new(&mut db).index_cached("T", "cat"));
    assert_eq!(execute(&mut db, q, None).unwrap(), scan);
    assert_eq!(execute(&mut db, agg, None).unwrap(), aggregate);
    execute(
        &mut db,
        "UPDATE T SET cat='c0' WHERE id='k0001'",
        Some("b26-update-0001"),
    )
    .unwrap();
    assert!(!Tables::new(&mut db).index_cached("T", "cat"));
    assert_eq!(
        execute(&mut db, agg, None).unwrap(),
        SqlResult::AggregateRows {
            columns: vec!["n".into(), "total".into()],
            rows: vec![vec![json!(65), json!(8321)]]
        }
    );
    db.verify().unwrap();
}
