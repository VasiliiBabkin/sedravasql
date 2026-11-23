//! Operator-reviewed recovery by copying a *verified prefix* to a NEW file.
//!
//! No truncation, removal of stale locks, or in-place rewriting. A fully
//! present frame with invalid checksum/semantics is NEVER recoverable here.
//! An incomplete final frame is only a *candidate* for operator review.
//! The source remains unmodified regardless of the result.
use super::{
    fail_corrupt, frame_digest, load, private_file, private_parent, step, Error, Frame, Replay,
    Result, FRAME_LIMIT, MAGIC,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Experimental gate to avoid scanning unlimited files during recovery.
pub(crate) const MAX_REPAIR_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryPlan {
    pub source_sha256: String,
    pub source_bytes: u64,
    pub verified_prefix_bytes: u64,
    pub incomplete_tail_bytes: u64,
    pub recovered_sequence: u64,
}

fn lock_path(path: &Path) -> Result<PathBuf> {
    let file = path
        .file_name()
        .ok_or(Error::Invalid("recovery: expected a filename"))?;
    let mut name = file.to_os_string();
    name.push(".lock");
    Ok(path.with_file_name(name))
}
pub(crate) fn require_idle_source(path: &Path) -> Result<()> {
    private_parent(path)?;
    private_file(path)?;
    // The operator must first establish that the previous writer has
    // terminated and independently remove a stale lock when appropriate.
    let lock = lock_path(path)?;
    match fs::symlink_metadata(lock) {
        Ok(_) => Err(Error::Conflict(
            "recovery: source has a lock; verify old process stopped, do not automatically remove it".into(),
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

/// Verify the header, every complete frame and monotonic sequence.
fn scan(file: &mut File) -> Result<(Replay, u64)> {
    let bytes = file.metadata()?.len();
    if bytes > MAX_REPAIR_INPUT_BYTES {
        return Err(Error::Invalid(
            "recovery: log exceeds experimental scan budget",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header)
        .map_err(|_| fail_corrupt("recovery header"))?;
    if &header != MAGIC {
        return Err(fail_corrupt("recovery foreign format"));
    }
    let mut state = Replay::default();
    loop {
        let offset = file.stream_position()?;
        let available = bytes
            .checked_sub(offset)
            .ok_or_else(|| fail_corrupt("recovery source changed during scan"))?;
        if available == 0 {
            return Err(Error::Invalid(
                "recovery: file has no incomplete final frame",
            ));
        }
        if available < 4 {
            return Ok((state, offset));
        }
        let mut len = [0u8; 4];
        file.read_exact(&mut len)?;
        let size = u32::from_be_bytes(len) as usize;
        if size == 0 || size > FRAME_LIMIT {
            return Err(fail_corrupt("recovery invalid frame size"));
        }
        let full = 4 + size as u64 + 32;
        if available < full {
            // A probable torn final frame: no valid checksum is available.
            // Never auto-truncate even here. Operators must inspect plan.
            return Ok((state, offset));
        }
        let mut payload = vec![0u8; size];
        file.read_exact(&mut payload)?;
        let mut digest = [0u8; 32];
        file.read_exact(&mut digest)?;
        if frame_digest(&state.hash, &len, &payload) != digest {
            return Err(fail_corrupt(
                "recovery fully present frame checksum mismatch",
            ));
        }
        let frame: Frame = serde_json::from_slice(&payload)
            .map_err(|_| fail_corrupt("recovery invalid frame structure"))?;
        if frame.seq != state.seq + 1 {
            return Err(fail_corrupt("recovery invalid sequence"));
        }
        state = step(&state, &frame.request_key, &frame.operations)
            .map_err(|_| fail_corrupt("recovery invalid recorded mutation"))?;
        state.hash = digest;
    }
}

pub(crate) fn file_sha256(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Read-only assessment. Requires an idle, private, single-link source.
pub fn plan_recovery(path: &Path) -> Result<RecoveryPlan> {
    require_idle_source(path)?;
    let mut file = File::open(path)?;
    private_file(path)?;
    let (state, prefix) = scan(&mut file)?;
    let size = file.metadata()?.len();
    let digest = file_sha256(&mut file)?;
    require_idle_source(path)?;
    if file.metadata()?.len() != size {
        return Err(Error::Conflict(
            "recovery: source changed during inspection".into(),
        ));
    }
    let tail = size
        .checked_sub(prefix)
        .ok_or_else(|| fail_corrupt("recovery invalid prefix"))?;
    if tail == 0 {
        return Err(Error::Invalid("recovery: nothing to recover"));
    }
    Ok(RecoveryPlan {
        source_sha256: digest,
        source_bytes: size,
        verified_prefix_bytes: prefix,
        incomplete_tail_bytes: tail,
        recovered_sequence: state.seq,
    })
}

/// Explicitly copy only validated bytes to a NEW file. The source, including
/// its incomplete tail, stays byte-for-byte unchanged. Caller must supply the
/// full SHA-256 from an earlier plan, not an automatically generated value.
pub fn recover_to_new_file(
    source: &Path,
    destination: &Path,
    expected_source_sha256: &str,
) -> Result<RecoveryPlan> {
    if expected_source_sha256.len() != 64
        || !expected_source_sha256
            .bytes()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Err(Error::Invalid("recovery: expected 64-digit SHA-256"));
    }
    if source == destination {
        return Err(Error::Invalid(
            "recovery: source and destination must differ",
        ));
    }
    require_idle_source(source)?;
    private_parent(destination)?;
    // Refuse all preexisting destinations, including dangling symlinks.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(Error::Invalid("recovery: destination exists"));
    }
    let planned = plan_recovery(source)?;
    if planned.source_sha256 != expected_source_sha256.to_ascii_lowercase() {
        return Err(Error::Conflict(
            "recovery: source hash no longer matches approved plan".into(),
        ));
    }
    let mut original = File::open(source)?;
    private_file(source)?;
    if original.metadata()?.len() != planned.source_bytes
        || file_sha256(&mut original)? != planned.source_sha256
    {
        return Err(Error::Conflict(
            "recovery: original changed before copying".into(),
        ));
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut output = opts.open(destination)?;
    let outcome = (|| -> Result<()> {
        original.seek(SeekFrom::Start(0))?;
        let copied = std::io::copy(
            &mut Read::by_ref(&mut original).take(planned.verified_prefix_bytes),
            &mut output,
        )?;
        if copied != planned.verified_prefix_bytes {
            return Err(Error::Conflict(
                "recovery: source shortened during copy".into(),
            ));
        }
        output.sync_all()?;
        File::open(
            destination
                .parent()
                .ok_or(Error::Invalid("recovery: missing parent"))?,
        )?
        .sync_all()?;
        private_file(destination)?;
        let restored = load(&mut output)?;
        if restored.seq != planned.recovered_sequence
            || output.metadata()?.len() != planned.verified_prefix_bytes
        {
            return Err(fail_corrupt("recovery copy did not verify"));
        }
        require_idle_source(source)?;
        if original.metadata()?.len() != planned.source_bytes
            || file_sha256(&mut original)? != planned.source_sha256
        {
            return Err(Error::Conflict(
                "recovery: source changed while copying".into(),
            ));
        }
        Ok(())
    })();
    if let Err(err) = outcome {
        // Recovery failed. Never delete or change the original. Only remove
        // the new destination if its identity matches our still-open handle.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (output.metadata(), fs::symlink_metadata(destination)) {
                if b.is_file()
                    && !b.file_type().is_symlink()
                    && a.dev() == b.dev()
                    && a.ino() == b.ino()
                {
                    let _ = fs::remove_file(destination);
                }
            }
        }
        return Err(err);
    }
    Ok(planned)
}
