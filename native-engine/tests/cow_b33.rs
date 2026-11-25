#![cfg(unix)]
use sedravasql_native_engine::cow_tree::{CowTree, Mutation, MAX_RECEIPTS};
use sha2::{Digest, Sha256};
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
fn bytes(p: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(p).unwrap(), fs::read(side(p, ".head")).unwrap())
}
#[test]
fn historical_replay_survives_normal_writes_and_compaction() {
    let d = dir();
    let p = d.path().join("tree");
    let dest = d.path().join("compact");
    let mut t = CowTree::create(&p).unwrap();
    let value = "Ж".repeat(5000);
    let ops = [m("a", 0, Some(&value)), m("b", 0, Some(""))];
    let first = t.commit_receipted("req1", &ops).unwrap();
    assert_eq!(first.generation, 1);
    assert!(!first.replayed);
    assert_eq!(&bytes(&p).1[..8], b"SEDRHDR4");
    t.commit(&[m("a", 1, Some("later"))]).unwrap();
    t.commit_reusing(&[m("b", 1, None)]).unwrap();
    drop(t);
    let mut t = CowTree::open(&p).unwrap();
    let before = bytes(&p);
    let r = t.commit_receipted("req1", &ops).unwrap();
    assert_eq!(r.generation, 1);
    assert!(r.replayed);
    assert_eq!(t.report().generation, 3);
    assert_eq!(t.get("a").unwrap().unwrap().value.as_deref(), Some("later"));
    assert!(t.get("b").unwrap().unwrap().value.is_none());
    assert!(t
        .commit_receipted("req1", &[ops[1].clone(), ops[0].clone()])
        .is_err());
    assert!(t
        .commit_receipted("req1", &[m("a", 0, Some(&value)), m("b", 0, None)])
        .is_err());
    assert!(t
        .commit_receipted("req1", &[m("a", 1, Some(&value)), ops[1].clone()])
        .is_err());
    assert_eq!(before, bytes(&p));
    t.compact_copy(&dest).unwrap();
    assert_eq!(before, bytes(&p));
    let mut copy = CowTree::open(&dest).unwrap();
    assert_eq!(copy.commit_receipted("req1", &ops).unwrap(), r);
    assert_eq!(copy.report().generation, 3);
    assert_eq!(
        copy.commit_receipted("req2", &[m("c", 0, Some("new"))])
            .unwrap()
            .generation,
        4
    );
}
#[test]
fn failed_cas_does_not_reserve_key_and_capacity_fails_closed() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    t.commit(&[m("a", 0, Some("first"))]).unwrap();
    let before = bytes(&p);
    assert!(t
        .commit_receipted("same", &[m("b", 0, Some("new")), m("a", 0, Some("bad"))])
        .is_err());
    assert_eq!(before, bytes(&p));
    t.commit_receipted("same", &[m("a", 1, Some("ok"))])
        .unwrap();
    for i in 1..MAX_RECEIPTS {
        let rev = t.get("a").unwrap().unwrap().revision;
        t.commit_receipted(&format!("req{i}"), &[m("a", rev, Some("next"))])
            .unwrap();
    }
    drop(t);
    let mut t = CowTree::open(&p).unwrap();
    let before = bytes(&p);
    let rev = t.get("a").unwrap().unwrap().revision;
    assert!(t
        .commit_receipted("overflow", &[m("a", rev, Some("no"))])
        .is_err());
    assert_eq!(before, bytes(&p));
    assert!(
        t.commit_receipted("same", &[m("a", 1, Some("ok"))])
            .unwrap()
            .replayed
    );
    assert_eq!(before, bytes(&p));
}
fn seal(page: &mut [u8]) {
    let digest = Sha256::digest(&page[..4064]);
    page[4064..].copy_from_slice(&digest);
}
#[test]
fn malformed_manifest_rejected_even_with_valid_checksum() {
    let d = dir();
    let p = d.path().join("tree");
    let hp = side(&p, ".head");
    let mut t = CowTree::create(&p).unwrap();
    t.commit_receipted("a1", &[m("a", 0, Some("1"))]).unwrap();
    t.commit_receipted("a2", &[m("a", 1, Some("2"))]).unwrap();
    let original = fs::read(&hp).unwrap();
    let mut h = original.clone();
    h[58 + 72..58 + 72 + 32].copy_from_slice(&original[58..90]);
    seal(&mut h);
    fs::write(&hp, &h).unwrap();
    assert!(t.commit_receipted("a1", &[m("a", 0, Some("1"))]).is_err());
    drop(t);
    assert!(CowTree::open(&p).is_err());
    let mut h = original.clone();
    h[122..130].copy_from_slice(&99u64.to_be_bytes());
    seal(&mut h);
    fs::write(&hp, h).unwrap();
    assert!(CowTree::open(&p).is_err());
    let mut h = original.clone();
    h[56..58].copy_from_slice(&0u16.to_be_bytes());
    seal(&mut h);
    fs::write(&hp, h).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(&hp, original).unwrap();
    assert!(CowTree::open(&p).is_ok());
}
#[test]
fn receipt_child() {
    let Ok(p) = std::env::var("B33_CHILD") else {
        return;
    };
    let mut t = CowTree::open(Path::new(&p)).unwrap();
    t.commit_receipted(
        "lost-response",
        &[m("a", 2, Some("third")), m("b", 0, Some(&"Z".repeat(8001)))],
    )
    .unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn crash_receipt_and_data_publish_together() {
    for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
        let d = dir();
        let p = d.path().join("tree");
        let mut t = CowTree::create(&p).unwrap();
        t.commit_receipted("first", &[m("a", 0, Some("first"))])
            .unwrap();
        t.commit_reusing(&[m("a", 1, Some("second"))]).unwrap();
        drop(t);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "receipt_child", "--nocapture"])
            .env("B33_CHILD", &p)
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
        let mut t = CowTree::open(&p).unwrap();
        let published = point == "cow_after_publish";
        assert_eq!(
            t.get("a").unwrap().unwrap().value.as_deref(),
            Some(if published { "third" } else { "second" })
        );
        assert_eq!(t.get("b").unwrap().is_some(), published);
        assert!(
            t.commit_receipted("first", &[m("a", 0, Some("first"))])
                .unwrap()
                .replayed
        );
        let ops = [m("a", 2, Some("third")), m("b", 0, Some(&"Z".repeat(8001)))];
        let r = t.commit_receipted("lost-response", &ops).unwrap();
        assert_eq!(r.generation, 3);
        assert_eq!(r.replayed, published);
        assert_eq!(t.report().generation, 3);
        assert_eq!(t.get("b").unwrap().unwrap().value.unwrap().len(), 8001);
        assert!(t.commit_receipted("lost-response", &ops).unwrap().replayed);
    }
}
