#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{execute, execute_bound, explain, SqlResult},
    table::Cell,
    NativeLog,
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn joins_nulls_multiplicity_filter_sort_and_budget() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("db");
    let mut db = NativeLog::create(&path).unwrap();
    let commands = [
        "CREATE TABLE L (id TEXT PRIMARY KEY, n INT)",
        "CREATE TABLE R (id TEXT PRIMARY KEY, n INT, note TEXT)",
        "INSERT INTO L (id,n) VALUES ('a',1)",
        "INSERT INTO L (id,n) VALUES ('b',2)",
        "INSERT INTO L (id,n) VALUES ('c',3)",
        "INSERT INTO R (id,n,note) VALUES ('x',1,'one')",
        "INSERT INTO R (id,n,note) VALUES ('y',1,'two')",
        "INSERT INTO R (id,n,note) VALUES ('z',3,'three')",
    ];
    for (i, q) in commands.iter().enumerate() {
        execute(&mut db, q, Some(&format!("join-setup-{i:04}"))).unwrap();
    }
    let before = fs::read(&path).unwrap();
    let q = "SELECT L.id,R.note FROM L LEFT JOIN R ON L.n=R.n ORDER BY L.id";
    assert_eq!(
        execute(&mut db, q, None).unwrap(),
        SqlResult::JoinRows {
            columns: vec!["L.id".into(), "R.note".into()],
            rows: vec![
                vec![json!("a"), json!("one")],
                vec![json!("a"), json!("two")],
                vec![json!("b"), json!(null)],
                vec![json!("c"), json!("three")]
            ]
        }
    );
    assert_eq!(execute_bound(&mut db,"SELECT L.id,R.id FROM L INNER JOIN R ON R.n=L.n WHERE R.n>=? ORDER BY R.id DESC LIMIT 2",&[Cell::Int(1)],None).unwrap(),SqlResult::JoinRows{columns:vec!["L.id".into(),"R.id".into()],rows:vec![vec![json!("c"),json!("z")],vec![json!("a"),json!("y")]]});
    assert_eq!(
        explain(&mut db, q, &[]).unwrap().access,
        "query_local_right_index"
    );
    for q in [
        "SELECT id FROM L JOIN R ON L.n=R.n",
        "SELECT L.id FROM L JOIN R ON L.id=R.n",
        "SELECT L.id FROM L JOIN R ON L.n=L.n",
        "SELECT L.id FROM L JOIN R ON L.n=R.n WHERE R.n='x'",
        "SELECT L.id FROM L JOIN R ON L.n=R.n ORDER BY R.absent",
        "SELECT L.id FROM L JOIN L ON L.n=L.n",
        "SELECT COUNT(*) FROM L JOIN R ON L.n=R.n",
    ] {
        assert!(execute(&mut db, q, None).is_err(), "{q}");
    }
    assert_eq!(before, fs::read(&path).unwrap());
}
