#![cfg(unix)]
use sedravasql_native_engine::{
    table::{Cell, Column, ColumnType, Row, TableSchema, Tables},
    NativeLog,
};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};
fn schema(name: &str) -> TableSchema {
    TableSchema {
        name: name.into(),
        primary_key: "id".into(),
        columns: vec![
            Column {
                name: "id".into(),
                kind: ColumnType::Text,
            },
            Column {
                name: "amount".into(),
                kind: ColumnType::Int,
            },
            Column {
                name: "paid".into(),
                kind: ColumnType::Bool,
            },
        ],
    }
}
fn row(id: &str, amount: i32) -> Row {
    BTreeMap::from([
        ("id".into(), Cell::Text(id.into())),
        ("amount".into(), Cell::Int(amount)),
        ("paid".into(), Cell::Bool(false)),
    ])
}
#[test]
fn typed_schema_rows_cas_tombstones_and_scan() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("table.log");
    let mut db = NativeLog::create(&path).unwrap();
    {
        let mut t = Tables::new(&mut db);
        let schema = schema("Orders");
        assert!(t.schema("Orders").unwrap().is_none());
        t.create("table-schema-0001", &schema).unwrap();
        assert!(t.create("table-schema-0001", &schema).unwrap().replayed);
        assert!(t.create("table-schema-0002", &schema).is_err());
        let mut wrong = row("a", 10);
        wrong.insert("amount".into(), Cell::Text("10".into()));
        assert!(t.put("table-row-000001", "Orders", 0, wrong).is_err());
        t.put("table-row-000001", "Orders", 0, row("b", 20))
            .unwrap();
        t.put("table-row-000002", "Orders", 0, row("a", 10))
            .unwrap();
        assert!(t
            .put("table-row-000003", "Orders", 0, row("a", 99))
            .is_err());
        assert_eq!(t.scan("Orders", None, 1).unwrap()[0].key, "a");
        assert_eq!(t.scan("Orders", Some("a"), 10).unwrap()[0].key, "b");
        t.delete("table-delete-001", "Orders", "a", 1).unwrap();
        assert_eq!(t.get("Orders", "a").unwrap().unwrap().values, None);
        assert!(t
            .put("table-row-000003", "Orders", 0, row("a", 99))
            .is_err());
        assert!(t.scan("Orders", None, 257).is_err());
    }
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    assert_eq!(db.sequence(), 4);
    let mut t = Tables::new(&mut db);
    assert_eq!(t.schema("Orders").unwrap().unwrap(), schema("Orders"));
    assert!(
        t.put("table-row-000002", "Orders", 0, row("a", 10))
            .unwrap()
            .replayed
    );
    t.put("table-row-000003", "Orders", 2, row("a", 99))
        .unwrap();
    assert_eq!(
        t.get("Orders", "a").unwrap().unwrap().values,
        Some(row("a", 99))
    );
    assert_eq!(t.get("Orders", "missing").unwrap(), None);
}

#[test]
fn atomic_cross_table_batch_no_partial_writes() {
    use sedravasql_native_engine::table::TableMutation;
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("batch.log");
    let mut db = NativeLog::create(&path).unwrap();
    let mut t = Tables::new(&mut db);
    t.create("table-create-0001", &schema("Orders")).unwrap();
    t.create("table-create-0002", &schema("Payments")).unwrap();
    let ops = [
        TableMutation::Put {
            table: "Orders".into(),
            expected_revision: 0,
            row: row("a", 10),
        },
        TableMutation::Put {
            table: "Payments".into(),
            expected_revision: 0,
            row: row("a", 10),
        },
    ];
    let receipt = t.batch("table-batch-00001", &ops).unwrap();
    assert_eq!(receipt.sequence, 3);
    let before = fs::read(&path).unwrap();
    let bad = [
        TableMutation::Put {
            table: "Orders".into(),
            expected_revision: 1,
            row: row("a", 20),
        },
        TableMutation::Delete {
            table: "Payments".into(),
            key: "a".into(),
            expected_revision: 0,
        },
    ];
    assert!(t.batch("table-batch-00002", &bad).is_err());
    assert_eq!(
        t.get("Orders", "a").unwrap().unwrap().values,
        Some(row("a", 10))
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let good = [
        bad[0].clone(),
        TableMutation::Delete {
            table: "Payments".into(),
            key: "a".into(),
            expected_revision: 1,
        },
    ];
    t.batch("table-batch-00002", &good).unwrap();
    drop(db);
    let mut db = NativeLog::open(&path).unwrap();
    let mut t = Tables::new(&mut db);
    assert!(t.batch("table-batch-00001", &ops).unwrap().replayed);
    assert!(t.batch("table-batch-00002", &good).unwrap().replayed);
    assert_eq!(t.get("Payments", "a").unwrap().unwrap().values, None);
    assert_eq!(
        t.get("Orders", "a").unwrap().unwrap().values,
        Some(row("a", 20))
    );
}
