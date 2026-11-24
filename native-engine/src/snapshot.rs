//! B3: offline, read-only materialized snapshot candidate, with NO restore.
//!
//! This is deliberately a different magic/file format from SEDRLOG1.
//! It cannot be opened as a writable native log, and never truncates or
//! compacts the source. Requires a stopped writer and operator-approved hash.
use super::{
    load, private_file, private_parent, Error, NativeCommit, NativeEntry, Result, MAX_NATIVE_BATCH,
};
use crate::backup::{plan_backup, same_file};
use crate::recovery::{file_sha256, require_idle_source, MAX_REPAIR_INPUT_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const SNAPSHOT_MAGIC: &[u8; 8] = b"SEDRSNP1";
const SNAPSHOT_SCHEMA: &str = "sedravasql.snapshot.b3-preview.v1";
pub(crate) const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) request_key: String,
    pub(crate) fingerprint: String,
    pub(crate) result: NativeCommit,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub(crate) schema: String,
    pub(crate) log_sha256: String,
    pub(crate) log_bytes: u64,
    pub(crate) sequence: u64,
    pub(crate) chain_head_sha256: String,
    pub(crate) entries: Vec<NativeEntry>,
    pub(crate) receipts: Vec<Receipt>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SnapshotReport {
    pub schema: String,
    pub source_sha256: String,
    pub source_bytes: u64,
    pub sequence: u64,
    pub entries: usize,
    pub receipts: usize,
    pub snapshot_sha256: String,
    pub snapshot_bytes: u64,
}

fn hash_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn head_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
pub(crate) fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(crate) fn validate(snap: &Snapshot) -> Result<()> {
    if snap.schema != SNAPSHOT_SCHEMA
        || !is_hash(&snap.log_sha256)
        || !is_hash(&snap.chain_head_sha256)
        || snap.log_bytes > MAX_REPAIR_INPUT_BYTES
        || snap.sequence < snap.receipts.len() as u64
        || snap.sequence != snap.receipts.len() as u64
    {
        return Err(Error::Invalid("snapshot: inconsistent metadata"));
    }
    if !snap
        .entries
        .windows(2)
        .all(|pair| (&pair[0].namespace, &pair[0].key) < (&pair[1].namespace, &pair[1].key))
    {
        return Err(Error::Invalid("snapshot: entry keys are not sorted/unique"));
    }
    if snap.entries.iter().any(|item| {
        item.revision == 0
            || item.revision > snap.sequence
            || item.namespace.is_empty()
            || item.key.is_empty()
    }) {
        return Err(Error::Invalid("snapshot: invalid entry"));
    }
    if !snap
        .receipts
        .windows(2)
        .all(|pair| pair[0].request_key < pair[1].request_key)
    {
        return Err(Error::Invalid(
            "snapshot: request keys are not sorted/unique",
        ));
    }
    let mut sequence_ids = HashSet::new();
    for item in &snap.receipts {
        if !is_hash(&item.fingerprint)
            || item.result.sequence == 0
            || item.result.sequence > snap.sequence
            || !sequence_ids.insert(item.result.sequence)
            || item.result.replayed
            || item.result.entries.is_empty()
            || item.result.entries.len() > MAX_NATIVE_BATCH
        {
            return Err(Error::Invalid("snapshot: invalid stored receipt"));
        }
    }
    Ok(())
}
pub(crate) fn serialize_snapshot(snapshot: &Snapshot) -> Result<Vec<u8>> {
    validate(snapshot)?;
    let payload = serde_json::to_vec(snapshot)
        .map_err(|_| Error::Invalid("snapshot: cannot encode state"))?;
    if payload.len() > MAX_SNAPSHOT_BYTES as usize - 44 {
        return Err(Error::Invalid("snapshot: state exceeds 64 MiB budget"));
    }
    let mut out = Vec::with_capacity(payload.len() + 44);
    out.extend_from_slice(SNAPSHOT_MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&Sha256::digest(&out));
    Ok(out)
}
pub(crate) fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot> {
    if bytes.len() < 44 || &bytes[..8] != SNAPSHOT_MAGIC {
        return Err(Error::Invalid("snapshot: foreign or truncated format"));
    }
    let size = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if size > MAX_SNAPSHOT_BYTES as usize - 44 || size + 44 != bytes.len() {
        return Err(Error::Invalid(
            "snapshot: invalid frame size/trailing bytes",
        ));
    }
    if Sha256::digest(&bytes[..12 + size]).as_slice() != &bytes[12 + size..] {
        return Err(Error::Invalid("snapshot: checksum mismatch"));
    }
    let snap: Snapshot = serde_json::from_slice(&bytes[12..12 + size])
        .map_err(|_| Error::Invalid("snapshot: invalid JSON body"))?;
    validate(&snap)?;
    Ok(snap)
}
pub(crate) fn report(snap: &Snapshot, bytes: &[u8]) -> SnapshotReport {
    SnapshotReport {
        schema: snap.schema.clone(),
        source_sha256: snap.log_sha256.clone(),
        source_bytes: snap.log_bytes,
        sequence: snap.sequence,
        entries: snap.entries.len(),
        receipts: snap.receipts.len(),
        snapshot_sha256: hash_hex(bytes),
        snapshot_bytes: bytes.len() as u64,
    }
}

/// Read-only inspection of an isolated snapshot. It does not open a database.
pub fn verify_snapshot(path: &Path) -> Result<SnapshotReport> {
    private_parent(path)?;
    private_file(path)?;
    let mut file = File::open(path)?;
    same_file(&file, path)?;
    if file.metadata()?.len() > MAX_SNAPSHOT_BYTES {
        return Err(Error::Invalid("snapshot: larger than 64 MiB"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(Error::Invalid("snapshot: too large"));
    }
    let snapshot = parse_snapshot(&bytes)?;
    same_file(&file, path)?;
    if file.metadata()?.len() != bytes.len() as u64 {
        return Err(Error::Conflict(
            "snapshot: file changed during verification".into(),
        ));
    }
    // Only accidental corruption protection; same-UID malicious replacement
    // and carefully recomputed checksums are explicitly out of scope.
    file.seek(SeekFrom::Start(0))?;
    let digest = file_sha256(&mut file)?;
    if digest != hash_hex(&bytes) {
        return Err(Error::Conflict(
            "snapshot: changed during verification".into(),
        ));
    }
    Ok(report(&snapshot, &bytes))
}

/// Canonical snapshot from an already verified exclusive log handle.
pub(crate) fn from_log(log: &mut crate::NativeLog) -> Result<Snapshot> {
    log.verify()?;
    let at = log.file.stream_position()?;
    let digest = file_sha256(&mut log.file);
    log.file.seek(SeekFrom::Start(at))?;
    let mut receipts = log
        .state
        .receipts
        .iter()
        .map(|(key, (fingerprint, result))| Receipt {
            request_key: key.clone(),
            fingerprint: fingerprint.clone(),
            result: result.clone(),
        })
        .collect::<Vec<_>>();
    receipts.sort_by(|a, b| a.request_key.cmp(&b.request_key));
    let snap = Snapshot {
        schema: SNAPSHOT_SCHEMA.into(),
        log_sha256: digest?,
        log_bytes: log.log_bytes,
        sequence: log.state.seq,
        chain_head_sha256: head_hex(&log.state.hash),
        entries: log.state.entries.values().cloned().collect(),
        receipts,
    };
    validate(&snap)?;
    Ok(snap)
}

/// Materialize sorted entries + original receipt history from a COMPLETE,
/// idle and previously approved source into a distinct, new file.
/// Does NOT enable restoring/compacting the writable log.
pub fn create_snapshot(
    source: &Path,
    destination: &Path,
    expected_source_sha256: &str,
) -> Result<SnapshotReport> {
    if !is_hash(expected_source_sha256) {
        return Err(Error::Invalid("snapshot: expected 64-digit source SHA-256"));
    }
    if source == destination {
        return Err(Error::Invalid("snapshot: source and destination identical"));
    }
    require_idle_source(source)?;
    private_parent(destination)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(Error::Invalid("snapshot: destination exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(Error::Io(e)),
    }
    let plan = plan_backup(source)?;
    if plan.source_sha256 != expected_source_sha256.to_ascii_lowercase() {
        return Err(Error::Conflict(
            "snapshot: approved source SHA mismatch".into(),
        ));
    }
    let mut original = File::open(source)?;
    same_file(&original, source)?;
    let log = load(&mut original)?;
    if log.seq != plan.sequence
        || original.metadata()?.len() != plan.source_bytes
        || file_sha256(&mut original)? != plan.source_sha256
    {
        return Err(Error::Conflict(
            "snapshot: source changed before serialization".into(),
        ));
    }
    let entries = log.entries.into_values().collect();
    let mut receipts = log
        .receipts
        .into_iter()
        .map(|(request_key, (fingerprint, result))| Receipt {
            request_key,
            fingerprint,
            result,
        })
        .collect::<Vec<_>>();
    receipts.sort_by(|a, b| a.request_key.cmp(&b.request_key));
    let snapshot = Snapshot {
        schema: SNAPSHOT_SCHEMA.into(),
        log_sha256: plan.source_sha256.clone(),
        log_bytes: plan.source_bytes,
        sequence: plan.sequence,
        chain_head_sha256: head_hex(&log.hash),
        entries,
        receipts,
    };
    let contents = serialize_snapshot(&snapshot)?;
    let report = report(&snapshot, &contents);
    // Re-check before creating the destination; never change the original.
    let still = plan_backup(source)?;
    if still.source_sha256 != plan.source_sha256 || still.sequence != plan.sequence {
        return Err(Error::Conflict(
            "snapshot: source changed before output".into(),
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
        output.write_all(&contents)?;
        output.sync_all()?;
        File::open(
            destination
                .parent()
                .ok_or(Error::Invalid("snapshot: no parent"))?,
        )?
        .sync_all()?;
        private_file(destination)?;
        let on_disk = verify_snapshot(destination)?;
        if on_disk != report {
            return Err(Error::Conflict(
                "snapshot: output verification mismatch".into(),
            ));
        }
        same_file(&original, source)?;
        let after = plan_backup(source)?;
        if after.source_sha256 != plan.source_sha256 || after.sequence != plan.sequence {
            return Err(Error::Conflict(
                "snapshot: source changed during snapshot".into(),
            ));
        }
        Ok(())
    })();
    if let Err(err) = outcome {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(mine), Ok(current)) = (output.metadata(), fs::symlink_metadata(destination))
            {
                if current.is_file()
                    && !current.file_type().is_symlink()
                    && mine.dev() == current.dev()
                    && mine.ino() == current.ino()
                {
                    let _ = fs::remove_file(destination);
                }
            }
        }
        return Err(err);
    }
    Ok(report)
}
