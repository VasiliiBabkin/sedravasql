//! B29-A: experimental direct-addressed binary copy-on-write B+ tree.
//! Single writer, private Unix directories, bounded pages, no SQL integration.
use crate::{
    backup::same_file, private_file, private_parent, valid_id, Error, ExclusiveLock, Result,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const SIZE: usize = 4096;
const PAYLOAD_END: usize = SIZE - 32;
const PAGE_MAGIC: &[u8; 8] = b"SEDRCOW1";
const HEAD_V1: &[u8; 8] = b"SEDRHDR1";
const HEAD_V2: &[u8; 8] = b"SEDRHDR2";
const HEAD_MAGIC: &[u8; 8] = b"SEDRHDR3";
const HEAD_V4: &[u8; 8] = b"SEDRHDR4";
/// Fail closed when the manifest cannot retain another receipt. No eviction.
pub const MAX_RECEIPTS: usize = 48;
const MAX_PAGES: u64 = 65_536;
const MAX_ROWS: u64 = 32_768;
const MAX_DEPTH: usize = 8;
const LEAF_MAX: usize = 8;
const BRANCH_MAX: usize = 24;
const INLINE_MAX: usize = 256;
const VALUE_MAX: usize = 65_536;
const OVERFLOW_CHUNK: usize = 4000;
// Limit memory for the offline compact-copy operation separately from
// normal point reads and writes. No on-disk mutation if this budget fails.
const SNAPSHOT_VALUE_MAX: usize = 32 * 1024 * 1024;
const CACHE_MAX: usize = 64;
const MAX_STAGE: usize = 640;
type Page = [u8; SIZE];

fn bad() -> Error {
    Error::Invalid("cow tree: invalid binary structure")
}
fn cap() -> Error {
    Error::Capacity("cow tree: capacity budget")
}
fn hash(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}
fn sibling(p: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = p.file_name().ok_or_else(bad)?.to_os_string();
    name.push(suffix);
    Ok(p.with_file_name(name))
}
fn exists(p: &Path) -> Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn new_file(p: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(p)?)
}
fn sync_dir(p: &Path) -> Result<()> {
    File::open(p.parent().ok_or_else(bad)?)?.sync_all()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    pub key: String,
    pub revision: u64,
    pub value: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Mutation {
    pub key: String,
    pub expected_revision: u64,
    pub value: Option<String>,
}
#[derive(Clone, Debug)]
enum StoredValue {
    Inline(String),
    Overflow {
        len: usize,
        first: u64,
        digest: [u8; 32],
    },
}
#[derive(Clone, Debug)]
struct StoredRow {
    key: String,
    revision: u64,
    value: Option<StoredValue>,
}
#[derive(Clone, Debug)]
enum Node {
    Leaf(Vec<StoredRow>),
    Overflow {
        next: u64,
        bytes: Vec<u8>,
    },
    Branch {
        keys: Vec<String>,
        children: Vec<u64>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Head {
    identity: [u8; 16],
    generation: u64,
    root: u64,
    pages: u64,
    rows: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredReceipt {
    key: [u8; 32],
    request: [u8; 32],
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Receipt {
    pub generation: u64,
    pub replayed: bool,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub generation: u64,
    pub rows: u64,
    pub pages: u64,
    pub file_bytes: u64,
    pub cached_pages: usize,
    pub cache_hits: u64,
    pub disk_reads: u64,
}

struct Decode<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Decode<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(bad)?;
        let out = self.bytes.get(self.at..end).ok_or_else(bad)?;
        self.at = end;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().map_err(|_| bad())?,
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| bad())?,
        ))
    }
    fn key(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| bad())?;
        if !valid_id(s) {
            return Err(bad());
        }
        Ok(s.into())
    }
}
fn put_key(buf: &mut Vec<u8>, key: &str) {
    buf.extend((key.len() as u16).to_be_bytes());
    buf.extend(key.as_bytes());
}
fn seal(p: &mut Page) {
    let digest = hash(&p[..PAYLOAD_END]);
    p[PAYLOAD_END..].copy_from_slice(&digest);
}
fn checked(p: &Page, magic: &[u8; 8]) -> Result<()> {
    if &p[..8] != magic || hash(&p[..PAYLOAD_END]) != p[PAYLOAD_END..] {
        return Err(bad());
    }
    Ok(())
}
fn header(identity: &[u8; 16]) -> Page {
    let mut p = [0; SIZE];
    p[..8].copy_from_slice(PAGE_MAGIC);
    p[8..24].copy_from_slice(identity);
    seal(&mut p);
    p
}
impl Head {
    fn encode(self) -> Page {
        let mut p = [0; SIZE];
        p[..8].copy_from_slice(HEAD_MAGIC);
        p[8..24].copy_from_slice(&self.identity);
        for (i, n) in [self.generation, self.root, self.pages, self.rows]
            .into_iter()
            .enumerate()
        {
            p[24 + i * 8..32 + i * 8].copy_from_slice(&n.to_be_bytes());
        }
        seal(&mut p);
        p
    }
    fn decode(p: &Page) -> Result<(Self, Vec<StoredReceipt>)> {
        let legacy = &p[..8] == HEAD_V1;
        let magic = if legacy {
            HEAD_V1
        } else if &p[..8] == HEAD_V2 {
            HEAD_V2
        } else if &p[..8] == HEAD_V4 {
            HEAD_V4
        } else {
            HEAD_MAGIC
        };
        checked(p, magic)?;
        let mut d = Decode {
            bytes: &p[..PAYLOAD_END],
            at: 8,
        };
        let identity = d.take(16)?.try_into().map_err(|_| bad())?;
        let h = Self {
            identity,
            generation: d.u64()?,
            root: d.u64()?,
            pages: d.u64()?,
            rows: d.u64()?,
        };
        if h.pages < 2
            || h.pages > MAX_PAGES
            || h.root == 0
            || h.root >= h.pages
            || h.rows > MAX_ROWS
            || (legacy && h.generation >= h.pages)
        {
            return Err(bad());
        }
        let mut receipts = Vec::new();
        if magic == HEAD_V4 {
            let count = d.u16()? as usize;
            if !(1..=MAX_RECEIPTS).contains(&count) {
                return Err(bad());
            }
            let mut keys = BTreeSet::new();
            let mut previous = 0;
            for _ in 0..count {
                let r = StoredReceipt {
                    key: d.take(32)?.try_into().map_err(|_| bad())?,
                    request: d.take(32)?.try_into().map_err(|_| bad())?,
                    generation: d.u64()?,
                };
                if r.generation <= previous || r.generation > h.generation || !keys.insert(r.key) {
                    return Err(bad());
                }
                previous = r.generation;
                receipts.push(r);
            }
        }
        if p[d.at..PAYLOAD_END].iter().any(|&b| b != 0) {
            return Err(bad());
        }
        Ok((h, receipts))
    }

    fn encode_manifest(self, receipts: &[StoredReceipt]) -> Result<Page> {
        if receipts.len() > MAX_RECEIPTS {
            return Err(cap());
        }
        let mut p = self.encode();
        if !receipts.is_empty() {
            p[..8].copy_from_slice(HEAD_V4);
            p[56..58].copy_from_slice(&(receipts.len() as u16).to_be_bytes());
            let mut at = 58;
            for r in receipts {
                p[at..at + 32].copy_from_slice(&r.key);
                p[at + 32..at + 64].copy_from_slice(&r.request);
                p[at + 64..at + 72].copy_from_slice(&r.generation.to_be_bytes());
                at += 72;
            }
            seal(&mut p);
        }
        // Validate canonical encoding before touching persistent bytes.
        Self::decode(&p)?;
        Ok(p)
    }
}

fn request_digest(ops: &[Mutation]) -> [u8; 32] {
    let mut d = Sha256::new();
    d.update(b"sedravasql-cow-receipt-request-v1\0");
    d.update((ops.len() as u64).to_be_bytes());
    for op in ops {
        d.update((op.key.len() as u64).to_be_bytes());
        d.update(op.key.as_bytes());
        d.update(op.expected_revision.to_be_bytes());
        match &op.value {
            None => d.update([0]),
            Some(value) => {
                d.update([1]);
                d.update((value.len() as u64).to_be_bytes());
                d.update(value.as_bytes());
            }
        }
    }
    d.finalize().into()
}
fn encode(node: &Node, h: Head, id: u64) -> Result<Page> {
    let mut b = Vec::new();
    let kind = match node {
        Node::Leaf(rows) => {
            if rows.len() > LEAF_MAX {
                return Err(cap());
            }
            b.extend((rows.len() as u16).to_be_bytes());
            for r in rows {
                put_key(&mut b, &r.key);
                b.extend(r.revision.to_be_bytes());
                match &r.value {
                    None => b.extend(u16::MAX.to_be_bytes()),
                    Some(StoredValue::Inline(v)) => {
                        if v.len() > INLINE_MAX || v.contains('\0') {
                            return Err(bad());
                        }
                        b.extend((v.len() as u16).to_be_bytes());
                        b.extend(v.as_bytes());
                    }
                    Some(StoredValue::Overflow { len, first, digest }) => {
                        if !(INLINE_MAX + 1..=VALUE_MAX).contains(len)
                            || *first == 0
                            || *first >= id
                        {
                            return Err(bad());
                        }
                        b.extend((u16::MAX - 1).to_be_bytes());
                        b.extend((*len as u64).to_be_bytes());
                        b.extend(first.to_be_bytes());
                        b.extend(digest);
                    }
                }
            }
            1
        }
        Node::Overflow { next, bytes } => {
            if bytes.is_empty() || bytes.len() > OVERFLOW_CHUNK || *next >= id {
                return Err(bad());
            }
            b.extend((bytes.len() as u16).to_be_bytes());
            b.extend(next.to_be_bytes());
            b.extend(bytes);
            3
        }
        Node::Branch { keys, children } => {
            if keys.is_empty() || keys.len() > BRANCH_MAX || children.len() != keys.len() + 1 {
                return Err(bad());
            }
            b.extend((keys.len() as u16).to_be_bytes());
            b.extend(children[0].to_be_bytes());
            for (k, c) in keys.iter().zip(&children[1..]) {
                put_key(&mut b, k);
                b.extend(c.to_be_bytes());
            }
            2
        }
    };
    if b.len() > PAYLOAD_END - 43 {
        return Err(cap());
    }
    let mut p = [0; SIZE];
    p[..8].copy_from_slice(PAGE_MAGIC);
    p[8..24].copy_from_slice(&h.identity);
    p[24..32].copy_from_slice(&id.to_be_bytes());
    p[32..40].copy_from_slice(&h.generation.to_be_bytes());
    p[40] = kind;
    p[41..43].copy_from_slice(&(b.len() as u16).to_be_bytes());
    p[43..43 + b.len()].copy_from_slice(&b);
    seal(&mut p);
    Ok(p)
}
fn decode(p: &Page, h: Head, id: u64) -> Result<Node> {
    checked(p, PAGE_MAGIC)?;
    let mut hd = Decode {
        bytes: &p[..PAYLOAD_END],
        at: 8,
    };
    if hd.take(16)? != h.identity || hd.u64()? != id || hd.u64()? > h.generation {
        return Err(bad());
    }
    let kind = hd.take(1)?[0];
    let n = hd.u16()? as usize;
    let b = hd.take(n)?;
    if p[hd.at..PAYLOAD_END].iter().any(|&b| b != 0) {
        return Err(bad());
    }
    let mut d = Decode { bytes: b, at: 0 };
    let count = d.u16()? as usize;
    let node = match kind {
        1 if count <= LEAF_MAX => {
            let mut rows = Vec::with_capacity(count);
            for _ in 0..count {
                let key = d.key()?;
                let revision = d.u64()?;
                if revision == 0 || revision > h.generation {
                    return Err(bad());
                }
                let len = d.u16()?;
                let value = if len == u16::MAX {
                    None
                } else if len == u16::MAX - 1 {
                    let len = usize::try_from(d.u64()?).map_err(|_| bad())?;
                    let first = d.u64()?;
                    let digest = d.take(32)?.try_into().map_err(|_| bad())?;
                    if !(INLINE_MAX + 1..=VALUE_MAX).contains(&len) || first == 0 || first >= id {
                        return Err(bad());
                    }
                    Some(StoredValue::Overflow { len, first, digest })
                } else {
                    if len as usize > INLINE_MAX {
                        return Err(bad());
                    }
                    let v = std::str::from_utf8(d.take(len as usize)?).map_err(|_| bad())?;
                    if v.contains('\0') {
                        return Err(bad());
                    }
                    Some(StoredValue::Inline(v.into()))
                };
                rows.push(StoredRow {
                    key,
                    revision,
                    value,
                });
            }
            if rows.windows(2).any(|w| w[0].key >= w[1].key) {
                return Err(bad());
            }
            Node::Leaf(rows)
        }
        3 if (1..=OVERFLOW_CHUNK).contains(&count) => {
            let next = d.u64()?;
            if next >= id {
                return Err(bad());
            }
            Node::Overflow {
                next,
                bytes: d.take(count)?.to_vec(),
            }
        }
        2 if (1..=BRANCH_MAX).contains(&count) => {
            let mut keys = Vec::with_capacity(count);
            let mut children = Vec::with_capacity(count + 1);
            children.push(d.u64()?);
            for _ in 0..count {
                keys.push(d.key()?);
                children.push(d.u64()?);
            }
            if keys.windows(2).any(|w| w[0] >= w[1]) || children.iter().any(|&c| c == 0 || c >= id)
            {
                return Err(bad());
            }
            Node::Branch { keys, children }
        }
        _ => return Err(bad()),
    };
    if d.at != b.len() {
        return Err(bad());
    }
    Ok(node)
}
// Split bytes, not Unicode scalar boundaries; the full string is decoded
// only after exact-length and digest validation. Child links descend in ID.
fn store_row(row: Record, mut add: impl FnMut(Node) -> Result<u64>) -> Result<StoredRow> {
    let value = match row.value {
        None => None,
        Some(v) if v.len() <= INLINE_MAX => Some(StoredValue::Inline(v)),
        Some(v) => {
            if v.len() > VALUE_MAX || v.contains('\0') {
                return Err(cap());
            }
            let mut first = 0;
            for chunk in v.as_bytes().chunks(OVERFLOW_CHUNK).rev() {
                first = add(Node::Overflow {
                    next: first,
                    bytes: chunk.to_vec(),
                })?;
            }
            Some(StoredValue::Overflow {
                len: v.len(),
                first,
                digest: hash(v.as_bytes()),
            })
        }
    };
    Ok(StoredRow {
        key: row.key,
        revision: row.revision,
        value,
    })
}

fn read_page(f: &mut File, id: u64) -> Result<Page> {
    let mut p = [0; SIZE];
    f.seek(SeekFrom::Start(
        id.checked_mul(SIZE as u64).ok_or_else(bad)?,
    ))?;
    f.read_exact(&mut p)?;
    Ok(p)
}

struct Stage {
    head: Head,
    base: u64,
    nodes: Vec<Node>,
    reuse: bool,
}
impl Stage {
    fn add(&mut self, node: Node) -> Result<u64> {
        if self.nodes.len() >= MAX_STAGE || (!self.reuse && self.head.pages >= MAX_PAGES) {
            return Err(cap());
        }
        let id = self.head.pages;
        // Validate fit and serialize before admitting a page.
        encode(&node, self.head, id)?;
        self.head.pages += 1;
        self.nodes.push(node);
        Ok(id)
    }
    fn compact(&mut self) -> Result<()> {
        let mut live = BTreeSet::new();
        let mut todo = vec![self.head.root];
        while let Some(id) = todo.pop() {
            if id < self.base || !live.insert(id) {
                continue;
            }
            let node = self.nodes.get((id - self.base) as usize).ok_or_else(bad)?;
            match node {
                Node::Branch { children, .. } => todo.extend(children.iter().copied()),
                Node::Leaf(rows) => {
                    for r in rows {
                        if let Some(StoredValue::Overflow { first, .. }) = &r.value {
                            todo.push(*first);
                        }
                    }
                }
                Node::Overflow { next, .. } => {
                    if *next != 0 {
                        todo.push(*next);
                    }
                }
            }
        }
        let remap: BTreeMap<u64, u64> = live
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, self.base + i as u64))
            .collect();
        let translate = |id: u64| -> Result<u64> {
            if id < self.base {
                Ok(id)
            } else {
                remap.get(&id).copied().ok_or_else(bad)
            }
        };
        let mut nodes = Vec::with_capacity(live.len());
        for id in live {
            let mut n = self
                .nodes
                .get((id - self.base) as usize)
                .cloned()
                .ok_or_else(bad)?;
            match &mut n {
                Node::Branch { children, .. } => {
                    for child in children {
                        *child = translate(*child)?;
                    }
                }
                Node::Leaf(rows) => {
                    for r in rows {
                        if let Some(StoredValue::Overflow { first, .. }) = &mut r.value {
                            *first = translate(*first)?;
                        }
                    }
                }
                Node::Overflow { next, .. } => {
                    if *next != 0 {
                        *next = translate(*next)?;
                    }
                }
            }
            nodes.push(n);
        }
        self.head.root = translate(self.head.root)?;
        self.head.pages = self.base + nodes.len() as u64;
        self.nodes = nodes;
        Ok(())
    }
}
pub struct CowTree {
    path: PathBuf,
    file: File,
    _lock: ExclusiveLock,
    head: Head,
    receipts: Vec<StoredReceipt>,
    height: usize,
    cache: BTreeMap<u64, Node>,
    fifo: VecDeque<u64>,
    hits: u64,
    reads: u64,
    poisoned: bool,
}
struct Walk {
    first: Option<String>,
    last: Option<String>,
    height: usize,
    rows: u64,
}
struct Split {
    left: u64,
    right: Option<(String, u64)>,
}
impl CowTree {
    fn from_parts(path: &Path, file: File, lock: ExclusiveLock, head: Head) -> Self {
        Self {
            path: path.into(),
            file,
            _lock: lock,
            head,
            receipts: vec![],
            height: 1,
            cache: BTreeMap::new(),
            fifo: VecDeque::new(),
            hits: 0,
            reads: 0,
            poisoned: false,
        }
    }
    pub fn create(path: &Path) -> Result<Self> {
        private_parent(path)?;
        let lock = ExclusiveLock::acquire(path)?;
        let hp = sibling(path, ".head")?;
        let tmp = sibling(path, ".head.tmp")?;
        if exists(path)? || exists(&hp)? || exists(&tmp)? {
            return Err(Error::Conflict("cow tree: destination exists".into()));
        }
        let mut identity = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut identity)?;
        let head = Head {
            identity,
            generation: 0,
            root: 1,
            pages: 2,
            rows: 0,
        };
        let mut file = new_file(path)?;
        file.write_all(&header(&identity))?;
        file.write_all(&encode(&Node::Leaf(vec![]), head, 1)?)?;
        file.sync_all()?;
        let mut hf = new_file(&hp)?;
        hf.write_all(&head.encode())?;
        hf.sync_all()?;
        sync_dir(path)?;
        Ok(Self::from_parts(path, file, lock, head))
    }
    pub fn open(path: &Path) -> Result<Self> {
        private_parent(path)?;
        private_file(path)?;
        let lock = ExclusiveLock::acquire(path)?;
        let hp = sibling(path, ".head")?;
        let tmp = sibling(path, ".head.tmp")?;
        if exists(&tmp)? {
            return Err(Error::Conflict(
                "cow tree: staging head requires operator review".into(),
            ));
        }
        private_file(&hp)?;
        let mut hf = File::open(&hp)?;
        same_file(&hf, &hp)?;
        if hf.metadata()?.len() != SIZE as u64 {
            return Err(bad());
        }
        let (head, receipts) = Head::decode(&read_page(&mut hf, 0)?)?;
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        same_file(&file, path)?;
        if read_page(&mut file, 0)? != header(&head.identity) {
            return Err(bad());
        }
        let committed = head.pages * SIZE as u64;
        let length = file.metadata()?.len();
        if length < committed || length > MAX_PAGES * SIZE as u64 {
            return Err(bad());
        }
        let mut tree = Self::from_parts(path, file, lock, head);
        tree.receipts = receipts;
        // Never roll back to an older root if committed bytes are corrupt.
        let mut seen = BTreeSet::new();
        let info = tree.verify_node(head.root, 1, &mut seen)?;
        if info.rows != head.rows {
            return Err(bad());
        }
        tree.height = info.height;
        // Only unreferenced, unpublished append tail can be discarded.
        if length > committed {
            tree.file.set_len(committed)?;
            tree.file.sync_all()?;
        }
        Ok(tree)
    }
    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Conflict(
                "cow tree: uncertain commit; reopen required".into(),
            ));
        }
        same_file(&self.file, &self.path)
    }
    fn node(&mut self, id: u64) -> Result<Node> {
        if id == 0 || id >= self.head.pages {
            return Err(bad());
        }
        if let Some(n) = self.cache.get(&id) {
            self.hits += 1;
            return Ok(n.clone());
        }
        let n = decode(&read_page(&mut self.file, id)?, self.head, id)?;
        self.reads += 1;
        if self.cache.len() == CACHE_MAX {
            let old = self.fifo.pop_front().ok_or_else(bad)?;
            self.cache.remove(&old);
        }
        self.cache.insert(id, n.clone());
        self.fifo.push_back(id);
        Ok(n)
    }
    fn staged_node(&mut self, s: &Stage, id: u64) -> Result<Node> {
        if id < s.base {
            self.node(id)
        } else {
            s.nodes.get((id - s.base) as usize).cloned().ok_or_else(bad)
        }
    }
    fn materialize(
        &mut self,
        r: StoredRow,
        mut seen: Option<&mut BTreeSet<u64>>,
    ) -> Result<Record> {
        let value = match r.value {
            None => None,
            Some(StoredValue::Inline(v)) => Some(v),
            Some(StoredValue::Overflow { len, first, digest }) => {
                let mut data = Vec::with_capacity(len);
                let mut id = first;
                while data.len() < len {
                    if id == 0 || seen.as_deref_mut().is_some_and(|s| !s.insert(id)) {
                        return Err(bad());
                    }
                    let Node::Overflow { next, bytes } = self.node(id)? else {
                        return Err(bad());
                    };
                    let expected = (len - data.len()).min(OVERFLOW_CHUNK);
                    if bytes.len() != expected || (next == 0) != (data.len() + expected == len) {
                        return Err(bad());
                    }
                    data.extend(bytes);
                    id = next;
                }
                if id != 0 || hash(&data) != digest {
                    return Err(bad());
                }
                let v = String::from_utf8(data).map_err(|_| bad())?;
                if v.contains('\0') {
                    return Err(bad());
                }
                Some(v)
            }
        };
        Ok(Record {
            key: r.key,
            revision: r.revision,
            value,
        })
    }

    fn verify_node(&mut self, id: u64, depth: usize, seen: &mut BTreeSet<u64>) -> Result<Walk> {
        if depth > MAX_DEPTH || !seen.insert(id) {
            return Err(bad());
        }
        match self.node(id)? {
            Node::Leaf(rows) => {
                if rows.is_empty() && depth != 1 {
                    return Err(bad());
                }
                for r in &rows {
                    self.materialize(r.clone(), Some(seen))?;
                }
                Ok(Walk {
                    first: rows.first().map(|r| r.key.clone()),
                    last: rows.last().map(|r| r.key.clone()),
                    height: 1,
                    rows: rows.len() as u64,
                })
            }
            Node::Branch { keys, children } => {
                let mut out = Walk {
                    first: None,
                    last: None,
                    height: 0,
                    rows: 0,
                };
                for (i, c) in children.into_iter().enumerate() {
                    let v = self.verify_node(c, depth + 1, seen)?;
                    if v.first.is_none() || v.last.is_none() {
                        return Err(bad());
                    }
                    if i == 0 {
                        out.first = v.first.clone();
                        out.height = v.height + 1;
                    } else if v.first.as_ref() != Some(&keys[i - 1])
                        || out.last >= v.first
                        || out.height != v.height + 1
                    {
                        return Err(bad());
                    }
                    out.last = v.last;
                    out.rows = out.rows.checked_add(v.rows).ok_or_else(bad)?;
                    if out.rows > MAX_ROWS {
                        return Err(bad());
                    }
                }
                Ok(out)
            }
            Node::Overflow { .. } => Err(bad()),
        }
    }
    // B30: an offline compact copy, never an in-place swap of the two
    // committed files. Verify uncached source bytes before reading rows.
    // Retain physical keys (including tombstones), revisions and generation.
    fn verified_ownership(&mut self) -> Result<BTreeSet<u64>> {
        self.healthy()?;
        let hp = sibling(&self.path, ".head")?;
        private_file(&hp)?;
        let mut hf = File::open(&hp)?;
        same_file(&hf, &hp)?;
        if hf.metadata()?.len() != SIZE as u64
            || Head::decode(&read_page(&mut hf, 0)?)? != (self.head, self.receipts.clone())
            || self.file.metadata()?.len() != self.head.pages * SIZE as u64
            || read_page(&mut self.file, 0)? != header(&self.head.identity)
            || exists(&sibling(&self.path, ".head.tmp")?)?
        {
            return Err(bad());
        }
        self.cache.clear();
        self.fifo.clear();
        let mut seen = BTreeSet::new();
        let info = self.verify_node(self.head.root, 1, &mut seen)?;
        if info.rows != self.head.rows || info.height != self.height {
            return Err(bad());
        }
        Ok(seen)
    }

    fn snapshot_rows(&mut self) -> Result<Vec<Record>> {
        self.verified_ownership()?;
        let mut rows = Vec::with_capacity(self.head.rows as usize);
        let mut value_bytes = 0;
        self.collect_rows(self.head.root, 1, &mut rows, &mut value_bytes)?;
        if rows.len() as u64 != self.head.rows || rows.windows(2).any(|w| w[0].key >= w[1].key) {
            return Err(bad());
        }
        Ok(rows)
    }

    fn collect_rows(
        &mut self,
        id: u64,
        depth: usize,
        out: &mut Vec<Record>,
        value_bytes: &mut usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(bad());
        }
        match self.node(id)? {
            Node::Leaf(rows) => {
                if out.len() + rows.len() > MAX_ROWS as usize {
                    return Err(cap());
                }
                for r in rows {
                    let r = self.materialize(r, None)?;
                    *value_bytes = value_bytes
                        .checked_add(r.value.as_ref().map_or(0, |v| v.len()))
                        .ok_or_else(cap)?;
                    if *value_bytes > SNAPSHOT_VALUE_MAX {
                        return Err(cap());
                    }
                    out.push(r);
                }
            }
            Node::Branch { children, .. } => {
                for c in children {
                    self.collect_rows(c, depth + 1, out, value_bytes)?;
                }
            }
            Node::Overflow { .. } => return Err(bad()),
        }
        Ok(())
    }

    /// Build a packed, independent snapshot with fresh identity. The source
    /// is unchanged, including on failure. Never replace an existing target.
    /// Memory is bounded by MAX_ROWS and VALUE_MAX. Incomplete destinations
    /// after interruption must be reviewed or discarded, not resumed.
    /// Publication is via the same synced staging manifest as commit.
    pub fn compact_copy(&mut self, destination: &Path) -> Result<Report> {
        private_parent(destination)?;
        // Reserve the complete source and destination file families, including
        // a staging name which may not exist yet. Resolve parent aliases.
        let normalize = |p: &Path| -> Result<PathBuf> {
            Ok(fs::canonicalize(p.parent().ok_or_else(bad)?)?.join(p.file_name().ok_or_else(bad)?))
        };
        let source = normalize(&self.path)?;
        let target = normalize(destination)?;
        let names = |p: &Path| -> Result<Vec<PathBuf>> {
            Ok(vec![
                p.to_owned(),
                sibling(p, ".head")?,
                sibling(p, ".head.tmp")?,
                sibling(p, ".lock")?,
            ])
        };
        let reserved = names(&source)?;
        if names(&target)?.iter().any(|p| reserved.contains(p)) {
            return Err(Error::Conflict(
                "cow tree: overlapping compact file families".into(),
            ));
        }
        let lock = ExclusiveLock::acquire(destination)?;
        let hp = sibling(destination, ".head")?;
        let tmp = sibling(destination, ".head.tmp")?;
        if exists(destination)? || exists(&hp)? || exists(&tmp)? {
            return Err(Error::Conflict(
                "cow tree: compact destination exists".into(),
            ));
        }
        let rows = self.snapshot_rows()?;
        let mut identity = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut identity)?;
        let mut head = Head {
            identity,
            generation: self.head.generation,
            rows: self.head.rows,
            root: 1,
            pages: 1,
        };
        let mut nodes = Vec::new();
        let mut level = Vec::new();
        // Children are written before parents; separators equal right-child
        // minima. Dense leaves, evenly balanced groups of at most 25 children
        // avoid an illegal one-child non-root branch.
        for chunk in rows.chunks(LEAF_MAX) {
            let mut stored = Vec::with_capacity(chunk.len());
            for r in chunk {
                stored.push(store_row(r.clone(), |n| {
                    let id = nodes.len() as u64 + 1;
                    if id >= MAX_PAGES {
                        return Err(cap());
                    }
                    nodes.push(n);
                    Ok(id)
                })?);
            }
            let id = nodes.len() as u64 + 1;
            level.push((chunk[0].key.clone(), id));
            nodes.push(Node::Leaf(stored));
        }
        if level.is_empty() {
            level.push((String::new(), 1));
            nodes.push(Node::Leaf(vec![]));
        }
        let mut height = 1;
        while level.len() > 1 {
            height += 1;
            if height > MAX_DEPTH {
                return Err(cap());
            }
            let groups = level.len().div_ceil(BRANCH_MAX + 1);
            let size = level.len() / groups;
            let extra = level.len() % groups;
            let mut next = Vec::with_capacity(groups);
            let mut offset = 0;
            for i in 0..groups {
                let count = size + usize::from(i < extra);
                let chunk = &level[offset..offset + count];
                if !(2..=BRANCH_MAX + 1).contains(&count) {
                    return Err(bad());
                }
                let id = nodes.len() as u64 + 1;
                nodes.push(Node::Branch {
                    keys: chunk[1..].iter().map(|(k, _)| k.clone()).collect(),
                    children: chunk.iter().map(|(_, id)| *id).collect(),
                });
                next.push((chunk[0].0.clone(), id));
                offset += count;
            }
            level = next;
        }
        head.root = level[0].1;
        head.pages = nodes.len() as u64 + 1;
        if head.pages > MAX_PAGES {
            return Err(cap());
        }
        let manifest = head.encode_manifest(&self.receipts)?;
        // Preflight each binary node before creating the output.
        for (i, n) in nodes.iter().enumerate() {
            encode(n, head, i as u64 + 1)?;
        }
        let mut file = new_file(destination)?;
        file.write_all(&header(&identity))?;
        for (i, n) in nodes.iter().enumerate() {
            file.write_all(&encode(n, head, i as u64 + 1)?)?;
        }
        file.sync_all()?;
        crate::debug_fault("cow_compact_after_pages", &file);
        let mut stage = new_file(&tmp)?;
        stage.write_all(&manifest)?;
        stage.sync_all()?;
        crate::debug_fault("cow_compact_after_stage", &stage);
        fs::rename(&tmp, &hp)?;
        crate::debug_fault("cow_compact_after_publish", &stage);
        sync_dir(destination)?;
        drop(stage);
        // Re-read the committed header and pages under the same destination
        // lock; no race with a cooperating opener during verification.
        let mut output = Self::from_parts(destination, file, lock, head);
        output.height = height;
        output.receipts = self.receipts.clone();
        if output.snapshot_rows()? != rows {
            return Err(bad());
        }
        Ok(output.report())
    }

    pub fn report(&self) -> Report {
        Report {
            generation: self.head.generation,
            rows: self.head.rows,
            pages: self.head.pages,
            file_bytes: self.head.pages * SIZE as u64 + SIZE as u64,
            cached_pages: self.cache.len(),
            cache_hits: self.hits,
            disk_reads: self.reads,
        }
    }
    pub fn get(&mut self, key: &str) -> Result<Option<Record>> {
        self.healthy()?;
        if !valid_id(key) {
            return Err(bad());
        }
        let mut id = self.head.root;
        for _ in 0..MAX_DEPTH {
            match self.node(id)? {
                Node::Leaf(rows) => {
                    return match rows.binary_search_by(|r| r.key.as_str().cmp(key)) {
                        Ok(i) => Ok(Some(self.materialize(rows[i].clone(), None)?)),
                        Err(_) => Ok(None),
                    };
                }
                Node::Branch { keys, children } => {
                    id = children[keys.partition_point(|k| k.as_str() <= key)]
                }
                Node::Overflow { .. } => return Err(bad()),
            }
        }
        Err(bad())
    }
    fn range_node(
        &mut self,
        id: u64,
        start: &str,
        end: &str,
        limit: usize,
        depth: usize,
        out: &mut Vec<Record>,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(bad());
        }
        match self.node(id)? {
            Node::Leaf(rows) => {
                for r in rows
                    .into_iter()
                    .filter(|r| r.key.as_str() >= start && r.key.as_str() < end)
                    .take(limit - out.len())
                {
                    out.push(self.materialize(r, None)?);
                }
            }
            Node::Branch { keys, children } => {
                let first = keys.partition_point(|k| k.as_str() <= start);
                let last = keys.partition_point(|k| k.as_str() < end);
                for &c in &children[first..=last] {
                    if out.len() == limit {
                        break;
                    }
                    self.range_node(c, start, end, limit, depth + 1, out)?;
                }
            }
            Node::Overflow { .. } => return Err(bad()),
        }
        Ok(())
    }
    pub fn range(&mut self, start: &str, end: &str, limit: usize) -> Result<Vec<Record>> {
        self.healthy()?;
        if !valid_id(start) || !valid_id(end) || start >= end || !(1..=256).contains(&limit) {
            return Err(bad());
        }
        let mut out = Vec::new();
        self.range_node(self.head.root, start, end, limit, 1, &mut out)?;
        Ok(out)
    }
    fn insert(&mut self, s: &mut Stage, id: u64, m: &Mutation, depth: usize) -> Result<Split> {
        if depth > MAX_DEPTH {
            return Err(cap());
        }
        match self.staged_node(s, id)? {
            Node::Leaf(mut rows) => {
                let pos = rows.binary_search_by(|r| r.key.cmp(&m.key));
                let old = pos.ok().map(|i| &rows[i]);
                if old.map_or(0, |r| r.revision) != m.expected_revision
                    || m.value.is_none() && old.is_none_or(|r| r.value.is_none())
                {
                    return Err(Error::Conflict(
                        "cow tree: revision or deletion conflict".into(),
                    ));
                }
                let row = store_row(
                    Record {
                        key: m.key.clone(),
                        revision: m.expected_revision.checked_add(1).ok_or_else(cap)?,
                        value: m.value.clone(),
                    },
                    |n| s.add(n),
                )?;
                match pos {
                    Ok(i) => rows[i] = row,
                    Err(i) => {
                        if s.head.rows >= MAX_ROWS {
                            return Err(cap());
                        }
                        s.head.rows += 1;
                        rows.insert(i, row);
                    }
                }
                if rows.len() <= LEAF_MAX {
                    return Ok(Split {
                        left: s.add(Node::Leaf(rows))?,
                        right: None,
                    });
                }
                let right = rows.split_off(rows.len() / 2);
                let sep = right[0].key.clone();
                let left = s.add(Node::Leaf(rows))?;
                Ok(Split {
                    left,
                    right: Some((sep, s.add(Node::Leaf(right))?)),
                })
            }
            Node::Branch {
                mut keys,
                mut children,
            } => {
                let i = keys.partition_point(|k| k <= &m.key);
                let split = self.insert(s, children[i], m, depth + 1)?;
                children[i] = split.left;
                if let Some((sep, right)) = split.right {
                    keys.insert(i, sep);
                    children.insert(i + 1, right);
                }
                if keys.len() <= BRANCH_MAX {
                    return Ok(Split {
                        left: s.add(Node::Branch { keys, children })?,
                        right: None,
                    });
                }
                let mid = keys.len() / 2;
                let right_keys = keys.split_off(mid + 1);
                let sep = keys.pop().ok_or_else(bad)?;
                let right_children = children.split_off(mid + 1);
                let left = s.add(Node::Branch { keys, children })?;
                Ok(Split {
                    left,
                    right: Some((
                        sep,
                        s.add(Node::Branch {
                            keys: right_keys,
                            children: right_children,
                        })?,
                    )),
                })
            }
            Node::Overflow { .. } => Err(bad()),
        }
    }
    /// Atomically publish up to 16 distinct key mutations. Any admission
    /// error leaves the file unchanged; any I/O error after append poisons.
    pub fn commit(&mut self, ops: &[Mutation]) -> Result<Report> {
        self.commit_inner(ops, false, None)
    }

    /// Experimental B32: reuse only pages unreachable from the currently
    /// committed root. The ordinary append-only commit remains unchanged.
    pub fn commit_reusing(&mut self, ops: &[Mutation]) -> Result<Report> {
        self.commit_inner(ops, true, None)
    }

    /// Commit or replay a durable request. The key is scoped to this
    /// database and must not be reused for a different ordered payload.
    /// Receipts are never silently evicted; new keys fail at capacity.
    pub fn commit_receipted(&mut self, key: &str, ops: &[Mutation]) -> Result<Receipt> {
        if !valid_id(key) {
            return Err(bad());
        }
        Self::validate_ops(ops)?;
        self.verified_ownership()?;
        let key = hash(key.as_bytes());
        let request = request_digest(ops);
        if let Some(r) = self.receipts.iter().find(|r| r.key == key) {
            if r.request != request {
                return Err(Error::Conflict(
                    "cow tree: receipt key payload mismatch".into(),
                ));
            }
            return Ok(Receipt {
                generation: r.generation,
                replayed: true,
            });
        }
        if self.receipts.len() >= MAX_RECEIPTS {
            return Err(cap());
        }
        let generation = self.head.generation.checked_add(1).ok_or_else(cap)?;
        self.commit_inner(
            ops,
            true,
            Some(StoredReceipt {
                key,
                request,
                generation,
            }),
        )?;
        Ok(Receipt {
            generation,
            replayed: false,
        })
    }

    fn validate_ops(ops: &[Mutation]) -> Result<()> {
        if !(1..=16).contains(&ops.len()) {
            return Err(bad());
        }
        let mut keys = BTreeSet::new();
        for m in ops {
            if !valid_id(&m.key)
                || !keys.insert(&m.key)
                || m.value
                    .as_ref()
                    .is_some_and(|v| v.len() > VALUE_MAX || v.contains('\0'))
            {
                return Err(bad());
            }
        }
        Ok(())
    }

    fn commit_inner(
        &mut self,
        ops: &[Mutation],
        reuse: bool,
        receipt: Option<StoredReceipt>,
    ) -> Result<Report> {
        self.healthy()?;
        Self::validate_ops(ops)?;
        let ownership = if reuse || !self.receipts.is_empty() {
            Some(self.verified_ownership()?)
        } else {
            None
        };
        let mut s = Stage {
            head: self.head,
            base: self.head.pages,
            nodes: vec![],
            reuse,
        };
        s.head.generation = s.head.generation.checked_add(1).ok_or_else(cap)?;
        let mut height = self.height;
        for m in ops {
            let root = s.head.root;
            let split = self.insert(&mut s, root, m, 1)?;
            s.head.root = match split.right {
                None => split.left,
                Some((key, right)) => {
                    height += 1;
                    if height > MAX_DEPTH {
                        return Err(cap());
                    }
                    s.add(Node::Branch {
                        keys: vec![key],
                        children: vec![split.left, right],
                    })?
                }
            };
        }
        // Omit intermediate staged nodes superseded inside this batch.
        s.compact()?;
        // No persistent I/O until every mutation and page has been admitted.
        let pages = if let Some(owned) = ownership.filter(|_| reuse) {
            // The staged IDs are virtual and topologically ordered. Map
            // children first, picking only old unowned IDs above all links.
            // Preserve the existing descending-link disk format.
            let mut free: BTreeSet<u64> = (1..s.base).filter(|id| !owned.contains(id)).collect();
            let mut remap = BTreeMap::new();
            let mut high = s.base;
            let mut writes = Vec::with_capacity(s.nodes.len());
            for (i, mut node) in s.nodes.into_iter().enumerate() {
                let mut min = 0;
                let mut translate = |id: &mut u64| -> Result<()> {
                    if *id >= s.base {
                        *id = *remap.get(id).ok_or_else(bad)?;
                    } else if *id != 0 && !owned.contains(id) {
                        return Err(bad());
                    }
                    min = min.max(*id);
                    Ok(())
                };
                match &mut node {
                    Node::Leaf(rows) => {
                        for row in rows {
                            if let Some(StoredValue::Overflow { first, .. }) = &mut row.value {
                                translate(first)?;
                            }
                        }
                    }
                    Node::Branch { children, .. } => {
                        for child in children {
                            translate(child)?;
                        }
                    }
                    Node::Overflow { next, .. } => translate(next)?,
                }
                let id = if let Some(id) = free.range((min + 1)..).next().copied() {
                    free.remove(&id);
                    id
                } else {
                    if high >= MAX_PAGES || high <= min {
                        return Err(cap());
                    }
                    let id = high;
                    high += 1;
                    id
                };
                remap.insert(s.base + i as u64, id);
                writes.push((id, encode(&node, s.head, id)?));
            }
            s.head.root = *remap.get(&s.head.root).ok_or_else(bad)?;
            s.head.pages = high;
            writes
        } else {
            s.nodes
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let id = s.base + i as u64;
                    Ok((id, encode(n, s.head, id)?))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let mut receipts = self.receipts.clone();
        if let Some(r) = receipt {
            receipts.push(r);
        }
        let manifest = s.head.encode_manifest(&receipts)?;
        let hp = sibling(&self.path, ".head")?;
        let tmp = sibling(&self.path, ".head.tmp")?;
        if exists(&tmp)? || self.file.metadata()?.len() != s.base * SIZE as u64 {
            return Err(Error::Conflict("cow tree: unexpected tail or stage".into()));
        }
        // Stabilize the current head directory entry before overwriting any
        // page retired by an earlier published generation.
        if reuse {
            sync_dir(&self.path)?;
        }
        self.poisoned = true;
        for (id, p) in pages {
            self.file.seek(SeekFrom::Start(id * SIZE as u64))?;
            self.file.write_all(&p)?;
        }
        self.file.sync_all()?;
        crate::debug_fault("cow_after_pages", &self.file);
        let mut stage = new_file(&tmp)?;
        stage.write_all(&manifest)?;
        stage.sync_all()?;
        crate::debug_fault("cow_after_stage", &stage);
        fs::rename(&tmp, &hp)?;
        crate::debug_fault("cow_after_publish", &stage);
        sync_dir(&self.path)?;
        self.head = s.head;
        self.receipts = receipts;
        if reuse {
            // Reused physical IDs can still have cached older contents.
            self.cache.clear();
            self.fifo.clear();
        }
        self.height = height;
        self.poisoned = false;
        Ok(self.report())
    }
}
