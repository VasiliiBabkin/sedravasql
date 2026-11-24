//! B15b: lossless offline archive of an entire verified SEDRLOG1.
//! Archive is not a live database, and does not delete or rotate the source.
use super::{load, private_file, private_parent, Error, ExclusiveLock, Result, MAX_LOG_BYTES};
use crate::backup::same_file;
use crate::recovery::file_sha256;
use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SEDRARC1";
const HEADER: usize = 8 + 8 + 8 + 32;
const MAX_ARCHIVE: u64 = MAX_LOG_BYTES + 1024 * 1024;
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchiveReport {
    pub source_sha256: String,
    pub source_bytes: u64,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub sequence: u64,
}

fn hex(hash: &[u8]) -> String {
    hash.iter().map(|v| format!("{v:02x}")).collect()
}
fn approved(s: &str) -> Result<()> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid("archive: expected lowercase SHA-256"));
    }
    Ok(())
}
fn read_bounded(file: &mut File, max: u64) -> Result<Vec<u8>> {
    if file.metadata()?.len() > max {
        return Err(Error::Capacity("archive: input size limit"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(Error::Capacity("archive: input size limit"));
    }
    Ok(bytes)
}
fn encode(raw: &[u8], sequence: u64) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw)?;
    let compressed = encoder.finish()?;
    let mut out = Vec::with_capacity(HEADER + compressed.len() + 32);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(raw.len() as u64).to_be_bytes());
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(&Sha256::digest(raw));
    out.extend_from_slice(&compressed);
    out.extend_from_slice(&Sha256::digest(&out));
    if out.len() as u64 > MAX_ARCHIVE {
        return Err(Error::Capacity("archive: output size limit"));
    }
    Ok(out)
}
fn decode(bytes: &[u8]) -> Result<(ArchiveReport, Vec<u8>)> {
    if bytes.len() < HEADER + 32 || bytes.len() as u64 > MAX_ARCHIVE || &bytes[..8] != MAGIC {
        return Err(Error::Invalid("archive: foreign or truncated format"));
    }
    let end = bytes.len() - 32;
    if Sha256::digest(&bytes[..end]).as_slice() != &bytes[end..] {
        return Err(Error::Invalid("archive: checksum mismatch"));
    }
    let size = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let seq = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    if !(8..=MAX_LOG_BYTES).contains(&size) {
        return Err(Error::Capacity("archive: expanded size limit"));
    }
    let mut decoder = Decompress::new(true);
    let mut raw = vec![0u8; size as usize + 1];
    let status = decoder
        .decompress(&bytes[HEADER..end], &mut raw, FlushDecompress::Finish)
        .map_err(|_| Error::Invalid("archive: compressed stream invalid"))?;
    if status != Status::StreamEnd
        || decoder.total_out() != size
        || decoder.total_in() != (end - HEADER) as u64
    {
        return Err(Error::Invalid(
            "archive: incomplete/trailing stream or expanded length mismatch",
        ));
    }
    raw.truncate(size as usize);
    if Sha256::digest(&raw).as_slice() != &bytes[24..56] {
        return Err(Error::Invalid("archive: expanded checksum mismatch"));
    }
    // Independently validate every journal frame and transaction, not just the archive hash.
    let state = super::load_reader(&mut Cursor::new(&raw))?;
    if state.seq != seq {
        return Err(Error::Invalid("archive: sequence mismatch"));
    }
    Ok((
        ArchiveReport {
            source_sha256: hex(&bytes[24..56]),
            source_bytes: size,
            archive_sha256: hex(&Sha256::digest(bytes)),
            archive_bytes: bytes.len() as u64,
            sequence: seq,
        },
        raw,
    ))
}
fn create_new(
    path: &Path,
    bytes: &[u8],
    check: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    private_parent(path)?;
    // Acquire destination lock before create_new. Cooperating NativeLog cannot open
    // a partly written output. Never remove an existing destination.
    let _lock = ExclusiveLock::acquire(path)?;
    let mut opts = OpenOptions::new();
    opts.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        File::open(
            path.parent()
                .ok_or(Error::Invalid("archive: parent missing"))?,
        )?
        .sync_all()?;
        same_file(&file, path)?;
        private_file(path)?;
        check(&mut file)
    })();
    if let Err(e) = result {
        if same_file(&file, path).is_ok() {
            let _ = fs::remove_file(path);
        }
        return Err(e);
    }
    Ok(())
}
pub fn create_archive(source: &Path, destination: &Path, expected: &str) -> Result<ArchiveReport> {
    approved(expected)?;
    if source == destination {
        return Err(Error::Invalid("archive: identical paths"));
    }
    private_parent(source)?;
    private_file(source)?;
    let _lock = ExclusiveLock::acquire(source)?;
    let mut original = File::open(source)?;
    same_file(&original, source)?;
    let raw = read_bounded(&mut original, MAX_LOG_BYTES)?;
    let state = super::load_reader(&mut Cursor::new(&raw))?;
    let source_hash = hex(&Sha256::digest(&raw));
    if source_hash != expected {
        return Err(Error::Conflict("archive: source SHA mismatch".into()));
    }
    let archive = encode(&raw, state.seq)?;
    let (report, decoded) = decode(&archive)?;
    if decoded != raw {
        return Err(Error::Invalid("archive: roundtrip mismatch"));
    }
    same_file(&original, source)?;
    if file_sha256(&mut original)? != expected {
        return Err(Error::Conflict("archive: source changed".into()));
    }
    create_new(destination, &archive, |f| {
        let actual = read_bounded(f, MAX_ARCHIVE)?;
        if decode(&actual)?.0 != report {
            return Err(Error::Invalid("archive: written output mismatch"));
        }
        same_file(&original, source)?;
        if file_sha256(&mut original)? != expected {
            return Err(Error::Conflict("archive: source changed".into()));
        }
        Ok(())
    })?;
    Ok(report)
}
pub fn verify_archive(path: &Path) -> Result<ArchiveReport> {
    private_parent(path)?;
    private_file(path)?;
    let _lock = ExclusiveLock::acquire(path)?;
    let mut f = File::open(path)?;
    same_file(&f, path)?;
    let (report, _) = decode(&read_bounded(&mut f, MAX_ARCHIVE)?)?;
    same_file(&f, path)?;
    if file_sha256(&mut f)? != report.archive_sha256 {
        return Err(Error::Conflict("archive: changed".into()));
    }
    Ok(report)
}
pub fn restore_archive(source: &Path, destination: &Path, expected: &str) -> Result<ArchiveReport> {
    approved(expected)?;
    if source == destination {
        return Err(Error::Invalid("archive: identical paths"));
    }
    private_parent(source)?;
    private_file(source)?;
    let _lock = ExclusiveLock::acquire(source)?;
    let mut original = File::open(source)?;
    same_file(&original, source)?;
    let (report, raw) = decode(&read_bounded(&mut original, MAX_ARCHIVE)?)?;
    if report.archive_sha256 != expected {
        return Err(Error::Conflict("archive: approved SHA mismatch".into()));
    }
    create_new(destination, &raw, |f| {
        if load(f)?.seq != report.sequence || file_sha256(f)? != report.source_sha256 {
            return Err(Error::Invalid("archive: restored output mismatch"));
        }
        same_file(&original, source)?;
        if file_sha256(&mut original)? != expected {
            return Err(Error::Conflict("archive: source changed".into()));
        }
        Ok(())
    })?;
    Ok(report)
}
