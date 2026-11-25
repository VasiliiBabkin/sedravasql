#![cfg(unix)]
use sedravasql_native_engine::cow_tree::{CowTree, Mutation};
#[cfg(debug_assertions)]
use std::process::Command;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn side(p: &Path, suffix: &str) -> PathBuf {
    let mut n = p.file_name().unwrap().to_os_string();
    n.push(suffix);
    p.with_file_name(n)
}
fn m(key: &str, rev: u64, v: Option<&str>) -> Mutation {
    Mutation {
        key: key.into(),
        expected_revision: rev,
        value: v.map(str::to_owned),
    }
}
#[test]
fn repeated_updates_reuse_pages_and_survive_reopen() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    for rev in 0..100 {
        let value = format!("value{rev}");
        t.commit_reusing(&[m("a", rev, Some(&value))]).unwrap();
        assert_eq!(
            t.get("a").unwrap().unwrap().value.as_deref(),
            Some(value.as_str())
        );
        assert_eq!(t.get("a").unwrap().unwrap().revision, rev + 1);
        assert!(t.report().pages <= 3);
        if rev % 10 == 0 {
            drop(t);
            t = CowTree::open(&p).unwrap();
        }
    }
    assert_eq!(t.report().pages, 3);
    assert_eq!(t.report().generation, 100);
}
#[test]
fn mixed_tree_overflow_reuse_preserves_oracle_and_rejects_cas() {
    let d = dir();
    let p = d.path().join("tree");
    let copy = d.path().join("copy");
    let mut t = CowTree::create(&p).unwrap();
    let mut reference = CowTree::create(&d.path().join("reference")).unwrap();
    for i in 0..6 {
        let ops = (0..16)
            .map(|j| m(&format!("k{:03}", i * 16 + j), 0, Some("initial")))
            .collect::<Vec<_>>();
        t.commit_reusing(&ops).unwrap();
        reference.commit(&ops).unwrap();
    }
    for i in 0..48 {
        let key = format!("k{:03}", (i * 19) % 96);
        let rev = reference.get(&key).unwrap().unwrap().revision;
        let value = if i % 3 == 0 {
            Some("Ж".repeat(4001))
        } else if i % 3 == 1 {
            None
        } else {
            Some("short".into())
        };
        let ops = [m(&key, rev, value.as_deref())];
        t.commit_reusing(&ops).unwrap();
        reference.commit(&ops).unwrap();
        assert_eq!(
            t.range("k", "l", 256).unwrap(),
            reference.range("k", "l", 256).unwrap()
        );
        if i % 8 == 0 {
            drop(t);
            t = CowTree::open(&p).unwrap();
        }
    }
    assert!(t.report().pages < reference.report().pages);
    let before = (fs::read(&p).unwrap(), fs::read(side(&p, ".head")).unwrap());
    assert!(t
        .commit_reusing(&[m("new", 0, Some("new")), m("k000", 0, Some("bad"))])
        .is_err());
    assert_eq!(
        before,
        (fs::read(&p).unwrap(), fs::read(side(&p, ".head")).unwrap())
    );
    t.compact_copy(&copy).unwrap();
    assert_eq!(
        CowTree::open(&copy).unwrap().range("k", "l", 256).unwrap(),
        reference.range("k", "l", 256).unwrap()
    );
}
#[test]
fn cached_live_page_corruption_blocks_reuse_without_writes() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    t.commit_reusing(&[m("a", 0, Some("original"))]).unwrap();
    t.get("a").unwrap();
    let head = fs::read(side(&p, ".head")).unwrap();
    let id = u64::from_be_bytes(head[32..40].try_into().unwrap()) as usize;
    let mut data = fs::read(&p).unwrap();
    data[id * 4096 + 100] ^= 1;
    fs::write(&p, &data).unwrap();
    assert!(t.commit_reusing(&[m("a", 1, Some("bad"))]).is_err());
    assert_eq!(fs::read(&p).unwrap(), data);
    assert_eq!(fs::read(side(&p, ".head")).unwrap(), head);
}
#[test]
fn reuse_child() {
    let Ok(p) = std::env::var("B32_CHILD") else {
        return;
    };
    let mut t = CowTree::open(Path::new(&p)).unwrap();
    t.commit_reusing(&[m("a", 2, Some("new"))]).unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn process_exit_while_overwriting_free_page_keeps_committed_root() {
    for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
        let d = dir();
        let p = d.path().join("tree");
        let mut t = CowTree::create(&p).unwrap();
        t.commit_reusing(&[m("a", 0, Some("first"))]).unwrap();
        t.commit_reusing(&[m("a", 1, Some("second"))]).unwrap();
        assert_eq!(t.report().pages, 3);
        drop(t);
        let before = fs::read(&p).unwrap();
        let head = fs::read(side(&p, ".head")).unwrap();
        let live = u64::from_be_bytes(head[32..40].try_into().unwrap()) as usize;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "reuse_child", "--nocapture"])
            .env("B32_CHILD", &p)
            .env("SEDRAVASQL_NATIVE_TEST_FAULT", point)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert_eq!(child.wait().unwrap().code(), Some(86));
        let lock = side(&p, ".lock");
        assert_eq!(
            fs::read_to_string(&lock).unwrap().trim(),
            format!("pid={pid}")
        );
        fs::remove_file(lock).unwrap();
        if point == "cow_after_stage" {
            assert!(CowTree::open(&p).is_err());
            fs::remove_file(side(&p, ".head.tmp")).unwrap();
        }
        let after = fs::read(&p).unwrap();
        assert_ne!(before, after);
        assert_eq!(before.len(), after.len());
        assert_eq!(
            before[live * 4096..(live + 1) * 4096],
            after[live * 4096..(live + 1) * 4096]
        );
        let mut t = CowTree::open(&p).unwrap();
        let current = t.get("a").unwrap().unwrap();
        if point == "cow_after_publish" {
            assert_eq!(current.value.as_deref(), Some("new"));
            assert_eq!(current.revision, 3);
        } else {
            assert_eq!(current.value.as_deref(), Some("second"));
            assert_eq!(current.revision, 2);
        }
        t.commit_reusing(&[m("a", current.revision, Some("recovered"))])
            .unwrap();
        assert_eq!(t.report().pages, 3);
    }
}
