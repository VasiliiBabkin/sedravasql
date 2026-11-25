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
fn op(i: u64) -> Mutation {
    Mutation {
        key: "record".into(),
        expected_revision: i,
        value: Some(format!("value-{i}")),
    }
}
fn bytes(p: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(p).unwrap(), fs::read(side(p, ".head")).unwrap())
}
fn root(h: &[u8]) -> usize {
    u64::from_be_bytes(h[56..64].try_into().unwrap()) as usize
}
fn seal(p: &mut [u8]) {
    let digest = Sha256::digest(&p[..4064]);
    p[4064..].copy_from_slice(&digest);
}
#[test]
fn packed_history_crosses_multiple_pages_and_compacts() {
    let d = dir();
    let p = d.path().join("tree");
    let q = d.path().join("packed");
    let mut t = CowTree::create(&p).unwrap();
    for i in 0..240 {
        let r = t.commit_receipted(&format!("req{i}"), &[op(i)]).unwrap();
        assert_eq!(r.generation, i + 1);
        assert!(!r.replayed);
        if [47, 48, 95, 96, 143, 144, 239].contains(&i) {
            drop(t);
            t = CowTree::open(&p).unwrap();
        }
    }
    let before = bytes(&p);
    assert_eq!(&before.1[..8], b"SEDRHDR5");
    assert_eq!(
        u64::from_be_bytes(before.1[64..72].try_into().unwrap()),
        240
    );
    assert!(t.report().pages < 30, "{:?}", t.report());
    for i in [0, 47, 48, 95, 96, 143, 239] {
        let r = t.commit_receipted(&format!("req{i}"), &[op(i)]).unwrap();
        assert!(r.replayed);
        assert_eq!(r.generation, i + 1);
    }
    assert_eq!(before, bytes(&p));
    t.compact_copy(&q).unwrap();
    assert_eq!(before, bytes(&p));
    let mut c = CowTree::open(&q).unwrap();
    assert_eq!(c.report().pages, 7);
    for i in 0..240 {
        assert!(
            c.commit_receipted(&format!("req{i}"), &[op(i)])
                .unwrap()
                .replayed
        );
    }
    c.commit_receipted("req240", &[op(240)]).unwrap();
    assert_eq!(c.get("record").unwrap().unwrap().revision, 241);
}
#[test]
fn corrupt_receipt_page_or_manifest_is_rejected() {
    let d = dir();
    let p = d.path().join("tree");
    let hp = side(&p, ".head");
    let mut t = CowTree::create(&p).unwrap();
    for i in 0..49 {
        t.commit_receipted(&format!("req{i}"), &[op(i)]).unwrap();
    }
    drop(t);
    let (data, h) = bytes(&p);
    let id = root(&h);
    let mut damaged = data.clone();
    let page = &mut damaged[id * 4096..(id + 1) * 4096];
    page[40..48].copy_from_slice(&(id as u64).to_be_bytes());
    seal(page);
    fs::write(&p, damaged).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(&p, &data).unwrap();
    let mut bad = h.clone();
    bad[64..72].copy_from_slice(&((MAX_RECEIPTS + 1) as u64).to_be_bytes());
    seal(&mut bad);
    fs::write(&hp, bad).unwrap();
    assert!(CowTree::open(&p).is_err());
    let mut bad = h.clone();
    let data_root = h[32..40].to_vec();
    bad[56..64].copy_from_slice(&data_root);
    seal(&mut bad);
    fs::write(&hp, bad).unwrap();
    assert!(CowTree::open(&p).is_err());
    fs::write(&hp, h).unwrap();
    assert!(CowTree::open(&p).is_ok());
}
#[test]
fn receipt_child() {
    let Ok(p) = std::env::var("B33B_CHILD") else {
        return;
    };
    let mut t = CowTree::open(Path::new(&p)).unwrap();
    let i = t.report().generation;
    t.commit_receipted(&format!("req{i}"), &[op(i)]).unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn crash_during_v4_upgrade_and_tail_replacement() {
    for count in [48, 49] {
        for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
            let d = dir();
            let p = d.path().join("tree");
            let mut t = CowTree::create(&p).unwrap();
            for i in 0..count {
                t.commit_receipted(&format!("req{i}"), &[op(i)]).unwrap();
            }
            drop(t);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "receipt_child", "--nocapture"])
                .env("B33B_CHILD", &p)
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
            assert_eq!(t.report().generation, count + u64::from(published));
            assert!(t.commit_receipted("req0", &[op(0)]).unwrap().replayed);
            let r = t
                .commit_receipted(&format!("req{count}"), &[op(count)])
                .unwrap();
            assert_eq!(r.replayed, published);
            assert_eq!(r.generation, count + 1);
            assert_eq!(t.get("record").unwrap().unwrap().revision, count + 1);
        }
    }
}
