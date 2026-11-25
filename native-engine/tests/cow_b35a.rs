#![cfg(unix)]
use sedravasql_native_engine::{
    cow_store::CowStore, NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
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
fn side(p: &Path, s: &str) -> PathBuf {
    let mut n = p.file_name().unwrap().to_os_string();
    n.push(s);
    p.with_file_name(n)
}
fn put(ns: &str, k: &str, rev: u64, v: &str) -> NativeMutation {
    NativeMutation {
        namespace: ns.into(),
        key: k.into(),
        expected_revision: rev,
        change: NativeChange::Put(v.into()),
    }
}
fn bytes(p: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(p).unwrap(), fs::read(side(p, ".head")).unwrap())
}
#[test]
fn native_parity_large_frames_and_restart() {
    let d = dir();
    let p = d.path().join("cow");
    let q = d.path().join("log");
    let mut c = CowStore::create(&p).unwrap();
    let mut n = NativeLog::create(&q).unwrap();
    let big = "Ж".repeat(32768);
    let ops = (0..16)
        .map(|i| put("ns", &format!("k{i:02}"), 0, &big))
        .collect::<Vec<_>>();
    let r = n.commit_batch("native-req-000001", &ops).unwrap();
    assert_eq!(c.commit_batch("native-req-000001", &ops).unwrap(), r);
    let next = [
        put("ns", "k00", 1, "later"),
        NativeMutation {
            namespace: "ns".into(),
            key: "k01".into(),
            expected_revision: 1,
            change: NativeChange::Delete,
        },
        put("other", "k00", 0, "different"),
    ];
    assert_eq!(
        c.commit_batch("native-req-000002", &next).unwrap(),
        n.commit_batch("native-req-000002", &next).unwrap()
    );
    let before = bytes(&p);
    assert!(c
        .commit_batch(
            "native-req-000003",
            &[put("ns", "new", 0, "new"), put("ns", "k00", 0, "bad")]
        )
        .is_err());
    assert_eq!(before, bytes(&p));
    drop(c);
    let mut c = CowStore::open(&p).unwrap();
    c.verify().unwrap();
    assert_eq!(c.sequence(), n.sequence());
    assert_eq!(
        c.scan_namespace("ns", None, 256).unwrap(),
        n.scan_namespace("ns", None, 256).unwrap()
    );
    assert_eq!(
        c.scan_namespace_range("ns", "k02", "k05", Some("k02"), 256)
            .unwrap()
            .iter()
            .map(|r| r.key.as_str())
            .collect::<Vec<_>>(),
        vec!["k03", "k04"]
    );
    assert_eq!(
        c.read("other", "k00").unwrap(),
        n.read("other", "k00").unwrap()
    );
    assert_eq!(
        c.commit_batch("native-req-000001", &ops).unwrap(),
        n.commit_batch("native-req-000001", &ops).unwrap()
    );
    assert!(c
        .commit_batch("native-req-000001", &[put("ns", "k00", 0, "bad")])
        .is_err());
    assert_eq!(before, bytes(&p));
}
#[test]
fn wrong_format_and_invalid_history_refused() {
    use sedravasql_native_engine::cow_tree::{CowTree, Mutation};
    let d = dir();
    let p = d.path().join("cow");
    let q = d.path().join("log");
    drop(NativeLog::create(&q).unwrap());
    assert!(CowStore::open(&q).is_err());
    drop(CowStore::create(&p).unwrap());
    let mut t = CowTree::open(&p).unwrap();
    t.commit_reusing(&[Mutation {
        key: "unexpected".into(),
        expected_revision: 0,
        value: Some("junk".into()),
    }])
    .unwrap();
    drop(t);
    assert!(CowStore::open(&p).is_err());
}
#[test]
fn crash_child() {
    let Ok(p) = std::env::var("B35A_CHILD") else {
        return;
    };
    let mut c = CowStore::open(Path::new(&p)).unwrap();
    c.commit_batch(
        "native-req-000002",
        &[
            put("ns", "a", 1, &"Ж".repeat(32768)),
            put("ns", "b", 0, "new"),
        ],
    )
    .unwrap();
    panic!("fault did not fire");
}
#[cfg(debug_assertions)]
#[test]
fn crash_atomic_history_and_retry() {
    for point in ["cow_after_pages", "cow_after_stage", "cow_after_publish"] {
        let d = dir();
        let p = d.path().join("cow");
        let mut c = CowStore::create(&p).unwrap();
        c.commit_batch("native-req-000001", &[put("ns", "a", 0, "old")])
            .unwrap();
        drop(c);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture"])
            .env("B35A_CHILD", &p)
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
            assert!(CowStore::open(&p).is_err());
            fs::remove_file(side(&p, ".head.tmp")).unwrap();
        }
        let mut c = CowStore::open(&p).unwrap();
        let published = point == "cow_after_publish";
        assert_eq!(c.sequence(), 1 + u64::from(published));
        assert_eq!(
            c.lookup_receipt("native-req-000002").unwrap().is_some(),
            published
        );
        assert_eq!(c.read("ns", "b").unwrap().is_some(), published);
        let r = c
            .commit_batch(
                "native-req-000002",
                &[
                    put("ns", "a", 1, &"Ж".repeat(32768)),
                    put("ns", "b", 0, "new"),
                ],
            )
            .unwrap();
        assert_eq!(r.replayed, published);
        assert_eq!(r.sequence, 2);
        c.verify().unwrap();
    }
}
