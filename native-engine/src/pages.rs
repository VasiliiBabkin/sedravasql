//! B21/B22: immutable, checked page image of one approved snapshot generation.
//! Not a writable store, recovery checkpoint, or live SQL index.
use crate::{
    backup::same_file,
    private_file, private_parent,
    recovery::file_sha256,
    snapshot::{parse_snapshot, MAX_SNAPSHOT_BYTES},
    valid_id, Error, NativeEntry, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
pub const PAGE_SIZE: usize = 4096;
const PAYLOAD: usize = PAGE_SIZE - 16 - 32;
const MAX_PAGES: u32 = 32768;
const FANOUT: usize = 12;
const MAX_ROW: usize = 400_000;
const MAGIC: &[u8; 8] = b"SEDRPG01";
type Key = (String, String);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    root: u32,
    pages: u32,
    rows: u64,
    height: u32,
    sequence: u64,
    snapshot_sha256: String,
    log_sha256: String,
}
#[derive(Clone, Debug)]
struct Entry {
    key: Key,
    page: u32,
    bytes: u32,
}
#[derive(Debug, Serialize)]
pub struct PageReport {
    pub page_size: usize,
    pub pages: u32,
    pub rows: u64,
    pub height: u32,
    pub sequence: u64,
    pub snapshot_sha256: String,
    pub log_sha256: String,
}
fn bad() -> Error {
    Error::Invalid("pages: invalid page or tree")
}
fn sha(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        data.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap(),
    ))
}
fn encode(id: u32, kind: u8, payload: &[u8]) -> Result<[u8; PAGE_SIZE]> {
    if payload.len() > PAYLOAD {
        return Err(Error::Capacity("pages: payload"));
    }
    let mut page = [0; PAGE_SIZE];
    page[..8].copy_from_slice(MAGIC);
    page[8..12].copy_from_slice(&id.to_be_bytes());
    page[12] = kind;
    page[13..15].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    page[16..16 + payload.len()].copy_from_slice(payload);
    let hash = Sha256::digest(&page[..PAGE_SIZE - 32]);
    page[PAGE_SIZE - 32..].copy_from_slice(&hash);
    Ok(page)
}
fn read_page(file: &mut File, id: u32, pages: u32) -> Result<(u8, Vec<u8>)> {
    if id >= pages {
        return Err(bad());
    }
    file.seek(SeekFrom::Start(u64::from(id) * PAGE_SIZE as u64))?;
    let mut page = [0; PAGE_SIZE];
    file.read_exact(&mut page)?;
    let len = u16::from_be_bytes([page[13], page[14]]) as usize;
    if &page[..8] != MAGIC
        || u32_at(&page, 8)? != id
        || len > PAYLOAD
        || page[15] != 0
        || page[16 + len..PAGE_SIZE - 32].iter().any(|x| *x != 0)
        || Sha256::digest(&page[..PAGE_SIZE - 32]).as_slice() != &page[PAGE_SIZE - 32..]
    {
        return Err(bad());
    }
    Ok((page[12], page[16..16 + len].to_vec()))
}
fn pack(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut out = vec![entries.len() as u8];
    for e in entries {
        if !valid_id(&e.key.0) || !valid_id(&e.key.1) {
            return Err(bad());
        }
        for k in [&e.key.0, &e.key.1] {
            out.push(k.len() as u8);
            out.extend_from_slice(k.as_bytes());
        }
        out.extend_from_slice(&e.page.to_be_bytes());
        out.extend_from_slice(&e.bytes.to_be_bytes());
    }
    Ok(out)
}
fn unpack(data: &[u8]) -> Result<Vec<Entry>> {
    let count = usize::from(*data.first().ok_or_else(bad)?);
    if count > FANOUT {
        return Err(bad());
    }
    let mut at = 1;
    let mut out = Vec::<Entry>::new();
    for _ in 0..count {
        let mut key = Vec::new();
        for _ in 0..2 {
            let n = usize::from(*data.get(at).ok_or_else(bad)?);
            at += 1;
            let k = std::str::from_utf8(data.get(at..at + n).ok_or_else(bad)?)
                .map_err(|_| bad())?
                .to_owned();
            at += n;
            if !valid_id(&k) {
                return Err(bad());
            }
            key.push(k);
        }
        let e = Entry {
            key: (key.remove(0), key.remove(0)),
            page: u32_at(data, at)?,
            bytes: u32_at(data, at + 4)?,
        };
        at += 8;
        if out.last().is_some_and(|p| p.key >= e.key) {
            return Err(bad());
        }
        out.push(e);
    }
    if at != data.len() {
        return Err(bad());
    }
    Ok(out)
}
fn append(file: &mut File, next: &mut u32, kind: u8, payload: &[u8]) -> Result<u32> {
    if *next >= MAX_PAGES {
        return Err(Error::Capacity("pages: image budget"));
    }
    let id = *next;
    file.write_all(&encode(id, kind, payload)?)?;
    *next += 1;
    Ok(id)
}
fn make(file: &mut File, rows: &[NativeEntry], mut h: Header) -> Result<Header> {
    file.write_all(&[0; PAGE_SIZE])?;
    let mut next = 1;
    let mut leaf = Vec::new();
    let mut level = Vec::new();
    for row in rows {
        let data = serde_json::to_vec(row).map_err(|_| bad())?;
        if data.len() > MAX_ROW {
            return Err(Error::Capacity("pages: row budget"));
        }
        let start = next;
        for chunk in data.chunks(PAYLOAD) {
            append(file, &mut next, 3, chunk)?;
        }
        leaf.push(Entry {
            key: (row.namespace.clone(), row.key.clone()),
            page: start,
            bytes: data.len() as u32,
        });
        if leaf.len() == FANOUT {
            let id = append(file, &mut next, 1, &pack(&leaf)?)?;
            level.push(Entry {
                key: leaf[0].key.clone(),
                page: id,
                bytes: 0,
            });
            leaf.clear();
        }
    }
    if !leaf.is_empty() {
        let id = append(file, &mut next, 1, &pack(&leaf)?)?;
        level.push(Entry {
            key: leaf[0].key.clone(),
            page: id,
            bytes: 0,
        });
    }
    if level.is_empty() {
        h.root = append(file, &mut next, 1, &[0])?;
        h.height = 1;
    } else {
        h.height = 1;
        while level.len() > 1 {
            let mut above = Vec::new();
            for entries in level.chunks(FANOUT) {
                let id = append(file, &mut next, 2, &pack(entries)?)?;
                above.push(Entry {
                    key: entries[0].key.clone(),
                    page: id,
                    bytes: 0,
                });
            }
            level = above;
            h.height += 1;
        }
        h.root = level[0].page;
    }
    h.pages = next;
    let body = serde_json::to_vec(&h).map_err(|_| bad())?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&encode(0, 0, &body)?)?;
    file.sync_all()?;
    Ok(h)
}
fn mark(seen: &mut [bool], id: u32) -> Result<()> {
    let slot = seen.get_mut(id as usize).ok_or_else(bad)?;
    if *slot {
        return Err(bad());
    }
    *slot = true;
    Ok(())
}
fn read_row(
    file: &mut File,
    h: &Header,
    e: &Entry,
    mut seen: Option<&mut [bool]>,
    reads: &mut u64,
) -> Result<NativeEntry> {
    if e.bytes == 0 || e.bytes as usize > MAX_ROW || e.page == 0 {
        return Err(bad());
    }
    let mut remaining = e.bytes as usize;
    let mut bytes = Vec::with_capacity(remaining);
    let mut id = e.page;
    while remaining > 0 {
        if let Some(s) = seen.as_deref_mut() {
            mark(s, id)?;
        }
        let (kind, data) = read_page(file, id, h.pages)?;
        *reads += 1;
        if kind != 3 || data.len() != remaining.min(PAYLOAD) {
            return Err(bad());
        }
        remaining -= data.len();
        bytes.extend(data);
        id += 1;
    }
    let row: NativeEntry = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if (row.namespace.as_str(), row.key.as_str()) != (e.key.0.as_str(), e.key.1.as_str())
        || row.revision == 0
        || row.revision > h.sequence
        || row
            .value
            .as_ref()
            .is_some_and(|v| v.len() > crate::MAX_VALUE_BYTES || v.contains('\0'))
    {
        return Err(bad());
    }
    Ok(row)
}
struct Walk {
    first: Option<Key>,
    last: Option<Key>,
    rows: u64,
}
fn verify_node(
    file: &mut File,
    h: &Header,
    id: u32,
    depth: u32,
    seen: &mut [bool],
) -> Result<Walk> {
    mark(seen, id)?;
    let (kind, body) = read_page(file, id, h.pages)?;
    let entries = unpack(&body)?;
    if depth == 0 || kind != if depth == 1 { 1 } else { 2 } {
        return Err(bad());
    }
    if entries.is_empty() {
        if id != h.root || depth != 1 {
            return Err(bad());
        }
        return Ok(Walk {
            first: None,
            last: None,
            rows: 0,
        });
    }
    let mut result = Walk {
        first: Some(entries[0].key.clone()),
        last: None,
        rows: 0,
    };
    for e in entries {
        if result.last.as_ref().is_some_and(|k| k >= &e.key) {
            return Err(bad());
        }
        if depth == 1 {
            read_row(file, h, &e, Some(seen), &mut 0)?;
            result.last = Some(e.key);
            result.rows += 1;
        } else {
            if e.bytes != 0 || e.page >= id {
                return Err(bad());
            }
            let child = verify_node(file, h, e.page, depth - 1, seen)?;
            if child.first.as_ref() != Some(&e.key) {
                return Err(bad());
            }
            result.last = child.last;
            result.rows += child.rows;
        }
    }
    Ok(result)
}
#[cfg(unix)]
fn identity(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
pub struct PageReader {
    file: File,
    path: PathBuf,
    h: Header,
    #[cfg(unix)]
    identity: (u64, u64, u64, i64, i64, i64, i64),
    reads: u64,
}
impl PageReader {
    pub fn open(path: &Path) -> Result<Self> {
        private_parent(path)?;
        private_file(path)?;
        let mut file = File::open(path)?;
        same_file(&file, path)?;
        let meta = file.metadata()?;
        let len = meta.len();
        if len < 2 * PAGE_SIZE as u64
            || len % PAGE_SIZE as u64 != 0
            || len / PAGE_SIZE as u64 > MAX_PAGES as u64
        {
            return Err(bad());
        }
        let pages = (len / PAGE_SIZE as u64) as u32;
        let (kind, body) = read_page(&mut file, 0, pages)?;
        let h: Header = serde_json::from_slice(&body).map_err(|_| bad())?;
        if kind != 0
            || h.pages != pages
            || h.root == 0
            || h.root >= pages
            || !(1..=8).contains(&h.height)
            || !valid_hash(&h.snapshot_sha256)
            || !valid_hash(&h.log_sha256)
        {
            return Err(bad());
        }
        let mut seen = vec![false; pages as usize];
        seen[0] = true;
        let tree = verify_node(&mut file, &h, h.root, h.height, &mut seen)?;
        if tree.rows != h.rows || seen.iter().any(|x| !*x) {
            return Err(bad());
        }
        same_file(&file, path)?;
        #[cfg(unix)]
        if identity(&meta) != identity(&file.metadata()?) {
            return Err(Error::Conflict("pages: changed during verification".into()));
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            h,
            #[cfg(unix)]
            identity: identity(&meta),
            reads: 0,
        })
    }
    fn unchanged(&self) -> Result<()> {
        private_file(&self.path)?;
        same_file(&self.file, &self.path)?;
        #[cfg(unix)]
        if identity(&self.file.metadata()?) != self.identity {
            return Err(Error::Conflict("pages: image changed".into()));
        }
        Ok(())
    }
    pub fn report(&self) -> PageReport {
        PageReport {
            page_size: PAGE_SIZE,
            pages: self.h.pages,
            rows: self.h.rows,
            height: self.h.height,
            sequence: self.h.sequence,
            snapshot_sha256: self.h.snapshot_sha256.clone(),
            log_sha256: self.h.log_sha256.clone(),
        }
    }
    pub fn pages_read(&self) -> u64 {
        self.reads
    }
    pub fn get(&mut self, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
        if !valid_id(namespace) || !valid_id(key) {
            return Err(bad());
        }
        self.unchanged()?;
        let target = (namespace.to_owned(), key.to_owned());
        let mut id = self.h.root;
        let mut result = None;
        for depth in (1..=self.h.height).rev() {
            let (kind, data) = read_page(&mut self.file, id, self.h.pages)?;
            self.reads += 1;
            if kind != if depth == 1 { 1 } else { 2 } {
                return Err(bad());
            }
            let entries = unpack(&data)?;
            if depth == 1 {
                if let Ok(i) = entries.binary_search_by(|e| e.key.cmp(&target)) {
                    result = Some(read_row(
                        &mut self.file,
                        &self.h,
                        &entries[i],
                        None,
                        &mut self.reads,
                    )?);
                }
                break;
            }
            let i = entries.partition_point(|e| e.key <= target);
            if i == 0 {
                break;
            }
            id = entries[i - 1].page;
        }
        self.unchanged()?;
        Ok(result)
    }
    /// Check the image's generation against an exclusively opened live journal.
    /// The log remains authoritative. No live mutation uses this image.
    pub fn matches_log(&self, log: &mut crate::NativeLog) -> Result<bool> {
        self.unchanged()?;
        log.verify()?;
        let at = log.file.stream_position()?;
        let digest = file_sha256(&mut log.file);
        log.file.seek(SeekFrom::Start(at))?;
        Ok(log.sequence() == self.h.sequence && digest? == self.h.log_sha256)
    }
}
pub fn create(source: &Path, dest: &Path, approved: &str) -> Result<PageReport> {
    if !valid_hash(approved) {
        return Err(bad());
    }
    private_parent(source)?;
    private_file(source)?;
    private_parent(dest)?;
    let mut input = File::open(source)?;
    same_file(&input, source)?;
    let mut bytes = Vec::new();
    (&mut input)
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES || sha(&bytes) != approved {
        return Err(Error::Conflict("pages: source hash mismatch".into()));
    }
    let snapshot = parse_snapshot(&bytes)?;
    same_file(&input, source)?;
    if file_sha256(&mut input)? != approved {
        return Err(Error::Conflict("pages: source changed".into()));
    }
    let h = Header {
        root: 0,
        pages: 0,
        height: 0,
        rows: snapshot.entries.len() as u64,
        sequence: snapshot.sequence,
        snapshot_sha256: approved.into(),
        log_sha256: snapshot.log_sha256,
    };
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(dest)?;
    // Never unlink a pathname on error; an incomplete image fails verification.
    make(&mut file, &snapshot.entries, h)?;
    drop(file);
    let report = PageReader::open(dest)?.report();
    File::open(dest.parent().ok_or_else(bad)?)?.sync_all()?;
    Ok(report)
}
