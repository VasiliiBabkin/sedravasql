#![cfg(unix)]
use sedravasql_native_engine::{
    sql::{build_tree_projection, execute, execute_bound, SqlResult, TreeSqlReader},
    table::Cell,
    tree::Tree,
    NativeLog,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn private_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn populate(path: &Path, count: usize) -> NativeLog {
    let mut db = NativeLog::create(path).unwrap();
    execute(
        &mut db,
        "CREATE TABLE T (id TEXT PRIMARY KEY, cat TEXT, n INT)",
        Some("b28-create-0001"),
    )
    .unwrap();
    for i in 0..count {
        execute(
            &mut db,
            &format!(
                "INSERT INTO T (id,cat,n) VALUES ('k{i:04}','c{}',{})",
                i % 4,
                i % 11
            ),
            Some(&format!("b28-insert-{i:04}")),
        )
        .unwrap();
    }
    db
}

#[test]
fn sql_tree_matches_select_and_fails_closed_after_changes() {
    let d = private_dir();
    let log = d.path().join("source");
    let index = d.path().join("index");
    let mut db = populate(&log, 280);
    execute(
        &mut db,
        "DELETE FROM T WHERE id='k0004'",
        Some("b28-delete-0001"),
    )
    .unwrap();
    let queries = [
        "SELECT id,n FROM T WHERE id>='k0000' AND id<'k0280' ORDER BY id",
        "SELECT id,n FROM T WHERE id>='k0000' AND id<'k0280' ORDER BY id DESC LIMIT 20",
        "SELECT id,n FROM T WHERE id>='k0010' AND id<'k0210' AND cat='c2' ORDER BY n DESC LIMIT 20",
        "SELECT DISTINCT cat FROM T WHERE id>='k0000' AND id<'k0280' ORDER BY cat DESC",
        "SELECT * FROM T WHERE id>='k0004' AND id<'k0005'",
        "SELECT id FROM T WHERE id>='k0040' AND id<'k0040'",
        "SELECT id FROM T WHERE id>='k9999' AND id<'z'",
        "SELECT id FROM T WHERE id>='k0000' AND id<'k0280' AND id>'k0260' ORDER BY id",
    ];
    let expected: Vec<_> = queries
        .iter()
        .map(|q| execute(&mut db, q, None).unwrap())
        .collect();
    let q = "SELECT id,n FROM T WHERE id>=? AND id<? ORDER BY id";
    let params = [Cell::Text("k0250".into()), Cell::Text("k0280".into())];
    let bound = execute_bound(&mut db, q, &params, None).unwrap();
    let before = fs::read(&log).unwrap();
    let report = build_tree_projection(&mut db, "T", &index).unwrap();
    assert_eq!(report.rows, 280);
    assert!(report.height >= 3);
    let saved = fs::read(&index).unwrap();
    assert!(build_tree_projection(&mut db, "T", &index).is_err());
    assert_eq!(fs::read(&index).unwrap(), saved);
    {
        let mut reader = TreeSqlReader::open(&mut db, "T", &index).unwrap();
        for (q, result) in queries.iter().zip(expected) {
            assert_eq!(reader.query(q, &[]).unwrap(), result, "{q}");
        }
        assert_eq!(reader.query(q, &params).unwrap(), bound);
        for bad in [
            "SELECT id FROM T",
            "SELECT id FROM T WHERE id>='k0' AND id<='z'",
            "SELECT id FROM T WHERE id>='k0' AND id>='k1' AND id<'z'",
            "SELECT id FROM T WHERE id>='é' AND id<'z'",
            "SELECT missing FROM T WHERE id>='k0' AND id<'z'",
            "SELECT id FROM T WHERE id>='k0' AND id<'z' AND n='wrong'",
            "SELECT COUNT(*) FROM T WHERE id>='k0' AND id<'z'",
            "UPDATE T SET n=4 WHERE id='k0001'",
            "SELECT id FROM Other WHERE id>='k0' AND id<'z'",
        ] {
            assert!(reader.query(bad, &[]).is_err(), "{bad}");
        }
    }
    assert_eq!(fs::read(&log).unwrap(), before);
    assert_eq!(fs::read(&index).unwrap(), saved);
    execute(
        &mut db,
        "UPDATE T SET n=10 WHERE id='k0001'",
        Some("b28-update-0001"),
    )
    .unwrap();
    assert!(TreeSqlReader::open(&mut db, "T", &index).is_err());
    let newer = d.path().join("newer");
    build_tree_projection(&mut db, "T", &newer).unwrap();
    {
        let mut reader = TreeSqlReader::open(&mut db, "T", &newer).unwrap();
        assert_eq!(
            reader
                .query("SELECT n FROM T WHERE id>='k0001' AND id<'k0002'", &[])
                .unwrap(),
            SqlResult::Rows {
                columns: vec!["n".into()],
                rows: vec![vec![Cell::Int(10)]]
            }
        );
    }
    execute(
        &mut db,
        "INSERT INTO T (id,cat,n) VALUES ('new','c0',1)",
        Some("b28-new-key-0001"),
    )
    .unwrap();
    assert!(TreeSqlReader::open(&mut db, "T", &newer).is_err());
    db.verify().unwrap();
}

#[test]
fn projection_rejects_incomplete_wrong_revision_and_corruption() {
    let d = private_dir();
    let log = d.path().join("log");
    let mut db = populate(&log, 3);
    let path = d.path().join("bad");
    let mut tree = Tree::create(&path, 32).unwrap();
    tree.put("k0000", 0, Some("T:1".into())).unwrap();
    tree.put("k0001", 0, Some("T:1".into())).unwrap();
    drop(tree);
    assert!(TreeSqlReader::open(&mut db, "T", &path).is_err());
    let mut tree = Tree::open(&path).unwrap();
    tree.put("k0009", 0, Some("T:1".into())).unwrap();
    drop(tree);
    assert!(TreeSqlReader::open(&mut db, "T", &path).is_err());
    let valid = d.path().join("valid");
    build_tree_projection(&mut db, "T", &valid).unwrap();
    let mut tree = Tree::open(&valid).unwrap();
    tree.put("k0001", 1, Some("T:1".into())).unwrap();
    drop(tree);
    assert!(TreeSqlReader::open(&mut db, "T", &valid).is_err());
    let corrupt = d.path().join("corrupt");
    build_tree_projection(&mut db, "T", &corrupt).unwrap();
    let mut data = fs::read(&corrupt).unwrap();
    data[100] ^= 1;
    fs::write(&corrupt, data).unwrap();
    assert!(TreeSqlReader::open(&mut db, "T", &corrupt).is_err());
}

#[test]
fn preflight_capacity_and_empty_projection() {
    let d = private_dir();
    let mut db = populate(&d.path().join("log"), 0);
    let index = d.path().join("empty");
    build_tree_projection(&mut db, "T", &index).unwrap();
    let mut reader = TreeSqlReader::open(&mut db, "T", &index).unwrap();
    assert_eq!(
        reader
            .query("SELECT id FROM T WHERE id>='a' AND id<'z'", &[])
            .unwrap(),
        SqlResult::Rows {
            columns: vec!["id".into()],
            rows: vec![]
        }
    );
    drop(reader);
    for i in 0..513 {
        execute(
            &mut db,
            &format!("INSERT INTO T (id,cat,n) VALUES ('k{i:04}','c',0)"),
            Some(&format!("b28-cap-{i:04}")),
        )
        .unwrap();
    }
    let over = d.path().join("too-many");
    let before = db.sequence();
    assert!(build_tree_projection(&mut db, "T", &over).is_err());
    assert!(!over.exists());
    assert_eq!(db.sequence(), before);
}
