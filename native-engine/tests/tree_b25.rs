#![cfg(unix)]
use sedravasql_native_engine::tree::Tree;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn splits_ranges_updates_and_reopen() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("tree");
    let mut t = Tree::create(&path, 512).unwrap();
    for i in 0..120 {
        let n = (i * 37) % 120;
        let k = format!("k{n:03}");
        t.put(&k, 0, Some(format!("value-{n}"))).unwrap();
    }
    assert_eq!(t.report().rows, 120);
    assert!(t.report().height >= 3);
    for i in 0..120 {
        let k = format!("k{i:03}");
        assert_eq!(
            t.get(&k).unwrap().unwrap().value,
            Some(format!("value-{i}"))
        );
    }
    let rows = t.range("k027", "k083", 256).unwrap();
    assert_eq!(rows.len(), 56);
    assert_eq!(rows[0].key, "k027");
    assert_eq!(rows[55].key, "k082");
    let before = fs::read(&path).unwrap();
    assert!(t.put("k037", 0, Some("conflict".into())).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let r = t.put("k037", 1, Some("updated".into())).unwrap();
    assert_eq!(r.changed_pages, 2);
    t.put("k038", 1, None).unwrap();
    drop(t);
    let mut t = Tree::open(&path).unwrap();
    assert_eq!(t.report().rows, 120);
    assert_eq!(
        t.get("k037").unwrap().unwrap().value.as_deref(),
        Some("updated")
    );
    assert!(t.get("k038").unwrap().unwrap().value.is_none());
    let r = t.range("k036", "k040", 3).unwrap();
    assert_eq!(
        r.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        ["k036", "k037", "k038"]
    );
    assert!(t.get("z").unwrap().is_none());
}
#[test]
fn capacity_failure_does_not_split_partially() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("tree");
    let mut t = Tree::create(&path, 16).unwrap();
    for i in 0..100 {
        let before = fs::read(&path).unwrap();
        if t.put(&format!("k{i:03}"), 0, Some("v".into())).is_err() {
            assert_eq!(before, fs::read(&path).unwrap());
            drop(t);
            let t = Tree::open(&path).unwrap();
            assert_eq!(t.report().rows, i);
            return;
        }
    }
    panic!("expected capacity");
}
