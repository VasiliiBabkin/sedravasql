//! B8: verified immutable sorted *offline* point-read sidecar from SEDRSNP1.
//! Does not modify/rewrite logs, does not become a live journal index, and
//! cannot restore receipts or serve as a writable database.
use crate::backup::same_file;
use crate::recovery::file_sha256;
use crate::snapshot::{is_hash, parse_snapshot, verify_snapshot, MAX_SNAPSHOT_BYTES};
use crate::{private_file, private_parent, valid_id, Error, NativeEntry, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SEDRIDX1";
const SCHEMA: &str = "sedravasql.index.b8-offline.v1";
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DirectoryEntry {
    namespace: String,
    key: String,
    revision: u64,
    offset: u64,
    bytes: u32,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Directory {
    schema: String,
    snapshot_sha256: String,
    log_sha256: String,
    sequence: u64,
    rows: Vec<DirectoryEntry>,
    data_bytes: u64,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct IndexReport {
    pub schema: String,
    pub snapshot_sha256: String,
    pub log_sha256: String,
    pub sequence: u64,
    pub entries: usize,
    pub index_sha256: String,
    pub index_bytes: u64,
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest_ok(value: &str) -> bool {
    is_hash(value) && value.bytes().all(|b| !b.is_ascii_uppercase())
}

fn validate_dir(dir: &Directory) -> Result<()> {
    if dir.schema != SCHEMA
        || !digest_ok(&dir.snapshot_sha256)
        || !digest_ok(&dir.log_sha256)
        || dir.data_bytes > MAX_INDEX_BYTES
        || dir.rows.len() > 1_000_000
    {
        return Err(Error::Invalid("index: invalid header"));
    }
    let mut at = 0u64;
    let mut previous: Option<(&str, &str)> = None;
    for row in &dir.rows {
        let name = (row.namespace.as_str(), row.key.as_str());
        if previous.is_some_and(|p| p >= name)
            || row.namespace.is_empty()
            || row.key.is_empty()
            || row.revision == 0
            || row.revision > dir.sequence
            || row.offset != at
            || row.bytes == 0
            || !digest_ok(&row.sha256)
        {
            return Err(Error::Invalid(
                "index: unsorted or inconsistent row directory",
            ));
        }
        at = at
            .checked_add(u64::from(row.bytes))
            .ok_or(Error::Invalid("index: overflow"))?;
        previous = Some(name);
    }
    if at != dir.data_bytes {
        return Err(Error::Invalid("index: wrong data byte count"));
    }
    Ok(())
}
fn decode(bytes: &[u8]) -> Result<(Directory, Vec<NativeEntry>)> {
    if bytes.len() < 48 || &bytes[..8] != MAGIC {
        return Err(Error::Invalid("index: foreign or truncated format"));
    }
    let dlen = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let blen = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if dlen == 0
        || dlen as u64 > MAX_INDEX_BYTES
        || blen as u64 > MAX_INDEX_BYTES
        || dlen.checked_add(blen).and_then(|n| n.checked_add(48)) != Some(bytes.len())
    {
        return Err(Error::Invalid("index: wrong framing or trailing bytes"));
    }
    if Sha256::digest(&bytes[..bytes.len() - 32]).as_slice() != &bytes[bytes.len() - 32..] {
        return Err(Error::Invalid("index: checksum mismatch"));
    }
    let dir: Directory = serde_json::from_slice(&bytes[16..16 + dlen])
        .map_err(|_| Error::Invalid("index: invalid directory JSON"))?;
    validate_dir(&dir)?;
    if dir.data_bytes != blen as u64 {
        return Err(Error::Invalid("index: truncated entry section"));
    }
    let data = &bytes[16 + dlen..16 + dlen + blen];
    let mut entries = Vec::with_capacity(dir.rows.len());
    for descriptor in &dir.rows {
        let start = usize::try_from(descriptor.offset)
            .map_err(|_| Error::Invalid("index: offset too large"))?;
        let end = start
            .checked_add(descriptor.bytes as usize)
            .ok_or(Error::Invalid("index: row length overflow"))?;
        let item = data
            .get(start..end)
            .ok_or(Error::Invalid("index: row outside data"))?;
        if sha(item) != descriptor.sha256 {
            return Err(Error::Invalid("index: row digest mismatch"));
        }
        let entry: NativeEntry = serde_json::from_slice(item)
            .map_err(|_| Error::Invalid("index: invalid stored entry"))?;
        if entry.namespace != descriptor.namespace
            || entry.key != descriptor.key
            || entry.revision != descriptor.revision
        {
            return Err(Error::Invalid("index: row key/revision mismatch"));
        }
        entries.push(entry);
    }
    Ok((dir, entries))
}
fn report(dir: &Directory, bytes: &[u8]) -> IndexReport {
    IndexReport {
        schema: dir.schema.clone(),
        snapshot_sha256: dir.snapshot_sha256.clone(),
        log_sha256: dir.log_sha256.clone(),
        sequence: dir.sequence,
        entries: dir.rows.len(),
        index_sha256: sha(bytes),
        index_bytes: bytes.len() as u64,
    }
}
fn read_index(path: &Path) -> Result<(Directory, Vec<NativeEntry>, IndexReport)> {
    private_parent(path)?;
    private_file(path)?;
    let mut file = File::open(path)?;
    same_file(&file, path)?;
    if file.metadata()?.len() > MAX_INDEX_BYTES {
        return Err(Error::Invalid("index: greater than 64 MiB"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(Error::Invalid("index: input too large"));
    }
    let (dir, entries) = decode(&bytes)?;
    same_file(&file, path)?;
    if file.metadata()?.len() != bytes.len() as u64 {
        return Err(Error::Conflict("index: changed during verification".into()));
    }
    file.seek(SeekFrom::Start(0))?;
    if file_sha256(&mut file)? != sha(&bytes) {
        return Err(Error::Conflict("index: changed during verification".into()));
    }
    let status = report(&dir, &bytes);
    Ok((dir, entries, status))
}
/// B9: one fully verified immutable-index file handle, with direct
/// per-key seek/read after opening. The original SEDRIDX1 format remains
/// unchanged. This is an OFFLINE snapshot index, never a live journal.
pub struct VerifiedIndexReader {
    file: File,
    path: std::path::PathBuf,
    rows: Vec<DirectoryEntry>,
    data_at: u64,
    report: IndexReport,
    #[cfg(unix)]
    identity: IndexIdentity,
    row_bytes_read: u64,
    row_seeks: u64,
}
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct IndexIdentity {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}
#[cfg(unix)]
fn identity(meta: &fs::Metadata) -> IndexIdentity {
    use std::os::unix::fs::MetadataExt;
    IndexIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
        ctime: meta.ctime(),
        ctime_nsec: meta.ctime_nsec(),
    }
}
impl VerifiedIndexReader {
    /// Full validation is done ONCE on opening, not per item lookup.
    pub fn open(path: &Path) -> Result<Self> {
        private_parent(path)?;
        private_file(path)?;
        let (dir, _, report) = read_index(path)?;
        let mut file = File::open(path)?;
        same_file(&file, path)?;
        // Recheck the bytes of the just-opened handle to prevent a
        // check/open replacement race between verification and reading.
        if file.metadata()?.len() != report.index_bytes
            || file_sha256(&mut file)? != report.index_sha256
        {
            return Err(Error::Conflict("index: replaced during open".into()));
        }
        file.seek(SeekFrom::Start(8))?;
        let mut lengths = [0u8; 8];
        file.read_exact(&mut lengths)?;
        let dir_len = u32::from_be_bytes(lengths[..4].try_into().unwrap()) as u64;
        let dat_len = u32::from_be_bytes(lengths[4..].try_into().unwrap()) as u64;
        if 48u64
            .checked_add(dir_len)
            .and_then(|x| x.checked_add(dat_len))
            != Some(report.index_bytes)
            || dat_len != dir.data_bytes
        {
            return Err(Error::Conflict(
                "index: changed framing after verification".into(),
            ));
        }
        let data_at = 16u64
            .checked_add(dir_len)
            .ok_or(Error::Invalid("index: directory size overflow"))?;
        let meta = file.metadata()?;
        Ok(Self {
            file,
            path: path.to_owned(),
            rows: dir.rows,
            data_at,
            report,
            #[cfg(unix)]
            identity: identity(&meta),
            row_bytes_read: 0,
            row_seeks: 0,
        })
    }
    pub fn report(&self) -> &IndexReport {
        &self.report
    }
    /// Bytes read from the ROW DATA section since session open. Excludes
    /// initial full verification and small per-read metadata checks.
    pub fn row_bytes_read(&self) -> u64 {
        self.row_bytes_read
    }
    /// Explicit seeks in the row-data section after open; not OS read calls.
    pub fn row_seeks(&self) -> u64 {
        self.row_seeks
    }
    /// Half-open key range, including tombstones. One initial data seek,
    /// then sequential verified reads, O(log N + K) directory operations.
    pub fn range(
        &mut self,
        namespace: &str,
        start: &str,
        end: &str,
        limit: usize,
    ) -> Result<Vec<NativeEntry>> {
        if !valid_id(namespace)
            || !valid_id(start)
            || !valid_id(end)
            || start >= end
            || !(1..=256).contains(&limit)
        {
            return Err(Error::Invalid("index: invalid range"));
        }
        self.check_identity()?;
        let first = self
            .rows
            .partition_point(|row| (row.namespace.as_str(), row.key.as_str()) < (namespace, start));
        let last = self
            .rows
            .partition_point(|row| (row.namespace.as_str(), row.key.as_str()) < (namespace, end));
        let stop = last.min(first.saturating_add(limit));
        let mut result = Vec::with_capacity(stop - first);
        if first < stop {
            self.seek_row(first)?;
            for i in first..stop {
                self.check_identity()?;
                result.push(self.read_current_row(i)?);
                self.check_identity()?;
            }
        }
        self.check_identity()?;
        Ok(result)
    }
    fn check_identity(&self) -> Result<()> {
        private_parent(&self.path)?;
        private_file(&self.path)?;
        same_file(&self.file, &self.path)?;
        #[cfg(unix)]
        if identity(&self.file.metadata()?) != self.identity {
            return Err(Error::Conflict("index: file changed after open".into()));
        }
        Ok(())
    }
    pub fn get(&mut self, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
        if !valid_id(namespace) || !valid_id(key) {
            return Err(Error::Invalid("index: invalid key lookup"));
        }
        self.check_identity()?;
        let found = self
            .rows
            .binary_search_by(|e| (e.namespace.as_str(), e.key.as_str()).cmp(&(namespace, key)));
        let Ok(index) = found else {
            self.check_identity()?;
            return Ok(None);
        };
        self.seek_row(index)?;
        let record = self.read_current_row(index)?;
        self.check_identity()?;
        Ok(Some(record))
    }
    fn seek_row(&mut self, index: usize) -> Result<()> {
        let absolute = self
            .data_at
            .checked_add(self.rows[index].offset)
            .ok_or(Error::Invalid("index: absolute offset overflow"))?;
        self.file.seek(SeekFrom::Start(absolute))?;
        self.row_seeks = self
            .row_seeks
            .checked_add(1)
            .ok_or(Error::Invalid("index: seek counter overflow"))?;
        Ok(())
    }
    // Caller positions the handle at the selected row; contiguous offsets
    // were verified on open. Exact byte count advances to the next row.
    fn read_current_row(&mut self, index: usize) -> Result<NativeEntry> {
        let entry = &self.rows[index];
        let mut content = vec![0u8; entry.bytes as usize];
        self.file.read_exact(&mut content)?;
        self.row_bytes_read = self
            .row_bytes_read
            .checked_add(content.len() as u64)
            .ok_or(Error::Invalid("index: read counter overflow"))?;
        if sha(&content) != entry.sha256 {
            return Err(Error::Conflict("index: indexed row corrupted".into()));
        }
        let record: NativeEntry = serde_json::from_slice(&content)
            .map_err(|_| Error::Invalid("index: invalid indexed row"))?;
        if record.namespace != entry.namespace
            || record.key != entry.key
            || record.revision != entry.revision
        {
            return Err(Error::Conflict("index: indexed row mismatch".into()));
        }
        Ok(record)
    }
}

pub fn verify_index(path: &Path) -> Result<IndexReport> {
    let (_, _, status) = read_index(path)?;
    Ok(status)
}
/// Strict lookup: verifies the full index every time, THEN binary-searches
/// its sorted key directory. This has O(file-size) verification overhead;
/// not a fast live index or a replacement for journal replay.
pub fn lookup_index(path: &Path, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
    if !valid_id(namespace) || !valid_id(key) {
        return Err(Error::Invalid("index: invalid namespace/key"));
    }
    let (dir, entries, _) = read_index(path)?;
    match dir
        .rows
        .binary_search_by(|r| (r.namespace.as_str(), r.key.as_str()).cmp(&(namespace, key)))
    {
        Ok(i) => Ok(Some(entries[i].clone())),
        Err(_) => Ok(None),
    }
}
/// Convert a B3 **verified, explicitly SHA-approved** snapshot to a distinct
/// immutable point-read sidecar, excluding the receipt history.
/// Source snapshots and original journals are never rewritten or truncated.
pub fn create_index(
    snapshot: &Path,
    destination: &Path,
    approved_sha256: &str,
) -> Result<IndexReport> {
    if !digest_ok(approved_sha256) {
        return Err(Error::Invalid(
            "index: require approved lowercase snapshot SHA-256",
        ));
    }
    if snapshot == destination {
        return Err(Error::Invalid("index: source=destination"));
    }
    private_parent(destination)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(Error::Invalid("index: destination exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }
    let source = verify_snapshot(snapshot)?;
    if source.snapshot_sha256 != approved_sha256 {
        return Err(Error::Conflict(
            "index: approved snapshot SHA mismatch".into(),
        ));
    }
    let mut original = File::open(snapshot)?;
    same_file(&original, snapshot)?;
    let bytes = fs::read(snapshot)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES || sha(&bytes) != approved_sha256 {
        return Err(Error::Conflict(
            "index: snapshot changed during input read".into(),
        ));
    }
    let snap = parse_snapshot(&bytes)?;
    if snap.log_sha256 != source.source_sha256
        || snap.sequence != source.sequence
        || snap.entries.len() != source.entries
    {
        return Err(Error::Conflict("index: snapshot metadata changed".into()));
    }
    let mut data = Vec::new();
    let mut rows = Vec::new();
    for entry in &snap.entries {
        let encoded =
            serde_json::to_vec(entry).map_err(|_| Error::Invalid("index: cannot encode entry"))?;
        let length =
            u32::try_from(encoded.len()).map_err(|_| Error::Invalid("index: row too large"))?;
        rows.push(DirectoryEntry {
            namespace: entry.namespace.clone(),
            key: entry.key.clone(),
            revision: entry.revision,
            offset: data.len() as u64,
            bytes: length,
            sha256: sha(&encoded),
        });
        data.extend_from_slice(&encoded);
        if data.len() as u64 > MAX_INDEX_BYTES {
            return Err(Error::Invalid("index: data budget exceeded"));
        }
    }
    let dir = Directory {
        schema: SCHEMA.into(),
        snapshot_sha256: approved_sha256.into(),
        log_sha256: snap.log_sha256,
        sequence: snap.sequence,
        rows,
        data_bytes: data.len() as u64,
    };
    validate_dir(&dir)?;
    let dir_bytes =
        serde_json::to_vec(&dir).map_err(|_| Error::Invalid("index: cannot encode directory"))?;
    let dir_len =
        u32::try_from(dir_bytes.len()).map_err(|_| Error::Invalid("index: directory too large"))?;
    let data_len =
        u32::try_from(data.len()).map_err(|_| Error::Invalid("index: rows too large"))?;
    let size = 48u64 + u64::from(dir_len) + u64::from(data_len);
    if size > MAX_INDEX_BYTES {
        return Err(Error::Invalid("index: 64 MiB budget exceeded"));
    }
    let mut output = Vec::with_capacity(size as usize);
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&dir_len.to_be_bytes());
    output.extend_from_slice(&data_len.to_be_bytes());
    output.extend_from_slice(&dir_bytes);
    output.extend_from_slice(&data);
    output.extend_from_slice(&Sha256::digest(&output));
    let status = report(&dir, &output);
    // Verify complete serialization before writing.
    let (parsed, entries) = decode(&output)?;
    if parsed != dir || entries != snap.entries {
        return Err(Error::Conflict("index: source roundtrip failed".into()));
    }
    same_file(&original, snapshot)?;
    if original.metadata()?.len() != bytes.len() as u64
        || file_sha256(&mut original)? != approved_sha256
    {
        return Err(Error::Conflict(
            "index: source changed before writing".into(),
        ));
    }
    let mut opts = OpenOptions::new();
    opts.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut dest = opts.open(destination)?;
    let result = (|| -> Result<()> {
        dest.write_all(&output)?;
        dest.sync_all()?;
        File::open(
            destination
                .parent()
                .ok_or(Error::Invalid("index: parent missing"))?,
        )?
        .sync_all()?;
        private_file(destination)?;
        if verify_index(destination)? != status {
            return Err(Error::Conflict("index: stored output differs".into()));
        }
        same_file(&original, snapshot)?;
        if original.metadata()?.len() != bytes.len() as u64
            || file_sha256(&mut original)? != approved_sha256
        {
            return Err(Error::Conflict(
                "index: snapshot changed during write".into(),
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (dest.metadata(), fs::symlink_metadata(destination)) {
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
    Ok(status)
}
