//! Verified offline backup for a COMPLETE and idle native log.
//!
//! B2 research only: no live backup, truncation, compaction, source editing
//! or automatic stale-lock handling. A pre-approved hash is mandatory.
use super::{fail_corrupt, load, private_file, private_parent, Error, Result};
use crate::recovery::{file_sha256, require_idle_source, MAX_REPAIR_INPUT_BYTES};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct BackupPlan {
    pub source_sha256: String,
    pub source_bytes: u64,
    pub sequence: u64,
}

#[cfg(unix)]
pub(crate) fn same_file(handle: &File, path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let a = handle.metadata()?;
    let b = fs::symlink_metadata(path)?;
    if !b.is_file() || b.file_type().is_symlink() || a.dev() != b.dev() || a.ino() != b.ino() {
        return Err(Error::Conflict("backup: source identity changed".into()));
    }
    Ok(())
}
#[cfg(not(unix))]
pub(crate) fn same_file(_handle: &File, _path: &Path) -> Result<()> {
    Err(Error::Invalid("backup: supported only on Unix preview"))
}

fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Read-only, bounded plan for a fully valid and idle native log.
pub fn plan_backup(source: &Path) -> Result<BackupPlan> {
    require_idle_source(source)?;
    let mut file = File::open(source)?;
    same_file(&file, source)?;
    let source_bytes = file.metadata()?.len();
    if source_bytes > MAX_REPAIR_INPUT_BYTES {
        return Err(Error::Invalid(
            "backup: 64 MiB experimental file budget exceeded",
        ));
    }
    let replay = load(&mut file)?; // strict full-frame integrity; never accept torn tail
    let digest = file_sha256(&mut file)?;
    require_idle_source(source)?;
    same_file(&file, source)?;
    if file.metadata()?.len() != source_bytes {
        return Err(Error::Conflict("backup: source size changed".into()));
    }
    // Re-reading this digest detects modification even with an unchanged length;
    // this is not a defense against a malicious concurrent same-UID writer.
    if file_sha256(&mut file)? != digest {
        return Err(Error::Conflict(
            "backup: source changed during inspection".into(),
        ));
    }
    Ok(BackupPlan {
        source_sha256: digest,
        source_bytes,
        sequence: replay.seq,
    })
}

/// Copy validated, unchanged source to NEW destination after explicit SHA approval.
pub fn backup_to_new_file(
    source: &Path,
    destination: &Path,
    expected_source_sha256: &str,
) -> Result<BackupPlan> {
    if !valid_sha(expected_source_sha256) {
        return Err(Error::Invalid("backup: expected full 64-digit SHA-256"));
    }
    if source == destination {
        return Err(Error::Invalid("backup: source and destination must differ"));
    }
    require_idle_source(source)?;
    private_parent(destination)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(Error::Invalid("backup: destination already exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(Error::Io(e)),
    }
    let plan = plan_backup(source)?;
    if plan.source_sha256 != expected_source_sha256.to_ascii_lowercase() {
        return Err(Error::Conflict(
            "backup: source hash differs from operator-approved plan".into(),
        ));
    }
    let mut original = File::open(source)?;
    same_file(&original, source)?;
    if original.metadata()?.len() != plan.source_bytes
        || file_sha256(&mut original)? != plan.source_sha256
    {
        return Err(Error::Conflict(
            "backup: source changed before copying".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination)?;
    let outcome = (|| -> Result<()> {
        original.seek(SeekFrom::Start(0))?;
        let copied = std::io::copy(&mut original, &mut output)?;
        if copied != plan.source_bytes {
            return Err(Error::Conflict(
                "backup: source size changed during copying".into(),
            ));
        }
        output.sync_all()?;
        File::open(
            destination
                .parent()
                .ok_or(Error::Invalid("backup: parent missing"))?,
        )?
        .sync_all()?;
        private_file(destination)?;
        let replay = load(&mut output)?;
        if replay.seq != plan.sequence
            || output.metadata()?.len() != plan.source_bytes
            || file_sha256(&mut output)? != plan.source_sha256
        {
            return Err(fail_corrupt("backup copy failed integrity check"));
        }
        same_file(&original, source)?;
        let after = plan_backup(source)?;
        if after.source_sha256 != plan.source_sha256 || after.sequence != plan.sequence {
            return Err(Error::Conflict(
                "backup: source changed during copying".into(),
            ));
        }
        Ok(())
    })();
    if let Err(e) = outcome {
        // Clean up only our own newly-created file, never an unrelated replacement.
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
        return Err(e);
    }
    Ok(plan)
}
