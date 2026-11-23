use sedravasql_native_engine::recovery::{plan_recovery, recover_to_new_file};
use sedravasql_native_engine::{NativeChange, NativeLog, NativeMutation, TransactionalStore};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}
fn put(rev: u64, value: &str) -> NativeMutation {
    NativeMutation {
        namespace: "sample".into(),
        key: "key".into(),
        expected_revision: rev,
        change: NativeChange::Put(value.into()),
    }
}
fn seed(path: &Path) -> Vec<u8> {
    let mut db = NativeLog::create(path).unwrap();
    db.commit_batch("recovery-test-000001", &[put(0, "committed")])
        .unwrap();
    drop(db);
    std::fs::read(path).unwrap()
}
fn suffix(path: &Path, extra: &[u8]) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(extra).unwrap();
    f.sync_all().unwrap();
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn truncated_length_payload_and_checksum_copy_only_validated_prefix() {
    // Recover a complete first transaction and preserve the invalid source
    // for independent operator examination. Never truncate it.
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("length1", vec![1]),
        ("length3", vec![0, 0, 1]),
        ("payload", {
            let mut a = 100u32.to_be_bytes().to_vec();
            a.extend_from_slice(b"{\"seq\":2");
            a
        }),
        ("checksum", {
            // A valid-looking full payload with incomplete digest.
            let data = serde_json::json!({"seq":2,"request_key":"recovery-test-000002",
                "operations":[{"namespace":"sample","key":"key","expected_revision":1,
                    "change":{"kind":"Put","value":"pending"}}]});
            let payload = serde_json::to_vec(&data).unwrap();
            let mut a = (payload.len() as u32).to_be_bytes().to_vec();
            a.extend_from_slice(&payload);
            a.extend_from_slice(&[1, 2, 3]);
            a
        }),
    ];
    for (kind, tail) in cases {
        let root = private_tempdir();
        let src = root.path().join(format!("src-{kind}.slog"));
        let dst = root.path().join("recovered.slog");
        let original = seed(&src);
        suffix(&src, &tail);
        let corrupted = std::fs::read(&src).unwrap();
        assert!(NativeLog::open(&src).is_err(), "{kind}");
        let plan = plan_recovery(&src).unwrap();
        assert_eq!(plan.source_sha256, hash(&corrupted));
        assert_eq!(plan.source_bytes, corrupted.len() as u64);
        assert_eq!(plan.verified_prefix_bytes, original.len() as u64);
        assert_eq!(plan.incomplete_tail_bytes, tail.len() as u64);
        assert_eq!(plan.recovered_sequence, 1);
        assert!(recover_to_new_file(&src, &dst, "invalid").is_err());
        assert!(!dst.exists());
        assert!(recover_to_new_file(&src, &dst, &"f".repeat(64)).is_err());
        assert!(!dst.exists());
        let accepted = recover_to_new_file(&src, &dst, &plan.source_sha256).unwrap();
        assert_eq!(accepted.source_sha256, plan.source_sha256);
        assert_eq!(
            std::fs::read(&src).unwrap(),
            corrupted,
            "source must remain unchanged"
        );
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            original,
            "no unexpected bytes copied"
        );
        let mut recovered = NativeLog::open(&dst).unwrap();
        recovered.verify().unwrap();
        assert_eq!(recovered.sequence(), 1);
        let old = recovered
            .commit_batch("recovery-test-000001", &[put(0, "committed")])
            .unwrap();
        assert!(old.replayed);
        assert_eq!(
            recovered
                .read("sample", "key")
                .unwrap()
                .unwrap()
                .value
                .as_deref(),
            Some("committed")
        );
        assert_eq!(
            recovered
                .commit_batch("recovery-test-000002", &[put(1, "new")])
                .unwrap()
                .sequence,
            2
        );
    }
}
#[test]
fn checksum_corruption_and_invalid_record_length_fail_closed() {
    let root = private_tempdir();
    let src = root.path().join("bad.slog");
    let mut bytes = seed(&src);
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&src, &bytes).unwrap();
    assert!(plan_recovery(&src).is_err());
    assert!(recover_to_new_file(&src, &root.path().join("copy"), &hash(&bytes)).is_err());
    assert!(!root.path().join("copy").exists());
    // A malformed oversized frame length is *never* eligible for tail recovery.
    std::fs::write(&src, b"SEDRLOG1").unwrap();
    suffix(&src, &u32::MAX.to_be_bytes());
    assert!(plan_recovery(&src).is_err());
}
#[test]
fn no_recovery_when_clean_or_locked_and_destination_never_overwritten() {
    let root = private_tempdir();
    let src = root.path().join("original.slog");
    let clean = seed(&src);
    assert!(plan_recovery(&src).is_err());
    suffix(&src, &[0]);
    let original = std::fs::read(&src).unwrap();
    let mut lock = src.as_os_str().to_os_string();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    std::fs::write(&lock, b"other writer?").unwrap();
    assert!(plan_recovery(&src).is_err());
    assert!(recover_to_new_file(&src, &root.path().join("copy"), &hash(&original)).is_err());
    std::fs::remove_file(lock).unwrap();
    let dest = root.path().join("already.slog");
    std::fs::write(&dest, b"important existing data").unwrap();
    assert!(recover_to_new_file(&src, &dest, &hash(&original)).is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), b"important existing data");
    assert_eq!(std::fs::read(&src).unwrap(), original);
    assert!(original.len() > clean.len());
}
#[test]
fn receipt_history_and_two_keys_survive_recovery() {
    let root = private_tempdir();
    let src = root.path().join("source.slog");
    let mut db = NativeLog::create(&src).unwrap();
    for rev in 0..12 {
        db.commit_batch(
            &format!("recover-batch-{rev:06}"),
            &[
                NativeMutation {
                    namespace: "n".into(),
                    key: "a".into(),
                    expected_revision: rev,
                    change: NativeChange::Put(format!("a{rev}")),
                },
                NativeMutation {
                    namespace: "n".into(),
                    key: "b".into(),
                    expected_revision: rev,
                    change: NativeChange::Put(format!("b{rev}")),
                },
            ],
        )
        .unwrap();
    }
    drop(db);
    suffix(&src, &50u32.to_be_bytes());
    suffix(&src, b"{\"seq\":");
    let plan = plan_recovery(&src).unwrap();
    assert_eq!(plan.recovered_sequence, 12);
    let dst = root.path().join("copy.slog");
    recover_to_new_file(&src, &dst, &plan.source_sha256).unwrap();
    let mut recovered = NativeLog::open(&dst).unwrap();
    assert_eq!(recovered.sequence(), 12);
    for rev in 0..12 {
        let old = recovered
            .commit_batch(
                &format!("recover-batch-{rev:06}"),
                &[
                    NativeMutation {
                        namespace: "n".into(),
                        key: "a".into(),
                        expected_revision: rev,
                        change: NativeChange::Put(format!("a{rev}")),
                    },
                    NativeMutation {
                        namespace: "n".into(),
                        key: "b".into(),
                        expected_revision: rev,
                        change: NativeChange::Put(format!("b{rev}")),
                    },
                ],
            )
            .unwrap();
        assert!(old.replayed);
    }
    recovered.verify().unwrap();
}
