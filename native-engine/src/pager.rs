//! B24: mutable fixed-page KV prototype with redo WAL. Separate from SQL.
use crate::{
    backup::same_file, private_file, private_parent, valid_id, Error, ExclusiveLock, NativeEntry,
    Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
const SIZE: usize = 4096;
const BODY: usize = SIZE - 48;
const MAGIC: &[u8; 8] = b"SEDRKV01";
const WAL_MAGIC: &[u8; 8] = b"SEDRWAL1";
const MAX_SLOTS: u32 = 1024;
const CACHE: usize = 32;
type Page = [u8; SIZE];
fn bad() -> Error {
    Error::Invalid("pager: invalid page or WAL")
}
fn hash(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}
fn page(id: u32, kind: u8, body: &[u8]) -> Result<Page> {
    if body.len() > BODY {
        return Err(Error::Capacity("pager: page payload"));
    }
    let mut p = [0; SIZE];
    p[..8].copy_from_slice(MAGIC);
    p[8..12].copy_from_slice(&id.to_be_bytes());
    p[12] = kind;
    p[13..15].copy_from_slice(&(body.len() as u16).to_be_bytes());
    p[16..16 + body.len()].copy_from_slice(body);
    let digest = hash(&p[..SIZE - 32]);
    p[SIZE - 32..].copy_from_slice(&digest);
    Ok(p)
}
fn decode(p: &Page, id: u32, kind: u8) -> Result<&[u8]> {
    let n = u16::from_be_bytes([p[13], p[14]]) as usize;
    if &p[..8] != MAGIC
        || p[8..12] != id.to_be_bytes()
        || p[12] != kind
        || p[15] != 0
        || n > BODY
        || p[16 + n..SIZE - 32].iter().any(|&v| v != 0)
        || hash(&p[..SIZE - 32]) != p[SIZE - 32..]
    {
        return Err(bad());
    }
    Ok(&p[16..16 + n])
}
fn read(f: &mut File, id: u32) -> Result<Page> {
    let mut p = [0; SIZE];
    f.seek(SeekFrom::Start(id as u64 * SIZE as u64))?;
    f.read_exact(&mut p)?;
    Ok(p)
}
fn write(f: &mut File, id: u32, p: &Page) -> Result<()> {
    f.seek(SeekFrom::Start(id as u64 * SIZE as u64))?;
    f.write_all(p)?;
    Ok(())
}
fn sibling(p: &Path, s: &str) -> Result<PathBuf> {
    let mut name = p.file_name().ok_or_else(bad)?.to_os_string();
    name.push(s);
    Ok(p.with_file_name(name))
}
fn sync_dir(p: &Path) -> Result<()> {
    File::open(p.parent().ok_or_else(bad)?)?.sync_all()?;
    Ok(())
}
fn exists(p: &Path) -> Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn new_file(p: &Path) -> Result<File> {
    let mut o = OpenOptions::new();
    o.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    Ok(o.open(p)?)
}
fn slot_for(ns: &str, key: &str, n: u32) -> u32 {
    let mut h = Sha256::new();
    h.update(ns.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    let d = h.finalize();
    u32::from_be_bytes(d[..4].try_into().unwrap()) % n
}
fn row(p: &Page, id: u32) -> Result<Option<NativeEntry>> {
    let b = decode(p, id, 2)?;
    if b.is_empty() {
        return Ok(None);
    }
    let r: NativeEntry = serde_json::from_slice(b).map_err(|_| bad())?;
    if !valid_id(&r.namespace)
        || !valid_id(&r.key)
        || r.revision == 0
        || r.value.as_ref().is_some_and(|v| v.contains('\0'))
    {
        return Err(bad());
    }
    Ok(Some(r))
}
fn generation(p: &Page) -> Result<u64> {
    let b = decode(p, 1, 1)?;
    Ok(u64::from_be_bytes(b.try_into().map_err(|_| bad())?))
}
fn header(f: &mut File) -> Result<(Page, u32)> {
    let p = read(f, 0)?;
    let b = decode(&p, 0, 0)?;
    if b.len() != 36 {
        return Err(bad());
    }
    let n = u32::from_be_bytes(b[..4].try_into().unwrap());
    if !(8..=MAX_SLOTS).contains(&n) || f.metadata()?.len() != u64::from(n + 2) * SIZE as u64 {
        return Err(bad());
    }
    Ok((p, n))
}
fn validate(f: &mut File, n: u32, overlay: &BTreeMap<u32, Page>) -> Result<u64> {
    let g = generation(&match overlay.get(&1) {
        Some(p) => *p,
        None => read(f, 1)?,
    })?;
    let mut slots = vec![];
    let mut seen = BTreeSet::new();
    for i in 0..n {
        let id = i + 2;
        let p = match overlay.get(&id) {
            Some(p) => *p,
            None => read(f, id)?,
        };
        let r = row(&p, id)?;
        if let Some(r) = &r {
            if r.revision > g || !seen.insert((r.namespace.clone(), r.key.clone())) {
                return Err(bad());
            }
        }
        slots.push(r);
    }
    for (i, r) in slots.iter().enumerate() {
        if let Some(r) = r {
            let start = slot_for(&r.namespace, &r.key, n);
            for j in 0..n {
                let k = ((start + j) % n) as usize;
                if k == i {
                    break;
                }
                if slots[k].is_none() {
                    return Err(bad());
                }
            }
        }
    }
    Ok(g)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub namespace: String,
    pub key: String,
    pub expected_revision: u64,
    pub value: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub generation: u64,
    pub changed_pages: usize,
    pub cache_hits: u64,
    pub disk_reads: u64,
}
pub struct Pager {
    file: File,
    path: PathBuf,
    _lock: ExclusiveLock,
    n: u32,
    header: Page,
    g: u64,
    cache: BTreeMap<u32, Page>,
    hits: u64,
    reads: u64,
    poisoned: bool,
}
fn wal_bytes(header: &Page, base: u64, changes: &BTreeMap<u32, Page>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(WAL_MAGIC);
    out.extend(hash(header));
    out.extend(base.to_be_bytes());
    out.extend((changes.len() as u32).to_be_bytes());
    for (id, p) in changes {
        out.extend(id.to_be_bytes());
        out.extend(p);
    }
    let digest = hash(&out);
    out.extend(digest);
    out
}
fn read_wal(path: &Path, h: &Page, n: u32) -> Result<(u64, BTreeMap<u32, Page>)> {
    private_file(path)?;
    let mut f = File::open(path)?;
    same_file(&f, path)?;
    let mut b = Vec::new();
    (&mut f)
        .take((84 + 17 * (SIZE + 4) + 1) as u64)
        .read_to_end(&mut b)?;
    if b.len() < 84
        || &b[..8] != WAL_MAGIC
        || b[8..40] != hash(h)
        || hash(&b[..b.len() - 32]) != b[b.len() - 32..]
    {
        return Err(bad());
    }
    let base = u64::from_be_bytes(b[40..48].try_into().unwrap());
    let count = u32::from_be_bytes(b[48..52].try_into().unwrap()) as usize;
    if !(2..=17).contains(&count) || b.len() != 84 + count * (SIZE + 4) {
        return Err(bad());
    }
    let mut changes = BTreeMap::new();
    for chunk in b[52..b.len() - 32].chunks_exact(SIZE + 4) {
        let id = u32::from_be_bytes(chunk[..4].try_into().unwrap());
        let p: Page = chunk[4..].try_into().unwrap();
        if id == 0 || id >= n + 2 || changes.insert(id, p).is_some() {
            return Err(bad());
        }
        if id == 1 {
            if generation(&p)? != base.checked_add(1).ok_or_else(bad)? {
                return Err(bad());
            }
        } else if row(&p, id)?.is_none() {
            return Err(bad());
        }
    }
    if !changes.contains_key(&1) {
        return Err(bad());
    }
    Ok((base, changes))
}
impl Pager {
    pub fn create(path: &Path, n: u32) -> Result<Self> {
        private_parent(path)?;
        if !(8..=MAX_SLOTS).contains(&n)
            || exists(path)?
            || exists(&sibling(path, ".wal")?)?
            || exists(&sibling(path, ".wal.tmp")?)?
        {
            return Err(bad());
        }
        let _lock = ExclusiveLock::acquire(path)?;
        let mut f = new_file(path)?;
        let mut b = [0; 36];
        b[..4].copy_from_slice(&n.to_be_bytes());
        File::open("/dev/urandom")?.read_exact(&mut b[4..])?;
        let h = page(0, 0, &b)?;
        write(&mut f, 0, &h)?;
        write(&mut f, 1, &page(1, 1, &0u64.to_be_bytes())?)?;
        for id in 2..n + 2 {
            write(&mut f, id, &page(id, 2, &[])?)?;
        }
        f.sync_all()?;
        sync_dir(path)?;
        Ok(Self {
            file: f,
            path: path.into(),
            _lock,
            n,
            header: h,
            g: 0,
            cache: BTreeMap::new(),
            hits: 0,
            reads: 0,
            poisoned: false,
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        private_parent(path)?;
        private_file(path)?;
        let _lock = ExclusiveLock::acquire(path)?;
        let mut f = OpenOptions::new().read(true).write(true).open(path)?;
        same_file(&f, path)?;
        let (h, n) = header(&mut f)?;
        let wal = sibling(path, ".wal")?;
        let tmp = sibling(path, ".wal.tmp")?;
        if exists(&tmp)? {
            return Err(Error::Conflict(
                "pager: staging WAL requires operator review".into(),
            ));
        }
        if exists(&wal)? {
            let (base, changes) = read_wal(&wal, &h, n)?;
            // A torn generation page is allowed only with a complete valid WAL.
            if let Ok(g) = generation(&read(&mut f, 1)?) {
                if g != base && g != base + 1 {
                    return Err(bad());
                }
            }
            validate(&mut f, n, &changes)?;
            for (id, p) in &changes {
                write(&mut f, *id, p)?;
            }
            f.sync_all()?;
            fs::remove_file(&wal)?;
            sync_dir(path)?;
        }
        let g = validate(&mut f, n, &BTreeMap::new())?;
        Ok(Self {
            file: f,
            path: path.into(),
            _lock,
            n,
            header: h,
            g,
            cache: BTreeMap::new(),
            hits: 0,
            reads: 0,
            poisoned: false,
        })
    }
    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Conflict(
                "pager: uncertain write; reopen required".into(),
            ));
        }
        same_file(&self.file, &self.path)
    }
    fn cached(&mut self, id: u32) -> Result<Page> {
        if let Some(p) = self.cache.get(&id) {
            self.hits += 1;
            return Ok(*p);
        }
        let p = read(&mut self.file, id)?;
        decode(&p, id, 2)?;
        self.reads += 1;
        if self.cache.len() >= CACHE {
            let key = *self.cache.keys().next().unwrap();
            self.cache.remove(&key);
        }
        self.cache.insert(id, p);
        Ok(p)
    }
    fn find(
        &mut self,
        ns: &str,
        key: &str,
        overlay: &BTreeMap<u32, Page>,
    ) -> Result<(u32, Option<NativeEntry>)> {
        if !valid_id(ns) || !valid_id(key) {
            return Err(bad());
        }
        let start = slot_for(ns, key, self.n);
        for j in 0..self.n {
            let id = 2 + (start + j) % self.n;
            let p = match overlay.get(&id) {
                Some(p) => *p,
                None => self.cached(id)?,
            };
            let r = row(&p, id)?;
            if r.as_ref().is_none_or(|r| r.namespace == ns && r.key == key) {
                return Ok((id, r));
            }
        }
        Err(Error::Capacity("pager: all slots occupied"))
    }
    pub fn get(&mut self, ns: &str, key: &str) -> Result<Option<NativeEntry>> {
        self.healthy()?;
        match self.find(ns, key, &BTreeMap::new()) {
            Ok((_, r)) => Ok(r),
            Err(Error::Capacity(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn report(&self) -> Report {
        Report {
            generation: self.g,
            changed_pages: 0,
            cache_hits: self.hits,
            disk_reads: self.reads,
        }
    }
    pub fn commit(&mut self, changes: &[Change]) -> Result<Report> {
        self.healthy()?;
        if changes.is_empty() || changes.len() > 16 {
            return Err(bad());
        }
        let next = self.g.checked_add(1).ok_or_else(bad)?;
        let mut overlay = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for c in changes {
            if !seen.insert((&c.namespace, &c.key)) {
                return Err(bad());
            }
            let (id, old) = self.find(&c.namespace, &c.key, &overlay)?;
            let rev = old.as_ref().map_or(0, |r| r.revision);
            if rev != c.expected_revision
                || c.value.is_none() && old.as_ref().is_none_or(|r| r.value.is_none())
            {
                return Err(Error::Conflict(
                    "pager: revision or deletion conflict".into(),
                ));
            }
            if c.value.as_ref().is_some_and(|v| v.contains('\0')) {
                return Err(bad());
            }
            let r = NativeEntry {
                namespace: c.namespace.clone(),
                key: c.key.clone(),
                value: c.value.clone(),
                revision: rev.checked_add(1).ok_or_else(bad)?,
            };
            overlay.insert(
                id,
                page(id, 2, &serde_json::to_vec(&r).map_err(|_| bad())?)?,
            );
        }
        overlay.insert(1, page(1, 1, &next.to_be_bytes())?);
        let wal = sibling(&self.path, ".wal")?;
        let tmp = sibling(&self.path, ".wal.tmp")?;
        if exists(&wal)? || exists(&tmp)? {
            return Err(Error::Conflict("pager: WAL exists".into()));
        }
        let bytes = wal_bytes(&self.header, self.g, &overlay);
        self.poisoned = true;
        let mut stage = new_file(&tmp)?;
        stage.write_all(&bytes)?;
        stage.sync_all()?;
        fs::rename(&tmp, &wal)?;
        sync_dir(&self.path)?;
        crate::debug_fault("pager_after_wal", &stage);
        for (i, (id, p)) in overlay.iter().enumerate() {
            write(&mut self.file, *id, p)?;
            if i == 0 {
                crate::debug_fault("pager_after_first_page", &self.file);
            }
        }
        self.file.sync_all()?;
        crate::debug_fault("pager_after_data", &self.file);
        fs::remove_file(&wal)?;
        sync_dir(&self.path)?;
        self.g = next;
        self.cache.clear();
        self.poisoned = false;
        Ok(Report {
            changed_pages: overlay.len(),
            ..self.report()
        })
    }
}
