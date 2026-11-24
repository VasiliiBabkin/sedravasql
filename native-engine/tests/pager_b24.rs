#![cfg(unix)]
use sedravasql_native_engine::pager::{Change, Pager};
use std::{fs, os::unix::fs::PermissionsExt};
fn change(key: &str, rev: u64, value: Option<&str>) -> Change {
    Change {
        namespace: "ns".into(),
        key: key.into(),
        expected_revision: rev,
        value: value.map(str::to_owned),
    }
}
#[test]
fn atomic_pages_cache_and_reopen() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("pager");
    let mut db = Pager::create(&path, 32).unwrap();
    let before = fs::read(&path).unwrap();
    let result = db
        .commit(&[
            change("a", 0, Some("first")),
            change("b", 0, Some("second")),
        ])
        .unwrap();
    assert_eq!(result.generation, 1);
    assert_eq!(result.changed_pages, 3);
    let after = fs::read(&path).unwrap();
    assert_eq!(
        before
            .chunks(4096)
            .zip(after.chunks(4096))
            .filter(|(a, b)| a != b)
            .count(),
        3
    );
    assert_eq!(
        db.get("ns", "a").unwrap().unwrap().value.as_deref(),
        Some("first")
    );
    let hits = db.report().cache_hits;
    db.get("ns", "a").unwrap();
    assert!(db.report().cache_hits > hits);
    assert!(db
        .commit(&[
            change("a", 1, Some("new")),
            change("b", 0, Some("conflict"))
        ])
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), after);
    assert!(db
        .commit(&[change("huge", 0, Some(&"a".repeat(5000)))])
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), after);
    db.commit(&[change("a", 1, None), change("b", 1, Some("updated"))])
        .unwrap();
    drop(db);
    let mut db = Pager::open(&path).unwrap();
    assert_eq!(db.report().generation, 2);
    let a = db.get("ns", "a").unwrap().unwrap();
    assert_eq!(a.revision, 2);
    assert!(a.value.is_none());
    assert_eq!(
        db.get("ns", "b").unwrap().unwrap().value.as_deref(),
        Some("updated")
    );
    assert!(db.get("ns", "missing").unwrap().is_none());
    assert!(Pager::open(&path).is_err());
}
#[test]
fn full_table_and_corruption() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("pager");
    let mut db = Pager::create(&path, 8).unwrap();
    let ops = (0..8)
        .map(|i| change(&format!("k{i}"), 0, Some("v")))
        .collect::<Vec<_>>();
    db.commit(&ops).unwrap();
    for i in 0..8 {
        assert!(db.get("ns", &format!("k{i}")).unwrap().is_some());
    }
    assert!(db.get("ns", "absent").unwrap().is_none());
    let before = fs::read(&path).unwrap();
    assert!(db.commit(&[change("overflow", 0, Some("v"))]).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(db);
    let mut bytes = before;
    bytes[4096 * 2 + 18] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(Pager::open(&path).is_err());
}
