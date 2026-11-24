use sedravasql_native_engine::backup::plan_backup;
use sedravasql_native_engine::index::{create_index, lookup_index, verify_index};
use sedravasql_native_engine::snapshot::create_snapshot;
use sedravasql_native_engine::{NativeChange, NativeLog, NativeMutation, TransactionalStore};
use std::fs;
use tempfile::tempdir;

fn seed() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
) {
    let d = tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = d.path().join("source.slog");
    let snap = d.path().join("source.snap");
    let mut db = NativeLog::create(&log).unwrap();
    for rev in 0..8 {
        db.commit_batch(
            &format!("index-request-{rev:08}"),
            &[
                NativeMutation {
                    namespace: "demo".into(),
                    key: "alpha".into(),
                    expected_revision: rev,
                    change: NativeChange::Put(format!("α-{rev}")),
                },
                NativeMutation {
                    namespace: "demo".into(),
                    key: "beta".into(),
                    expected_revision: rev,
                    change: NativeChange::Put(format!("β-{rev}")),
                },
            ],
        )
        .unwrap();
    }
    db.commit_batch(
        "index-request-delete-beta",
        &[NativeMutation {
            namespace: "demo".into(),
            key: "beta".into(),
            expected_revision: 8,
            change: NativeChange::Delete,
        }],
    )
    .unwrap();
    drop(db);
    let approved = plan_backup(&log).unwrap().source_sha256;
    let snap_report = create_snapshot(&log, &snap, &approved).unwrap();
    (d, log, snap, snap_report.snapshot_sha256)
}
#[test]
fn verified_index_is_deterministic_and_keeps_tombstone_revision() {
    let (d, log, snap, sha) = seed();
    let first = d.path().join("first.idx");
    let second = d.path().join("second.idx");
    let before_log = fs::read(&log).unwrap();
    let before_snap = fs::read(&snap).unwrap();
    let report = create_index(&snap, &first, &sha).unwrap();
    let report2 = create_index(&snap, &second, &sha).unwrap();
    assert_eq!(report, report2);
    assert_eq!(report.schema, "sedravasql.index.b8-offline.v1");
    assert_eq!(report.sequence, 9);
    assert_eq!(report.entries, 2);
    assert_eq!(verify_index(&first).unwrap(), report);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert_eq!(
        lookup_index(&first, "demo", "alpha")
            .unwrap()
            .unwrap()
            .value
            .as_deref(),
        Some("α-7")
    );
    let tombstone = lookup_index(&first, "demo", "beta").unwrap().unwrap();
    assert_eq!(tombstone.revision, 9);
    assert_eq!(tombstone.value, None);
    assert!(lookup_index(&first, "demo", "missing").unwrap().is_none());
    assert!(lookup_index(&first, "../evil", "alpha").is_err());
    assert_eq!(fs::read(log).unwrap(), before_log);
    assert_eq!(fs::read(snap).unwrap(), before_snap);
    assert!(NativeLog::open(&first).is_err());
}
#[test]
fn corrupt_truncated_and_trailing_bytes_are_rejected_on_lookup_and_verify() {
    let (d, _, snap, sha) = seed();
    let path = d.path().join("working.idx");
    create_index(&snap, &path, &sha).unwrap();
    let original = fs::read(&path).unwrap();
    for i in [
        0,
        8,
        12,
        16,
        original.len() / 2,
        original.len() - 33,
        original.len() - 1,
    ] {
        let mut changed = original.clone();
        changed[i] ^= 1;
        fs::write(&path, changed).unwrap();
        assert!(verify_index(&path).is_err(), "flip at {i}");
        assert!(lookup_index(&path, "demo", "alpha").is_err(), "flip at {i}");
    }
    for len in [0, 1, 11, 16, 48, original.len() - 1] {
        fs::write(&path, &original[..len]).unwrap();
        assert!(verify_index(&path).is_err(), "truncated {len}");
    }
    let mut extended = original.clone();
    extended.push(42);
    fs::write(&path, extended).unwrap();
    assert!(verify_index(&path).is_err());
    fs::write(&path, original).unwrap();
    assert!(verify_index(&path).is_ok());
}
#[test]
fn approval_existing_destination_and_foreign_input_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (d, log, snap, sha) = seed();
    let output = d.path().join("dest.idx");
    assert!(create_index(&snap, &output, &"0".repeat(64)).is_err());
    assert!(!output.exists());
    assert!(create_index(&log, &output, &sha).is_err());
    assert!(!output.exists());
    fs::write(&output, b"valuable").unwrap();
    assert!(create_index(&snap, &output, &sha).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"valuable");
    fs::remove_file(&output).unwrap();
    symlink("missing", &output).unwrap();
    assert!(create_index(&snap, &output, &sha).is_err());
    assert!(output.is_symlink());
    fs::remove_file(&output).unwrap();
    let alias = d.path().join("hardlink.snap");
    fs::hard_link(&snap, &alias).unwrap();
    assert!(create_index(&snap, &output, &sha).is_err());
    fs::remove_file(alias).unwrap();
    let public = d.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(create_index(&snap, &public.join("index"), &sha).is_err());
}
#[test]
fn empty_snapshot_gives_empty_valid_index() {
    let d = tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = d.path().join("empty.slog");
    let snapshot = d.path().join("empty.snap");
    let index = d.path().join("empty.idx");
    drop(NativeLog::create(&source).unwrap());
    let plan = plan_backup(&source).unwrap();
    let report = create_snapshot(&source, &snapshot, &plan.source_sha256).unwrap();
    let created = create_index(&snapshot, &index, &report.snapshot_sha256).unwrap();
    assert_eq!(created.sequence, 0);
    assert_eq!(created.entries, 0);
    assert!(lookup_index(&index, "demo", "missing").unwrap().is_none());
}
