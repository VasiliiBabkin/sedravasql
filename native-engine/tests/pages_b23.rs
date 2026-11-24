#![cfg(unix)]
use sedravasql_native_engine::{
    pages::{self, PageReader},
    NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}
#[test]
fn refresh_checked_generation() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = d.path().join("log");
    let image = d.path().join("image");
    let stage = d.path().join("stage");
    let mut db = NativeLog::create(&log).unwrap();
    pages::build_from_log(&mut db, &image).unwrap();
    let digest = sha(&image);
    let mut prior = PageReader::open(&image).unwrap();
    db.commit_batch(
        "refresh-test-001",
        &[NativeMutation {
            namespace: "x".into(),
            key: "y".into(),
            expected_revision: 0,
            change: NativeChange::Put("new".into()),
        }],
    )
    .unwrap();
    let before = sha(&log);
    assert!(pages::refresh(&mut db, &image, &stage, &"0".repeat(64)).is_err());
    assert!(!stage.exists());
    assert_eq!(sha(&image), digest);
    let report = pages::refresh(&mut db, &image, &stage, &digest).unwrap();
    assert_eq!(report.sequence, 1);
    assert!(!stage.exists());
    assert!(prior.get("x", "y").is_err());
    let mut current = PageReader::open(&image).unwrap();
    assert!(current.matches_log(&mut db).unwrap());
    assert_eq!(
        current.get("x", "y").unwrap().unwrap().value.as_deref(),
        Some("new")
    );
    assert_eq!(sha(&log), before);
    let foreign = d.path().join("foreign");
    let mut other = NativeLog::create(&foreign).unwrap();
    other
        .commit_batch(
            "refresh-other-01",
            &[NativeMutation {
                namespace: "x".into(),
                key: "y".into(),
                expected_revision: 0,
                change: NativeChange::Put("different".into()),
            }],
        )
        .unwrap();
    assert!(pages::refresh(&mut other, &image, &stage, &sha(&image)).is_err());
    assert!(!stage.exists());
}
