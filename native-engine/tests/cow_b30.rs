#![cfg(unix)]
use sedravasql_native_engine::cow_tree::{CowTree, Mutation, Record};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn side(p: &Path, s: &str) -> PathBuf {
    let mut n = p.file_name().unwrap().to_os_string();
    n.push(s);
    p.with_file_name(n)
}
fn bytes(p: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(p).unwrap(), fs::read(side(p, ".head")).unwrap())
}
fn m(key: &str, rev: u64, value: Option<&str>) -> Mutation {
    Mutation {
        key: key.into(),
        expected_revision: rev,
        value: value.map(str::to_owned),
    }
}
fn records(t: &mut CowTree) -> Vec<Record> {
    // This fixture has at most 240 keys. Includes tombstones.
    t.range("a", "z", 256).unwrap()
}
#[test]
fn packed_copy_preserves_generation_revisions_and_tombstones() {
    let d = dir();
    let p = d.path().join("source");
    let dest = d.path().join("packed");
    let mut t = CowTree::create(&p).unwrap();
    for i in 0..240 {
        let k = format!("k{:04}", i * 137 % 240);
        t.commit(&[m(&k, 0, Some("value"))]).unwrap();
    }
    for r in 1..=96 {
        t.commit(&[m("k0000", r, Some("updated"))]).unwrap();
    }
    t.commit(&[m("k0001", 1, None)]).unwrap();
    let expected = records(&mut t);
    let old = t.report();
    let original = bytes(&p);
    let packed = t.compact_copy(&dest).unwrap();
    assert_eq!(packed.generation, old.generation);
    assert_eq!(packed.rows, old.rows);
    assert!(packed.pages < packed.generation);
    assert!(packed.file_bytes * 10 < old.file_bytes);
    assert_eq!(bytes(&p), original);
    assert_ne!(&bytes(&dest).0[8..24], &original.0[8..24]);
    let saved = bytes(&dest);
    assert!(t.compact_copy(&dest).is_err());
    assert!(t.compact_copy(&p).is_err());
    assert!(t.compact_copy(&side(&p, ".head.tmp")).is_err());
    assert!(!side(&p, ".head.tmp").exists());
    assert_eq!(bytes(&dest), saved);
    let mut copy = CowTree::open(&dest).unwrap();
    assert_eq!(records(&mut copy), expected);
    assert_eq!(copy.get("k0001").unwrap().unwrap().value, None);
    assert!(copy.commit(&[m("k0000", 1, Some("stale"))]).is_err());
    copy.commit(&[m("k0000", 97, Some("new")), m("k0001", 2, Some("restored"))])
        .unwrap();
    assert_eq!(copy.report().generation, old.generation + 1);
    drop(copy);
    let mut copy = CowTree::open(&dest).unwrap();
    assert_eq!(copy.get("k0000").unwrap().unwrap().revision, 98);
    assert_eq!(copy.get("k0001").unwrap().unwrap().revision, 3);
    assert_eq!(records(&mut t), expected);
    assert_eq!(bytes(&p), original);
}
#[test]
fn empty_legacy_head_and_uncached_corruption() {
    let d = dir();
    let p = d.path().join("source");
    let dest = d.path().join("packed");
    let mut t = CowTree::create(&p).unwrap();
    assert_eq!(t.compact_copy(&dest).unwrap().rows, 0);
    drop(t);
    // Construct exactly the prior checksummed v1 header representation.
    let mut head = fs::read(side(&p, ".head")).unwrap();
    head[..8].copy_from_slice(b"SEDRHDR1");
    let digest = Sha256::digest(&head[..4064]);
    head[4064..].copy_from_slice(&digest);
    fs::write(side(&p, ".head"), head).unwrap();
    let mut t = CowTree::open(&p).unwrap();
    t.commit(&[m("a", 0, Some("one"))]).unwrap();
    assert_eq!(&fs::read(side(&p, ".head")).unwrap()[..8], b"SEDRHDR3");
    assert!(t.get("a").unwrap().is_some()); // warm cache
    let mut b = fs::read(&p).unwrap();
    let last = b.len() - 50;
    b[last] ^= 1;
    fs::write(&p, &b).unwrap();
    let target = d.path().join("corrupt-copy");
    assert!(t.compact_copy(&target).is_err());
    assert!(!target.exists());
    assert!(!side(&target, ".head").exists());
}
#[test]
fn compact_child() {
    let Ok(source) = std::env::var("B30_SOURCE") else {
        return;
    };
    let target = std::env::var("B30_TARGET").unwrap();
    let mut t = CowTree::open(Path::new(&source)).unwrap();
    t.compact_copy(Path::new(&target)).unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn compact_process_exit_never_mutates_source() {
    for point in [
        "cow_compact_after_pages",
        "cow_compact_after_stage",
        "cow_compact_after_publish",
    ] {
        let d = dir();
        let p = d.path().join("source");
        let dest = d.path().join("dest");
        let mut t = CowTree::create(&p).unwrap();
        t.commit(&[m("a", 0, Some("one")), m("b", 0, Some("two"))])
            .unwrap();
        t.commit(&[m("a", 1, None)]).unwrap();
        let expected = records(&mut t);
        drop(t);
        let before = bytes(&p);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "compact_child", "--nocapture"])
            .env("B30_SOURCE", &p)
            .env("B30_TARGET", &dest)
            .env("SEDRAVASQL_NATIVE_TEST_FAULT", point)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert_eq!(child.wait().unwrap().code(), Some(86));
        assert_eq!(bytes(&p), before);
        for path in [&p, &dest] {
            let lock = side(path, ".lock");
            assert_eq!(
                fs::read_to_string(&lock).unwrap().trim(),
                format!("pid={pid}")
            );
            fs::remove_file(lock).unwrap();
        }
        assert_eq!(records(&mut CowTree::open(&p).unwrap()), expected);
        if point == "cow_compact_after_publish" {
            let mut copy = CowTree::open(&dest).unwrap();
            assert_eq!(records(&mut copy), expected);
            assert_eq!(copy.report().generation, 2);
        } else {
            assert!(CowTree::open(&dest).is_err());
            // Incomplete destination is never implicitly overwritten.
            assert!(CowTree::open(&p).unwrap().compact_copy(&dest).is_err());
        }
    }
}
