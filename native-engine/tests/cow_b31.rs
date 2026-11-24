#![cfg(unix)]
use sedravasql_native_engine::cow_tree::{CowTree, Mutation};
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
fn boundaries_unicode_atomic_batches_and_compact() {
    let d = dir();
    let p = d.path().join("tree");
    let target = d.path().join("packed");
    let mut t = CowTree::create(&p).unwrap();
    let values = [
        "".into(),
        "a".repeat(256),
        "b".repeat(257),
        "x".repeat(4000),
        "x".repeat(4001),
        format!("{}🌍", "x".repeat(3999)),
        "Ж".repeat(32768),
    ];
    let ops = values
        .iter()
        .enumerate()
        .map(|(i, v)| m(&format!("a{i}"), 0, Some(v)))
        .collect::<Vec<_>>();
    t.commit(&ops).unwrap();
    let full = "Q".repeat(65536);
    let max_batch = (0..16)
        .map(|i| m(&format!("b{i:02}"), 0, Some(&full)))
        .collect::<Vec<_>>();
    t.commit(&max_batch).unwrap();
    let before = bytes(&p);
    assert!(t
        .commit(&[m("new", 0, Some("yes")), m("a0", 0, Some(&full))])
        .is_err());
    assert!(t
        .commit(&[m("oversize", 0, Some(&"z".repeat(65537)))])
        .is_err());
    assert!(t
        .commit(&[m("nul", 0, Some(&format!("{}\0", "z".repeat(4000))))])
        .is_err());
    assert_eq!(bytes(&p), before);
    drop(t);
    let mut t = CowTree::open(&p).unwrap();
    for (i, v) in values.iter().enumerate() {
        assert_eq!(
            t.get(&format!("a{i}")).unwrap().unwrap().value.as_ref(),
            Some(v)
        );
    }
    for i in 0..16 {
        assert_eq!(
            t.get(&format!("b{i:02}")).unwrap().unwrap().value.as_ref(),
            Some(&full)
        );
    }
    t.commit(&[
        m("a1", 1, Some(&full)),
        m("a2", 1, Some("small")),
        m("a6", 1, None),
        m("b00", 1, Some(&"R".repeat(5000))),
    ])
    .unwrap();
    t.commit(&[m("a6", 2, Some(&values[6]))]).unwrap();
    let expected = t.range("a", "z", 256).unwrap();
    let before = bytes(&p);
    let old = t.report();
    let packed = t.compact_copy(&target).unwrap();
    assert!(packed.file_bytes < old.file_bytes);
    assert_eq!(packed.generation, old.generation);
    assert_eq!(bytes(&p), before);
    let mut copy = CowTree::open(&target).unwrap();
    assert_eq!(copy.range("a", "z", 256).unwrap(), expected);
    assert_eq!(copy.get("a6").unwrap().unwrap().revision, 3);
    assert_eq!(
        copy.get("a2").unwrap().unwrap().value.as_deref(),
        Some("small")
    );
    assert!(copy.report().cached_pages <= 64);
    copy.commit(&[m("a1", 2, None), m("b01", 1, Some("inline"))])
        .unwrap();
    drop(copy);
    assert!(CowTree::open(&target)
        .unwrap()
        .get("a1")
        .unwrap()
        .unwrap()
        .value
        .is_none());
}
fn reseal(page: &mut [u8]) {
    let digest = Sha256::digest(&page[..4064]);
    page[4064..].copy_from_slice(&digest);
}
#[test]
fn overflow_corruption_even_with_valid_page_checksum() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    t.commit(&[
        m("a", 0, Some(&"A".repeat(8001))),
        m("b", 0, Some(&"B".repeat(8001))),
    ])
    .unwrap();
    drop(t);
    let data = fs::read(&p).unwrap();
    let first = (1..data.len() / 4096)
        .find(|i| data[i * 4096 + 40] == 3)
        .unwrap();
    let page = &data[first * 4096..(first + 1) * 4096];
    // Byte mutation with correct page checksum: full value digest must reject.
    let mut changed = data.clone();
    let chunk = &mut changed[first * 4096..(first + 1) * 4096];
    chunk[53] ^= 1;
    reseal(chunk);
    fs::write(&p, &changed).unwrap();
    assert!(CowTree::open(&p).is_err());
    // Self reference with a valid checksum.
    let mut changed = data.clone();
    let chunk = &mut changed[first * 4096..(first + 1) * 4096];
    chunk[45..53].copy_from_slice(&(first as u64).to_be_bytes());
    reseal(chunk);
    fs::write(&p, &changed).unwrap();
    assert!(CowTree::open(&p).is_err());
    // Restore, then damage checksum directly.
    let mut changed = data.clone();
    changed[first * 4096 + 4064] ^= 1;
    fs::write(&p, &changed).unwrap();
    assert!(CowTree::open(&p).is_err());
    assert_eq!(page[40], 3);
    fs::write(&p, &data).unwrap();
    assert_eq!(
        CowTree::open(&p)
            .unwrap()
            .get("a")
            .unwrap()
            .unwrap()
            .value
            .unwrap()
            .len(),
        8001
    );
}
#[test]
fn shared_overflow_chain_is_rejected() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    let v = "same".repeat(1000);
    t.commit(&[m("a", 0, Some(&v)), m("b", 0, Some(&v))])
        .unwrap();
    drop(t);
    let mut data = fs::read(&p).unwrap();
    let head = fs::read(side(&p, ".head")).unwrap();
    let id = u64::from_be_bytes(head[32..40].try_into().unwrap()) as usize;
    let page = &mut data[id * 4096..(id + 1) * 4096];
    assert_eq!(page[40], 1);
    // count(2), row keylen(2)+key(1)+revision(8)+sentinel(2)+len(8)+first(8)+digest(32)
    let first_a = 43 + 2 + 2 + 1 + 8 + 2 + 8;
    let first_b = first_a + 2 + 1 + 8 + 2 + 8 + 8 + 32;
    let a = page[first_a..first_a + 8].to_vec();
    page[first_b..first_b + 8].copy_from_slice(&a);
    reseal(page);
    fs::write(&p, data).unwrap();
    assert!(CowTree::open(&p).is_err());
}
#[test]
fn legacy_v2_is_readable() {
    let d = dir();
    let p = d.path().join("tree");
    let mut t = CowTree::create(&p).unwrap();
    t.commit(&[m("old", 0, Some("legacy"))]).unwrap();
    drop(t);
    let hp = side(&p, ".head");
    let mut h = fs::read(&hp).unwrap();
    h[..8].copy_from_slice(b"SEDRHDR2");
    reseal(&mut h);
    fs::write(&hp, h).unwrap();
    let mut t = CowTree::open(&p).unwrap();
    assert_eq!(
        t.get("old").unwrap().unwrap().value.as_deref(),
        Some("legacy")
    );
    t.commit(&[m("old", 1, Some(&"v".repeat(4097)))]).unwrap();
    assert_eq!(&fs::read(hp).unwrap()[..8], b"SEDRHDR3");
}
#[test]
fn overflow_child() {
    let Ok(p) = std::env::var("B31_CHILD") else {
        return;
    };
    let mut t = CowTree::open(Path::new(&p)).unwrap();
    t.commit(&[
        m("a", 1, Some(&"new!".repeat(16384))),
        m("b", 0, Some(&"Ж".repeat(2001))),
    ])
    .unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn process_exit_with_overflow_is_atomic() {
    for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
        let d = dir();
        let p = d.path().join("tree");
        let mut t = CowTree::create(&p).unwrap();
        let old = "old".repeat(1500);
        t.commit(&[m("a", 0, Some(&old))]).unwrap();
        drop(t);
        let before = bytes(&p);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "overflow_child", "--nocapture"])
            .env("B31_CHILD", &p)
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
        if point == "cow_after_publish" {
            assert_eq!(
                t.get("a").unwrap().unwrap().value.unwrap(),
                "new!".repeat(16384)
            );
            assert_eq!(
                t.get("b").unwrap().unwrap().value.unwrap(),
                "Ж".repeat(2001)
            );
        } else {
            assert_eq!(t.get("a").unwrap().unwrap().value.as_ref(), Some(&old));
            assert!(t.get("b").unwrap().is_none());
            assert_eq!(bytes(&p), before);
        }
        let rev = t.get("a").unwrap().unwrap().revision;
        t.commit(&[m("a", rev, Some("recovered"))]).unwrap();
    }
}
