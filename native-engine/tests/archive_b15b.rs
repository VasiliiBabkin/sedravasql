#![cfg(unix)]
use sedravasql_native_engine::{
    archive::{create_archive, restore_archive, verify_archive},
    NativeChange, NativeLog, NativeMutation, TransactionalStore,
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt};
fn sha(p: &std::path::Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).unwrap()))
}
#[test]
fn archive_exact_restore_receipts_and_new_commit() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = d.path().join("source");
    let archive = d.path().join("archive");
    let dest = d.path().join("restored");
    let mut db = NativeLog::create(&source).unwrap();
    for i in 0..64 {
        db.commit_batch(
            &format!("archive-request-{i:04}"),
            &[NativeMutation {
                namespace: "n".into(),
                key: "k".into(),
                expected_revision: i,
                change: NativeChange::Put(format!("{}-{i}", "repeat".repeat(120))),
            }],
        )
        .unwrap();
    }
    let original = fs::read(&source).unwrap();
    let digest = sha(&source);
    assert!(create_archive(&source, &archive, &digest).is_err());
    assert!(!archive.exists());
    drop(db);
    assert!(create_archive(&source, &archive, &"0".repeat(64)).is_err());
    let report = create_archive(&source, &archive, &digest).unwrap();
    assert_eq!(verify_archive(&archive).unwrap(), report);
    assert!(report.archive_bytes < report.source_bytes);
    assert!(NativeLog::open(&archive).is_err());
    assert_eq!(sha(&source), digest);
    assert!(create_archive(&source, &archive, &digest).is_err());
    assert!(restore_archive(&archive, &dest, &"0".repeat(64)).is_err());
    assert!(!dest.exists());
    assert_eq!(
        restore_archive(&archive, &dest, &report.archive_sha256).unwrap(),
        report
    );
    assert_eq!(fs::read(&dest).unwrap(), original);
    let mut db = NativeLog::open(&dest).unwrap();
    assert_eq!(db.sequence(), 64);
    assert_eq!(
        db.lookup_receipt("archive-request-0000")
            .unwrap()
            .unwrap()
            .sequence,
        1
    );
    db.commit_batch(
        "archive-request-0064",
        &[NativeMutation {
            namespace: "n".into(),
            key: "k".into(),
            expected_revision: 64,
            change: NativeChange::Delete,
        }],
    )
    .unwrap();
    drop(db);
    assert_eq!(sha(&source), digest);
    assert_eq!(sha(&archive), report.archive_sha256);
}
#[test]
fn archive_corruption_rejected_even_with_recomputed_outer_hash() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = d.path().join("source");
    let archive = d.path().join("archive");
    let dest = d.path().join("dest");
    drop(NativeLog::create(&source).unwrap());
    create_archive(&source, &archive, &sha(&source)).unwrap();
    let good = fs::read(&archive).unwrap();
    for mode in 0..4 {
        let mut bad = good.clone();
        match mode {
            0 => bad[24] ^= 1,
            1 => {
                bad.truncate(bad.len() - 1);
            }
            2 => {
                bad[16..24].copy_from_slice(&1u64.to_be_bytes());
                let n = bad.len() - 32;
                let h = Sha256::digest(&bad[..n]);
                bad[n..].copy_from_slice(&h);
            }
            _ => {
                let n = bad.len() - 32;
                bad.insert(n, 0);
                let n = bad.len() - 32;
                let h = Sha256::digest(&bad[..n]);
                bad[n..].copy_from_slice(&h);
            }
        }
        fs::write(&archive, &bad).unwrap();
        assert!(verify_archive(&archive).is_err(), "mode {mode}");
        assert!(restore_archive(&archive, &dest, &sha(&archive)).is_err());
        assert!(!dest.exists());
        assert_eq!(fs::read(&archive).unwrap(), bad);
    }
}
