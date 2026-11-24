#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, execute_bound, explain, SqlResult},
    table::Cell,
    NativeLog,
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn ordered_range_matches_full_scan_and_preserves_writes() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY,cat TEXT,n INT)",
        Some("b28b-create-0001"),
    )
    .unwrap();
    execute(
        &mut db,
        "CREATE TABLE Other (id TEXT PRIMARY KEY,n INT)",
        Some("b28b-create-0002"),
    )
    .unwrap();
    execute(
        &mut db,
        "INSERT INTO Other (id,n) VALUES ('k0001',999)",
        Some("b28b-other-0001"),
    )
    .unwrap();
    for i in 0..280 {
        execute(
            &mut db,
            &format!(
                "INSERT INTO T (id,cat,n) VALUES ('k{i:04}','c{}',{i})",
                i % 4
            ),
            Some(&format!("b28b-insert-{i:04}")),
        )
        .unwrap();
    }
    execute(
        &mut db,
        "DELETE FROM T WHERE id='k0000'",
        Some("b28b-delete-0001"),
    )
    .unwrap();
    let q = "SELECT id,n FROM T WHERE id>=? AND id<? ORDER BY id DESC LIMIT 256";
    let scan = "SELECT id,n FROM T WHERE id>=? AND id>=? AND id<? ORDER BY id DESC LIMIT 256";
    for (lo, hi) in [
        ("k0000", "k0280"),
        ("k0250", "k0280"),
        ("k0000", "k0001"),
        ("k0090", "k0010"),
        ("k0030", "k0030"),
        ("a", "z"),
    ] {
        let p = [Cell::Text(lo.into()), Cell::Text(hi.into())];
        let full = [p[0].clone(), p[0].clone(), p[1].clone()];
        assert_eq!(explain(&mut db, q, &p).unwrap().access, "primary_key_range");
        assert_eq!(
            explain(&mut db, scan, &full).unwrap().access,
            "bounded_table_scan"
        );
        assert_eq!(
            execute_bound(&mut db, q, &p, None).unwrap(),
            execute_bound(&mut db, scan, &full, None).unwrap()
        );
    }
    let before = fs::read(&path).unwrap();
    let aggregate =
        "SELECT COUNT(*) AS cnt,SUM(n) AS total FROM T WHERE id>='k0000' AND id<'k0280'";
    assert_eq!(
        explain(&mut db, aggregate, &[]).unwrap().access,
        "primary_key_range"
    );
    assert_eq!(
        execute(&mut db, aggregate, None).unwrap(),
        SqlResult::AggregateRows {
            columns: vec!["cnt".into(), "total".into()],
            rows: vec![vec![json!(279), json!(39060)]]
        }
    );
    let distinct =
        "SELECT DISTINCT cat FROM T WHERE id>='k0001' AND id<'k0280' AND n>=5 ORDER BY cat";
    assert_eq!(
        execute(&mut db, distinct, None).unwrap(),
        SqlResult::Rows {
            columns: vec!["cat".into()],
            rows: (0..4).map(|i| vec![Cell::Text(format!("c{i}"))]).collect()
        }
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        explain(&mut db, "SELECT id FROM T WHERE id>='é' AND id<'z'", &[])
            .unwrap()
            .access,
        "bounded_table_scan"
    );
    assert_eq!(
        explain(
            &mut db,
            "SELECT id FROM T WHERE id>='k0000' AND id<'k0280' AND id='k0001'",
            &[]
        )
        .unwrap()
        .access,
        "primary_key_lookup"
    );
    execute(
        &mut db,
        "CREATE INDEX by_cat ON T(cat)",
        Some("b28b-index-0001"),
    )
    .unwrap();
    assert_eq!(
        explain(
            &mut db,
            "SELECT id FROM T WHERE id>='k0000' AND id<'k0280' AND cat='c0'",
            &[]
        )
        .unwrap()
        .access,
        "secondary_index_lookup"
    );
    assert!(execute(
        &mut db,
        "SELECT n FROM T WHERE id>='k0000' AND id<'k0280' AND n='bad'",
        None
    )
    .is_err());
    execute(
        &mut db,
        "UPDATE T SET n=999 WHERE id>='k0001' AND id<'k0002'",
        Some("b28b-update-0001"),
    )
    .unwrap();
    assert_eq!(
        execute(
            &mut db,
            "SELECT n FROM T WHERE id>='k0001' AND id<'k0002'",
            None
        )
        .unwrap(),
        SqlResult::Rows {
            columns: vec!["n".into()],
            rows: vec![vec![Cell::Int(999)]]
        }
    );
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(
        execute(
            &mut db,
            "SELECT n FROM T WHERE id>='k0001' AND id<'k0002'",
            None
        )
        .unwrap(),
        SqlResult::Rows {
            columns: vec!["n".into()],
            rows: vec![vec![Cell::Int(999)]]
        }
    );
    db.verify().unwrap();
}
