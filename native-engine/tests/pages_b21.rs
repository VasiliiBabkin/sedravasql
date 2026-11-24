#![cfg(unix)]
use sedravasql_native_engine::{
    pages::{self, PageReader, PAGE_SIZE},
    snapshot::create_snapshot,
    NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
fn hash(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}
#[test]
fn multi_level_tree_and_generation_guard() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = d.path().join("log");
    let snap = d.path().join("snap");
    let image = d.path().join("pages");
    let mut db = NativeLog::create(&log).unwrap();
    for batch in 0..20 {
        let ops = (0..16)
            .map(|i| {
                let n = batch * 16 + i;
                NativeMutation {
                    namespace: "items".into(),
                    key: format!("k{n:04}"),
                    expected_revision: 0,
                    change: NativeChange::Put(if n == 159 {
                        "\"".repeat(65536)
                    } else {
                        format!("row-{n}")
                    }),
                }
            })
            .collect::<Vec<_>>();
        db.commit_batch(&format!("page-build-{batch:04}"), &ops)
            .unwrap();
    }
    db.commit_batch(
        "page-delete-0001",
        &[NativeMutation {
            namespace: "items".into(),
            key: "k0007".into(),
            expected_revision: 1,
            change: NativeChange::Delete,
        }],
    )
    .unwrap();
    drop(db);
    let original = hash(&log);
    create_snapshot(&log, &snap, &original).unwrap();
    let sh = hash(&snap);
    let report = pages::create(&snap, &image, &sh).unwrap();
    assert_eq!(report.height, 3);
    assert_eq!(report.rows, 320);
    assert_eq!(
        fs::metadata(&image).unwrap().len(),
        u64::from(report.pages) * PAGE_SIZE as u64
    );
    let mut r = PageReader::open(&image).unwrap();
    let mut db = NativeLog::open(&log).unwrap();
    assert!(r.matches_log(&mut db).unwrap());
    for i in [0, 7, 12, 143, 144, 159, 319, 999] {
        let key = format!("k{i:04}");
        let expected = db.read("items", &key).unwrap();
        let start = r.pages_read();
        assert_eq!(r.get("items", &key).unwrap(), expected);
        if i != 159 {
            assert!(r.pages_read() - start <= 4);
        }
    }
    assert!(r.get("aaa", "x").unwrap().is_none());
    db.commit_batch(
        "page-change-0001",
        &[NativeMutation {
            namespace: "items".into(),
            key: "k0000".into(),
            expected_revision: 1,
            change: NativeChange::Put("changed".into()),
        }],
    )
    .unwrap();
    assert!(!r.matches_log(&mut db).unwrap());
    assert_eq!(
        r.get("items", "k0000").unwrap().unwrap().value.as_deref(),
        Some("row-0")
    );
    assert_eq!(hash(&snap), sh);
    assert!(pages::create(&snap, &image, &sh).is_err());
    let bytes = fs::read(&image).unwrap();
    for (i, at) in [0, PAGE_SIZE + 20, bytes.len() - 1].into_iter().enumerate() {
        let dest = d.path().join(format!("bad{i}"));
        let mut copy = bytes.clone();
        copy[at] ^= 1;
        fs::write(&dest, copy).unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(PageReader::open(&dest).is_err());
    }
    let dest = d.path().join("truncated");
    fs::write(&dest, &bytes[..bytes.len() - 1]).unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(PageReader::open(&dest).is_err());
}
#[test]
fn empty_tree_and_modified_handle() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = d.path().join("log");
    let snap = d.path().join("snap");
    let image = d.path().join("pages");
    drop(NativeLog::create(&log).unwrap());
    create_snapshot(&log, &snap, &hash(&log)).unwrap();
    let report = pages::create(&snap, &image, &hash(&snap)).unwrap();
    assert_eq!(report.pages, 2);
    assert_eq!(report.rows, 0);
    let mut reader = PageReader::open(&image).unwrap();
    assert!(reader.get("ns", "a").unwrap().is_none());
    let mut bytes = fs::read(&image).unwrap();
    bytes[PAGE_SIZE + 20] ^= 1;
    fs::write(&image, bytes).unwrap();
    assert!(reader.get("ns", "a").is_err());
}
