#![cfg(unix)]
use sedravasql_native_engine::cow_tree::{CowTree, Mutation, Record};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn m(key: &str, rev: u64, v: Option<&str>) -> Mutation {
    Mutation {
        key: key.into(),
        expected_revision: rev,
        value: v.map(str::to_owned),
    }
}
fn side(p: &Path, s: &str) -> std::path::PathBuf {
    let mut n = p.file_name().unwrap().to_os_string();
    n.push(s);
    p.with_file_name(n)
}
fn bytes(p: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(p).unwrap(), fs::read(side(p, ".head")).unwrap())
}
#[test]
fn batches_splits_ranges_cache_and_reopen() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    let mut expected = BTreeMap::new();
    for base in (0..2048).step_by(16) {
        let ops = (base..base + 16)
            .map(|i| {
                let n = i * 7919 % 2048;
                let k = format!("k{n:04}");
                expected.insert(
                    k.clone(),
                    Record {
                        key: k.clone(),
                        revision: 1,
                        value: Some(format!("v{n}")),
                    },
                );
                m(&k, 0, Some(&format!("v{n}")))
            })
            .collect::<Vec<_>>();
        t.commit(&ops).unwrap();
    }
    assert_eq!(t.report().rows, 2048);
    assert_eq!(t.report().generation, 128);
    assert!(t.report().pages > 1024);
    assert!(t.report().cached_pages <= 64);
    let before = bytes(&p);
    assert!(t
        .commit(&[m("new", 0, Some("v")), m("k0001", 0, Some("bad"))])
        .is_err());
    assert_eq!(bytes(&p), before);
    assert!(t
        .commit(&[m("k0001", 1, Some("a")), m("k0001", 1, Some("b"))])
        .is_err());
    assert!(t
        .commit(&[m("large", 0, Some(&"x".repeat(65_537)))])
        .is_err());
    assert!(t.commit(&[m("missing", 0, None)]).is_err());
    assert_eq!(bytes(&p), before);
    t.commit(&[
        m("k0001", 1, None),
        m("k0002", 1, Some("новое")),
        m("new", 0, Some("")),
    ])
    .unwrap();
    for (k, v, rev) in [
        ("k0001", None, 2),
        ("k0002", Some("новое"), 2),
        ("new", Some(""), 1),
    ] {
        expected.insert(
            k.into(),
            Record {
                key: k.into(),
                revision: rev,
                value: v.map(str::to_owned),
            },
        );
    }
    assert!(CowTree::open(&p).is_err());
    drop(t);
    let mut t = CowTree::open(&p).unwrap();
    for (k, v) in &expected {
        assert_eq!(t.get(k).unwrap().as_ref(), Some(v));
    }
    assert!(t.get("absent").unwrap().is_none());
    for (a, b) in [
        ("a", "z"),
        ("k0000", "k0001"),
        ("k0999", "k1600"),
        ("k2047", "z"),
        ("k0100", "k0101"),
    ] {
        for limit in [1, 8, 256] {
            let want = expected
                .range(a.to_owned()..b.to_owned())
                .take(limit)
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>();
            assert_eq!(t.range(a, b, limit).unwrap(), want);
        }
    }
    let hits = t.report().cache_hits;
    t.get("k0002").unwrap();
    t.get("k0002").unwrap();
    assert!(t.report().cache_hits > hits);
    assert!(t.report().cached_pages <= 64);
    assert_eq!(t.report().rows, 2049);
    assert_eq!(t.report().generation, 129);
}
#[test]
fn corruption_truncation_and_unpublished_tail() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    t.commit(&[m("a", 0, Some("one"))]).unwrap();
    drop(t);
    let (data, head) = bytes(&p);
    let mut extra = data.clone();
    extra.extend(b"torn unpublished page");
    fs::write(&p, extra).unwrap();
    let mut t = CowTree::open(&p).unwrap();
    assert_eq!(t.get("a").unwrap().unwrap().value.as_deref(), Some("one"));
    drop(t);
    assert_eq!(bytes(&p), (data.clone(), head.clone()));
    fs::write(&p, &data[..data.len() - 1]).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(&p, &data).unwrap();
    let mut corrupt = data.clone();
    let i = corrupt.len() - 50;
    corrupt[i] ^= 1;
    fs::write(&p, corrupt).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(&p, &data).unwrap();
    let foreign = d.path().join("foreign");
    drop(CowTree::create(&foreign).unwrap());
    fs::write(
        side(&p, ".head"),
        fs::read(side(&foreign, ".head")).unwrap(),
    )
    .unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(side(&p, ".head"), &head).unwrap();
    let mut corrupt = head.clone();
    corrupt[60] ^= 1;
    fs::write(side(&p, ".head"), corrupt).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(side(&p, ".head"), head).unwrap();
    assert!(CowTree::create(&p).is_err());
    let link = d.path().join("link");
    fs::hard_link(&p, &link).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::remove_file(link).unwrap();
    assert!(CowTree::open(&p).is_ok());
}
#[test]
fn crash_child() {
    let Ok(p) = std::env::var("B29_CHILD_PATH") else {
        return;
    };
    let mut t = CowTree::open(Path::new(&p)).unwrap();
    t.commit(&[m("a", 1, Some("new")), m("b", 0, Some("second"))])
        .unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn actual_process_exit_at_publication_boundaries() {
    for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
        let d = dir();
        let p = d.path().join("tree");
        let mut t = CowTree::create(&p).unwrap();
        t.commit(&[m("a", 0, Some("old"))]).unwrap();
        drop(t);
        let before = bytes(&p);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture"])
            .env("B29_CHILD_PATH", &p)
            .env("SEDRAVASQL_NATIVE_TEST_FAULT", point)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert_eq!(child.wait().unwrap().code(), Some(86));
        // Only this fixture's confirmed-dead child owns the lock.
        let lock = side(&p, ".lock");
        assert_eq!(
            fs::read_to_string(&lock).unwrap().trim(),
            format!("pid={pid}")
        );
        assert!(CowTree::open(&p).is_err());
        fs::remove_file(lock).unwrap();
        if point == "cow_after_stage" {
            assert!(CowTree::open(&p).is_err());
            assert_eq!(fs::read(side(&p, ".head")).unwrap(), before.1);
            fs::remove_file(side(&p, ".head.tmp")).unwrap();
        }
        let mut t = CowTree::open(&p).unwrap();
        if point == "cow_after_publish" {
            assert_eq!(t.report().generation, 2);
            assert_eq!(t.get("a").unwrap().unwrap().value.as_deref(), Some("new"));
            assert_eq!(
                t.get("b").unwrap().unwrap().value.as_deref(),
                Some("second")
            );
        } else {
            assert_eq!(t.report().generation, 1);
            assert_eq!(t.get("a").unwrap().unwrap().value.as_deref(), Some("old"));
            assert!(t.get("b").unwrap().is_none());
            assert_eq!(bytes(&p), before);
        }
        // After recovery the next commit must remain possible.
        let rev = t.get("a").unwrap().unwrap().revision;
        t.commit(&[m("a", rev, Some("after"))]).unwrap();
        drop(t);
        assert_eq!(
            CowTree::open(&p)
                .unwrap()
                .get("a")
                .unwrap()
                .unwrap()
                .value
                .as_deref(),
            Some("after")
        );
    }
}
