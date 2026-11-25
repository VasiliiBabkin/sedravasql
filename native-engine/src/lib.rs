//! Experimental SQLite-free, single-writer append-only transaction log.
//!
//! This is a native *storage engine prototype*, not a public DB or a safe
//! replacement for the existing SQLite backend. It uses one checksummed frame
//! per atomic in-memory batch. Commit is acknowledged only after sync_all.
//! Torn/corrupt frames always fail closed; B1 recovery-prefix copying and
//! B2 offline backup, B3 snapshot and B4 operator-approved offline restore
//! are opt-in experiments, not production recovery.
//! A stale .lock after a crash must be handled by an operator.
//! Do not store irreplaceable data here.
pub mod archive;
pub mod backup;
#[cfg(unix)]
pub mod cow_migration;
#[cfg(unix)]
pub mod cow_tree;
pub mod index;
pub mod pager;
pub mod pages;
pub mod recovery;
pub mod restore;
pub mod snapshot;
pub mod sql;
pub mod table;
pub mod tree;

use std::error::Error as StdError;
use std::fmt;

pub const MAX_VALUE_BYTES: usize = 65_536;

/// Native-only error taxonomy; does not depend on SQLite or rusqlite.
#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Capacity(&'static str),
    Conflict(String),
    Unsupported(String),
    Io(std::io::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "INVALID_ARGUMENT: {s}"),
            Self::Capacity(s) => write!(f, "CAPACITY_EXCEEDED: {s}"),
            Self::Conflict(s) => write!(f, "CONFLICT: {s}"),
            Self::Unsupported(s) => write!(f, "UNSUPPORTED: {s}"),
            Self::Io(s) => write!(f, "IO_ERROR: {s}"),
        }
    }
}
impl StdError for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"SEDRLOG1";
const FRAME_LIMIT: usize = 1_100_000;
pub const MAX_NATIVE_BATCH: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value")]
pub enum NativeChange {
    Put(String),
    Delete,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeMutation {
    pub namespace: String,
    pub key: String,
    pub expected_revision: u64,
    pub change: NativeChange,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeEntry {
    pub namespace: String,
    pub key: String,
    /// None is a tombstone; revision is intentionally preserved.
    pub value: Option<String>,
    pub revision: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeCommit {
    pub sequence: u64,
    pub entries: Vec<NativeEntry>,
    pub replayed: bool,
}

/// B13: coherent view of one committed generation on an exclusive handle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeReadSnapshot {
    pub sequence: u64,
    pub rows: Vec<Option<NativeEntry>>,
}

/// B15a: write-admission ceilings, not a memory or filesystem quota.
pub const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_LOG_TRANSACTIONS: u64 = 65_536;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeLimits {
    pub max_bytes: u64,
    pub max_transactions: u64,
}
impl Default for NativeLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_LOG_BYTES,
            max_transactions: MAX_LOG_TRANSACTIONS,
        }
    }
}
impl NativeLimits {
    pub fn validate(self) -> Result<Self> {
        if !(8..=MAX_LOG_BYTES).contains(&self.max_bytes)
            || !(1..=MAX_LOG_TRANSACTIONS).contains(&self.max_transactions)
        {
            return Err(Error::Invalid("native log: invalid write limits"));
        }
        Ok(self)
    }
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NativeUsage {
    pub log_bytes: u64,
    pub transactions: u64,
    pub keys_including_tombstones: usize,
    pub receipts: usize,
    pub limits: NativeLimits,
    pub remaining_bytes: u64,
    pub remaining_transactions: u64,
}
/// Minimal backend-neutral *target* API, not yet implemented by SQLite.
/// This interface is unstable and not exposed through Sedrava DSL.
pub trait TransactionalStore {
    fn read(&self, namespace: &str, key: &str) -> Result<Option<NativeEntry>>;
    fn commit_batch(&mut self, request_key: &str, ops: &[NativeMutation]) -> Result<NativeCommit>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    seq: u64,
    request_key: String,
    operations: Vec<NativeMutation>,
}

struct Replay {
    seq: u64,
    entries: BTreeMap<(String, String), NativeEntry>,
    receipts: HashMap<String, (String, NativeCommit)>,
    hash: [u8; 32],
}
impl Default for Replay {
    fn default() -> Self {
        Self {
            seq: 0,
            entries: BTreeMap::new(),
            receipts: HashMap::new(),
            hash: Sha256::digest(MAGIC).into(),
        }
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Shared experimental request validation; keep both storage backends aligned.
pub fn validate_batch(request_key: &str, ops: &[NativeMutation]) -> Result<()> {
    if request_key.len() < 12 || !valid_id(request_key) {
        return Err(Error::Invalid("native log: invalid idempotency key"));
    }
    if ops.is_empty() || ops.len() > MAX_NATIVE_BATCH {
        return Err(Error::Invalid(
            "native log: batch must contain 1..16 operations",
        ));
    }
    let mut seen = HashSet::new();
    for op in ops {
        if !valid_id(&op.namespace) || !valid_id(&op.key) || !seen.insert((&op.namespace, &op.key))
        {
            return Err(Error::Invalid(
                "native log: invalid or repeated namespace/key",
            ));
        }
        if op.expected_revision >= i64::MAX as u64 {
            return Err(Error::Invalid("native log: revision overflow"));
        }
        if let NativeChange::Put(value) = &op.change {
            if value.len() > MAX_VALUE_BYTES || value.contains('\0') {
                return Err(Error::Invalid(
                    "native log: value too large or contains NUL",
                ));
            }
        }
    }
    Ok(())
}

/// Shared experimental canonical fingerprint (also used by the SQLite batch adapter).
/// Input order is significant and deliberately not normalized.
pub fn batch_fingerprint(ops: &[NativeMutation]) -> Result<String> {
    let encoded = serde_json::to_vec(&("sedravasql.native-batch.v1", ops))
        .map_err(|_| Error::Invalid("native log: encoding failed"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

// B14: stage only the changed rows. No clone of accumulated state/history.
struct PreparedBatch {
    request_key: String,
    fingerprint: String,
    result: NativeCommit,
}

fn prepare(state: &Replay, request_key: &str, ops: &[NativeMutation]) -> Result<PreparedBatch> {
    validate_batch(request_key, ops)?;
    if state.receipts.contains_key(request_key) {
        return Err(Error::Unsupported(
            "native log: duplicate stored request in frame".into(),
        ));
    }
    let seq = state
        .seq
        .checked_add(1)
        .ok_or(Error::Invalid("native log: sequence overflow"))?;
    let mut entries = Vec::with_capacity(ops.len());
    // Validate and compute the entire batch without making any persistent change.
    for op in ops {
        let address = (op.namespace.clone(), op.key.clone());
        let prev = state.entries.get(&address);
        let revision = prev.map_or(0, |row| row.revision);
        if revision != op.expected_revision {
            return Err(Error::Conflict(format!(
                "native log: revision mismatch at {}/{} (expected {}, actual {})",
                op.namespace, op.key, op.expected_revision, revision
            )));
        }
        if matches!(op.change, NativeChange::Delete) && prev.is_none_or(|row| row.value.is_none()) {
            return Err(Error::Conflict(
                "native log: cannot delete absent row".into(),
            ));
        }
        let row = NativeEntry {
            namespace: op.namespace.clone(),
            key: op.key.clone(),
            value: match &op.change {
                NativeChange::Put(v) => Some(v.clone()),
                NativeChange::Delete => None,
            },
            revision: revision + 1,
        };
        entries.push(row);
    }
    table::validate_constraints(state, &entries)?;
    Ok(PreparedBatch {
        request_key: request_key.to_owned(),
        fingerprint: batch_fingerprint(ops)?,
        result: NativeCommit {
            sequence: seq,
            entries,
            replayed: false,
        },
    })
}

// Infallible logical publication, called only after validation and, for
// live commits, durable file sync. Allocation failure is process-fatal.
fn apply(state: &mut Replay, pending: PreparedBatch, digest: [u8; 32]) {
    state.seq = pending.result.sequence;
    state.hash = digest;
    for row in &pending.result.entries {
        state
            .entries
            .insert((row.namespace.clone(), row.key.clone()), row.clone());
    }
    state
        .receipts
        .insert(pending.request_key, (pending.fingerprint, pending.result));
}

fn frame_digest(prev: &[u8; 32], len: &[u8; 4], payload: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(prev);
    hash.update(len);
    hash.update(payload);
    hash.finalize().into()
}

fn fail_corrupt(reason: &'static str) -> Error {
    Error::Unsupported(format!(
        "native log: corrupt or torn frame ({reason}); no automatic repair"
    ))
}

fn load(file: &mut File) -> Result<Replay> {
    load_reader(file)
}
fn load_reader(file: &mut (impl Read + Seek)) -> Result<Replay> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|_| fail_corrupt("missing header"))?;
    if &magic != MAGIC {
        return Err(fail_corrupt("format signature"));
    }
    let mut state = Replay::default();
    loop {
        let mut len = [0u8; 4];
        if file.read(&mut len[..1])? == 0 {
            break;
        }
        file.read_exact(&mut len[1..])
            .map_err(|_| fail_corrupt("truncated length"))?;
        let size = u32::from_be_bytes(len) as usize;
        if size == 0 || size > FRAME_LIMIT {
            return Err(fail_corrupt("invalid record size"));
        }
        let mut payload = vec![0u8; size];
        file.read_exact(&mut payload)
            .map_err(|_| fail_corrupt("truncated payload"))?;
        let mut digest = [0u8; 32];
        file.read_exact(&mut digest)
            .map_err(|_| fail_corrupt("truncated checksum"))?;
        if frame_digest(&state.hash, &len, &payload) != digest {
            return Err(fail_corrupt("checksum mismatch"));
        }
        let record: Frame =
            serde_json::from_slice(&payload).map_err(|_| fail_corrupt("record format"))?;
        if record.seq != state.seq + 1 {
            return Err(fail_corrupt("nonsequential record"));
        }
        let pending = prepare(&state, &record.request_key, &record.operations)
            .map_err(|_| fail_corrupt("invalid recorded transaction"))?;
        apply(&mut state, pending, digest);
    }
    file.seek(SeekFrom::End(0))?;
    Ok(state)
}

fn private_file(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(Error::Invalid("native log: path must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid(
                "native log: file must be private and single-link",
            ));
        }
    }
    Ok(())
}

fn private_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or(Error::Invalid("native log: missing parent"))?;
    let meta = fs::symlink_metadata(parent)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::Invalid(
            "native log: parent must be a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid(
                "native log: parent directory must be private (0700)",
            ));
        }
    }
    Ok(())
}

struct ExclusiveLock {
    path: PathBuf,
    file: File,
}
impl ExclusiveLock {
    fn acquire(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .ok_or(Error::Invalid("native log: missing filename"))?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".lock");
        let lock_path = path.with_file_name(lock_name);
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&lock_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Conflict(
                    "native log: exclusive lock exists (manual stale-lock review required)".into(),
                )
            } else {
                Error::Io(e)
            }
        })?;
        file.write_all(format!("pid={}\n", std::process::id()).as_bytes())?;
        file.sync_all()?;
        Ok(Self {
            path: lock_path,
            file,
        })
    }
}
impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        // Do not remove a lock that somebody else replaced.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(mine), Ok(current)) =
                (self.file.metadata(), fs::symlink_metadata(&self.path))
            {
                if current.is_file()
                    && !current.file_type().is_symlink()
                    && mine.ino() == current.ino()
                    && mine.dev() == current.dev()
                {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }
}

/// This is experimental, Unix-only and *not* production-ready. There is no
/// automatic stale-lock removal, compaction, rollback after torn writes, full
/// crash matrix, or backup policy. Existing SQLite databases are unaffected.
pub struct NativeLog {
    file: File,
    _lock: ExclusiveLock,
    state: Replay,
    poisoned: bool,
    log_bytes: u64,
    limits: NativeLimits,
    secondary_cache: std::cell::RefCell<table::IndexCache>,
}
impl NativeLog {
    pub fn create(path: &Path) -> Result<Self> {
        #[cfg(not(unix))]
        return Err(Error::Unsupported(
            "native log not validated on non-Unix targets".into(),
        ));
        private_parent(path)?;
        if path.exists() || fs::symlink_metadata(path).is_ok() {
            return Err(Error::Invalid(
                "native log: refusing to overwrite existing path",
            ));
        }
        let lock = ExclusiveLock::acquire(path)?;
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(path)?;
        file.write_all(MAGIC)?;
        file.sync_all()?;
        // Creating a directory entry requires syncing its containing directory.
        File::open(
            path.parent()
                .ok_or(Error::Invalid("native log: missing parent"))?,
        )?
        .sync_all()?;
        private_file(path)?;
        let state = load(&mut file)?;
        let log_bytes = file.stream_position()?;
        Ok(Self {
            file,
            _lock: lock,
            state,
            poisoned: false,
            log_bytes,
            limits: NativeLimits::default(),
            secondary_cache: Default::default(),
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        #[cfg(not(unix))]
        return Err(Error::Unsupported(
            "native log not validated on non-Unix targets".into(),
        ));
        private_parent(path)?;
        private_file(path)?;
        let lock = ExclusiveLock::acquire(path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let state = load(&mut file)?;
        let log_bytes = file.stream_position()?;
        Ok(Self {
            file,
            _lock: lock,
            state,
            poisoned: false,
            log_bytes,
            limits: NativeLimits::default(),
            secondary_cache: Default::default(),
        })
    }
    pub fn verify(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        let from_disk = match load(&mut self.file) {
            Ok(state) => state,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        if from_disk.seq != self.state.seq
            || from_disk.hash != self.state.hash
            || self.file.stream_position()? != self.log_bytes
        {
            self.poisoned = true;
            return Err(fail_corrupt("in-memory and disk state diverged"));
        }
        Ok(())
    }
    /// Limits are per handle; operators must reapply custom limits on reopen.
    /// Historical data remains readable/replayable even above a lower ceiling.
    pub fn set_limits(&mut self, limits: NativeLimits) -> Result<()> {
        self.limits = limits.validate()?;
        Ok(())
    }
    pub fn usage(&self) -> Result<NativeUsage> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        Ok(NativeUsage {
            log_bytes: self.log_bytes,
            transactions: self.state.seq,
            keys_including_tombstones: self.state.entries.len(),
            receipts: self.state.receipts.len(),
            limits: self.limits,
            remaining_bytes: self.limits.max_bytes.saturating_sub(self.log_bytes),
            remaining_transactions: self.limits.max_transactions.saturating_sub(self.state.seq),
        })
    }
    /// Read 1..16 addresses in input order from one committed Replay.
    /// Duplicates are preserved, missing rows are None, tombstones are rows.
    /// No external process may write while this exclusive handle is open.
    pub fn read_snapshot(&self, keys: &[(&str, &str)]) -> Result<NativeReadSnapshot> {
        if keys.is_empty()
            || keys.len() > MAX_NATIVE_BATCH
            || keys.iter().any(|(ns, key)| !valid_id(ns) || !valid_id(key))
        {
            return Err(Error::Invalid("native log: invalid snapshot key list"));
        }
        let rows = keys
            .iter()
            .map(|(ns, key)| self.read(ns, key))
            .collect::<Result<Vec<_>>>()?;
        Ok(NativeReadSnapshot {
            sequence: self.state.seq,
            rows,
        })
    }
    /// B16a: bounded ordered scan on a single exclusive committed generation.
    pub fn scan_namespace(
        &self,
        namespace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NativeEntry>> {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        if !valid_id(namespace)
            || after.is_some_and(|k| !valid_id(k))
            || !(1..=256).contains(&limit)
        {
            return Err(Error::Invalid("native log: invalid scan bounds"));
        }
        let start = match after {
            Some(k) => Excluded((namespace.to_owned(), k.to_owned())),
            None => Included((namespace.to_owned(), String::new())),
        };
        Ok(self
            .state
            .entries
            .range((start, Unbounded))
            .take_while(|((ns, _), _)| ns == namespace)
            .take(limit)
            .map(|(_, r)| r.clone())
            .collect())
    }
    /// B28-B: bounded primary-key range on one committed Replay.
    /// Half-open [start,end), exclusive optional continuation; includes tombstones.
    pub(crate) fn scan_namespace_range(
        &self,
        namespace: &str,
        start: &str,
        end: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NativeEntry>> {
        use std::ops::Bound::{Excluded, Included};
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        if !valid_id(namespace)
            || !valid_id(start)
            || !valid_id(end)
            || after.is_some_and(|k| !valid_id(k) || k < start || k >= end)
            || !(1..=256).contains(&limit)
        {
            return Err(Error::Invalid("native log: invalid range bounds"));
        }
        if start >= end {
            return Ok(vec![]);
        }
        let lower = match after {
            Some(k) => Excluded((namespace.to_owned(), k.to_owned())),
            None => Included((namespace.to_owned(), start.to_owned())),
        };
        let upper = Excluded((namespace.to_owned(), end.to_owned()));
        Ok(self
            .state
            .entries
            .range((lower, upper))
            .take(limit)
            .map(|(_, r)| r.clone())
            .collect())
    }
    /// Return the original durable engine receipt without re-executing a batch.
    pub fn lookup_receipt(&self, request_key: &str) -> Result<Option<NativeCommit>> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        if request_key.len() < 12 || !valid_id(request_key) {
            return Err(Error::Invalid("native log: invalid idempotency key"));
        }
        Ok(self
            .state
            .receipts
            .get(request_key)
            .map(|(_, receipt)| receipt.clone()))
    }
    pub fn sequence(&self) -> u64 {
        self.state.seq
    }
}
impl TransactionalStore for NativeLog {
    fn read(&self, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        if !valid_id(namespace) || !valid_id(key) {
            return Err(Error::Invalid("native log: invalid namespace/key"));
        }
        Ok(self
            .state
            .entries
            .get(&(namespace.to_owned(), key.to_owned()))
            .cloned())
    }
    fn commit_batch(&mut self, request_key: &str, ops: &[NativeMutation]) -> Result<NativeCommit> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "native log: previous write uncertain".into(),
            ));
        }
        validate_batch(request_key, ops)?;
        let fp = batch_fingerprint(ops)?;
        if let Some((stored, receipt)) = self.state.receipts.get(request_key) {
            if stored != &fp {
                return Err(Error::Conflict(
                    "native log: idempotency key reused with different request".into(),
                ));
            }
            let mut replay = receipt.clone();
            replay.replayed = true;
            return Ok(replay);
        }
        let pending = prepare(&self.state, request_key, ops)?;
        let record = Frame {
            seq: pending.result.sequence,
            request_key: request_key.to_owned(),
            operations: ops.to_vec(),
        };
        let payload = serde_json::to_vec(&record)
            .map_err(|_| Error::Invalid("native log: cannot serialize record"))?;
        if payload.is_empty() || payload.len() > FRAME_LIMIT {
            return Err(Error::Invalid("native log: oversized frame"));
        }
        // Exact encoded frame size, including 4-byte length and 32-byte digest.
        // Replays are handled above, even when the journal is at capacity.
        let projected = self
            .log_bytes
            .checked_add(payload.len() as u64 + 36)
            .ok_or(Error::Capacity("native log: byte counter overflow"))?;
        if pending.result.sequence > self.limits.max_transactions {
            return Err(Error::Capacity("native log: transaction ceiling"));
        }
        if projected > self.limits.max_bytes {
            return Err(Error::Capacity("native log: byte ceiling"));
        }
        if self.file.metadata()?.len() != self.log_bytes {
            self.poisoned = true;
            return Err(fail_corrupt(
                "journal length changed outside exclusive owner",
            ));
        }
        let len = (payload.len() as u32).to_be_bytes();
        let checksum = frame_digest(&self.state.hash, &len, &payload);
        // A complete frame is the atomic recovery unit; never update memory
        // until the whole frame has been written and sync_all() succeeded.
        let persistence = (|| -> Result<()> {
            self.file.write_all(&len)?;
            debug_fault("after_length", &self.file);
            self.file.write_all(&payload)?;
            debug_fault("after_payload", &self.file);
            self.file.write_all(&checksum)?;
            self.file.sync_all()?;
            debug_fault("after_sync", &self.file);
            Ok(())
        })();
        if let Err(err) = persistence {
            self.poisoned = true; // Commit status is uncertain; do not retry on this handle.
            return Err(err);
        }
        let result = pending.result.clone();
        apply(&mut self.state, pending, checksum);
        // Cached indexes are derived data, never an independent durable state.
        // Invalidate only indexes whose table/catalog was modified, after fsync.
        self.secondary_cache.get_mut().retain(|(table, _), _| {
            let ns = format!("sedravasql_table.{table}");
            let catalog = format!("sedravasql_index_catalog.{table}");
            !result
                .entries
                .iter()
                .any(|e| e.namespace == ns || e.namespace == catalog)
        });
        self.log_bytes = projected;
        Ok(result)
    }
}

/// Debug-only crash injection for disposable test files. Never enabled in
/// release builds. after_length and after_payload create synced *torn*
/// final frames with stale locks. after_sync simulates a lost HTTP response
/// after a complete durable transaction. This is NOT hardware power-loss.
#[cfg(debug_assertions)]
fn debug_fault(point: &str, file: &File) {
    if std::env::var("SEDRAVASQL_NATIVE_TEST_FAULT")
        .ok()
        .as_deref()
        == Some(point)
    {
        let _ = file.sync_all();
        eprintln!("SEDRAVASQL_NATIVE_TEST_FAULT:{point}");
        std::process::exit(86);
    }
}
#[cfg(not(debug_assertions))]
fn debug_fault(_point: &str, _file: &File) {}
