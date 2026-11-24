//! B4 operator-approved restore from a verified SEDRSNP1 materialization.
//! We reconstruct the exact historic SEDRLOG1 frames from idempotency
//! receipts and verify the original journal's full SHA-256 + chain hash
//! BEFORE creating a NEW destination. NO in-place swap/compaction.
use super::{
    apply, batch_fingerprint, frame_digest, load, prepare, private_file, private_parent, Error,
    Frame, NativeChange, NativeMutation, Replay, Result, FRAME_LIMIT, MAGIC,
};
use crate::backup::same_file;
use crate::recovery::file_sha256;
use crate::snapshot::{
    head_hex, is_hash, parse_snapshot, report, validate, Snapshot, MAX_SNAPSHOT_BYTES,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RestoreReport {
    pub snapshot_sha256: String,
    pub recovered_log_sha256: String,
    pub recovered_log_bytes: u64,
    pub sequence: u64,
    pub entries: usize,
    pub receipts: usize,
}

fn rebuild(snap: &Snapshot) -> Result<Vec<u8>> {
    validate(snap)?;
    let mut records = snap.receipts.iter().collect::<Vec<_>>();
    records.sort_by_key(|r| r.result.sequence);
    let mut out = MAGIC.to_vec();
    let mut state = Replay::default();

    for receipt in records {
        if receipt.result.sequence != state.seq + 1 {
            return Err(Error::Invalid(
                "restore: missing or reordered historical sequence",
            ));
        }
        let operations: Vec<NativeMutation> = receipt
            .result
            .entries
            .iter()
            .map(|r| {
                Ok(NativeMutation {
                    namespace: r.namespace.clone(),
                    key: r.key.clone(),
                    expected_revision: r
                        .revision
                        .checked_sub(1)
                        .ok_or(Error::Invalid("restore: zero revision in receipt"))?,
                    change: r
                        .value
                        .clone()
                        .map_or(NativeChange::Delete, NativeChange::Put),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if batch_fingerprint(&operations)? != receipt.fingerprint {
            return Err(Error::Invalid(
                "restore: receipt fingerprint does not match operations",
            ));
        }
        let pending = prepare(&state, &receipt.request_key, &operations).map_err(|_| {
            Error::Invalid("restore: receipt violates original transaction history")
        })?;
        let generated = &pending.result;
        if generated != &receipt.result {
            return Err(Error::Invalid(
                "restore: reconstructed transaction differs from receipt",
            ));
        }
        let frame = Frame {
            seq: pending.result.sequence,
            request_key: receipt.request_key.clone(),
            operations,
        };
        let payload = serde_json::to_vec(&frame)
            .map_err(|_| Error::Invalid("restore: cannot serialize frame"))?;
        if payload.is_empty() || payload.len() > FRAME_LIMIT {
            return Err(Error::Invalid("restore: oversized frame"));
        }
        if (out.len() as u64).saturating_add(payload.len() as u64 + 36) > 64 * 1024 * 1024 {
            return Err(Error::Invalid("restore: reconstructed log exceeds 64 MiB"));
        }
        let len = (payload.len() as u32).to_be_bytes();
        let digest = frame_digest(&state.hash, &len, &payload);
        out.extend_from_slice(&len);
        out.extend_from_slice(&payload);
        out.extend_from_slice(&digest);
        apply(&mut state, pending, digest);
    }
    let entries = state.entries.into_values().collect::<Vec<_>>();
    if entries != snap.entries
        || state.seq != snap.sequence
        || head_hex(&state.hash) != snap.chain_head_sha256
        || out.len() as u64 != snap.log_bytes
        || format!("{:x}", Sha256::digest(&out)) != snap.log_sha256
    {
        return Err(Error::Invalid(
            "restore: reconstructed log differs from original source checksum/state",
        ));
    }
    Ok(out)
}

/// Create a new, full SEDRLOG1 after operator approval. The snapshot and any
/// existing journal remain byte-for-byte unchanged. Never automatic.
pub fn restore_to_new_file(
    source_snapshot: &Path,
    destination: &Path,
    approved_snapshot_sha256: &str,
) -> Result<RestoreReport> {
    #[cfg(not(unix))]
    return Err(Error::Invalid("restore: Unix-only experimental preview"));

    if !is_hash(approved_snapshot_sha256) {
        return Err(Error::Invalid(
            "restore: expected 64-digit snapshot SHA-256",
        ));
    }
    if source_snapshot == destination {
        return Err(Error::Invalid(
            "restore: snapshot and destination must differ",
        ));
    }
    private_parent(source_snapshot)?;
    private_file(source_snapshot)?;
    private_parent(destination)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(Error::Invalid("restore: destination exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(Error::Io(e)),
    }
    let mut original = File::open(source_snapshot)?;
    same_file(&original, source_snapshot)?;
    let size = original.metadata()?.len();
    if size > MAX_SNAPSHOT_BYTES {
        return Err(Error::Invalid("restore: snapshot exceeds 64 MiB"));
    }
    let mut bytes = Vec::new();
    (&mut original)
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(Error::Conflict(
            "restore: snapshot changed while reading".into(),
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != approved_snapshot_sha256.to_ascii_lowercase() {
        return Err(Error::Conflict(
            "restore: operator-approved snapshot SHA mismatch".into(),
        ));
    }
    let snapshot = parse_snapshot(&bytes)?;
    let snapshot_report = report(&snapshot, &bytes);
    let rebuilt = rebuild(&snapshot)?;
    original.seek(SeekFrom::Start(0))?;
    if file_sha256(&mut original)? != digest || original.metadata()?.len() != size {
        return Err(Error::Conflict(
            "restore: source snapshot changed before output".into(),
        ));
    }
    same_file(&original, source_snapshot)?;

    let restored = RestoreReport {
        snapshot_sha256: digest.clone(),
        recovered_log_sha256: snapshot.log_sha256.clone(),
        recovered_log_bytes: rebuilt.len() as u64,
        sequence: snapshot.sequence,
        entries: snapshot.entries.len(),
        receipts: snapshot.receipts.len(),
    };
    let mut opts = OpenOptions::new();
    opts.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut output = opts.open(destination)?;
    let outcome = (|| -> Result<()> {
        output.write_all(&rebuilt)?;
        output.sync_all()?;
        File::open(
            destination
                .parent()
                .ok_or(Error::Invalid("restore: missing parent"))?,
        )?
        .sync_all()?;
        private_file(destination)?;
        let replay = load(&mut output)?;
        if replay.seq != restored.sequence
            || output.metadata()?.len() != rebuilt.len() as u64
            || file_sha256(&mut output)? != restored.recovered_log_sha256
        {
            return Err(Error::Conflict(
                "restore: destination replay/hash mismatch".into(),
            ));
        }
        original.seek(SeekFrom::Start(0))?;
        if file_sha256(&mut original)? != digest || original.metadata()?.len() != size {
            return Err(Error::Conflict(
                "restore: snapshot changed during restore".into(),
            ));
        }
        same_file(&original, source_snapshot)?;
        let fresh = crate::snapshot::verify_snapshot(source_snapshot)?;
        if fresh != snapshot_report {
            return Err(Error::Conflict(
                "restore: snapshot changed after restore".into(),
            ));
        }
        Ok(())
    })();
    if let Err(err) = outcome {
        // Never unlink a file substituted by another process, or source.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(mine), Ok(current)) = (output.metadata(), fs::symlink_metadata(destination))
            {
                if current.is_file()
                    && !current.file_type().is_symlink()
                    && mine.ino() == current.ino()
                    && mine.dev() == current.dev()
                {
                    let _ = fs::remove_file(destination);
                }
            }
        }
        return Err(err);
    }
    Ok(restored)
}
