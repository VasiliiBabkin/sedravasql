//! B35a: experimental transactional store on COW pages.
//! Canonical ordered frames are persisted atomically; Replay is derived in RAM.
//! Not a direct on-disk row index and not the B34 read-only migration format.
use super::cow_tree::{CowTree, Mutation, Record};
use super::*;
const TAG: &str = "sedravasql-cow-store-v1";
const CHUNK: usize = 60 * 1024;
const MAX_FRAME_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PHYSICAL_ROWS: u64 = 32_768;

fn bad() -> Error {
    Error::Invalid("cow store: invalid frame history")
}
fn cap() -> Error {
    Error::Capacity("cow store: frame history budget")
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn base(seq: u64) -> String {
    format!("f-{seq:020}")
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    chunks: usize,
    bytes: usize,
    chain: String,
}

fn frame_parts(frame: &Frame, prev: &[u8; 32]) -> Result<(Vec<Mutation>, [u8; 32], u64)> {
    let payload = serde_json::to_string(frame).map_err(|_| bad())?;
    if payload.is_empty() || payload.len() > FRAME_LIMIT {
        return Err(cap());
    }
    let len = (payload.len() as u32).to_be_bytes();
    let digest = frame_digest(prev, &len, payload.as_bytes());
    let key = base(frame.seq);
    let mut out = Vec::new();
    let mut start = 0;
    while start < payload.len() {
        let mut end = (start + CHUNK).min(payload.len());
        while !payload.is_char_boundary(end) {
            end -= 1;
        }
        out.push(Mutation {
            key: format!("{key}-{:04}", out.len()),
            expected_revision: 0,
            value: Some(payload[start..end].to_owned()),
        });
        start = end;
    }
    let desc = Descriptor {
        chunks: out.len(),
        bytes: payload.len(),
        chain: hex(&digest),
    };
    out.push(Mutation {
        key,
        expected_revision: 0,
        value: Some(serde_json::to_string(&desc).map_err(|_| bad())?),
    });
    let size = out
        .iter()
        .map(|m| m.value.as_ref().unwrap().len() as u64)
        .sum();
    Ok((out, digest, size))
}

fn read_history(tree: &mut CowTree) -> Result<(Replay, u64)> {
    let rows = tree.snapshot_rows()?;
    let mut rows = rows.into_iter();
    let mut state = Replay::default();
    let mut bytes = TAG.len() as u64;
    // Lexical order places f-* before the final format marker.
    let mut next = rows.next().ok_or_else(bad)?;
    while next.key != "format" {
        let sequence = state.seq.checked_add(1).ok_or_else(cap)?;
        if sequence > MAX_LOG_TRANSACTIONS || next.key != base(sequence) || next.revision != 1 {
            return Err(bad());
        }
        let desc: Descriptor =
            serde_json::from_str(next.value.as_deref().ok_or_else(bad)?).map_err(|_| bad())?;
        if desc.chunks == 0
            || desc.chunks > FRAME_LIMIT.div_ceil(CHUNK) + 1
            || desc.bytes == 0
            || desc.bytes > FRAME_LIMIT
        {
            return Err(bad());
        }
        bytes = bytes
            .checked_add(next.value.as_ref().unwrap().len() as u64)
            .ok_or_else(cap)?;
        let mut payload = String::with_capacity(desc.bytes);
        for i in 0..desc.chunks {
            let row = rows.next().ok_or_else(bad)?;
            let value = row.value.ok_or_else(bad)?;
            if row.key != format!("{}-{i:04}", base(sequence))
                || row.revision != 1
                || value.is_empty()
                || value.len() > CHUNK
                || payload.len().checked_add(value.len()).ok_or_else(cap)? > desc.bytes
            {
                return Err(bad());
            }
            payload.push_str(&value);
        }
        if payload.len() != desc.bytes {
            return Err(bad());
        }
        bytes = bytes.checked_add(payload.len() as u64).ok_or_else(cap)?;
        if bytes > MAX_FRAME_BYTES {
            return Err(cap());
        }
        let frame: Frame = serde_json::from_str(&payload).map_err(|_| bad())?;
        if frame.seq != sequence {
            return Err(bad());
        }
        let pending = prepare(&state, &frame.request_key, &frame.operations)?;
        let (canonical, digest, _) = frame_parts(&frame, &state.hash)?;
        // Require exact canonical fragment boundaries and descriptor bytes.
        let expected = canonical
            .last()
            .and_then(|m| m.value.as_deref())
            .ok_or_else(bad)?;
        if next.value.as_deref() != Some(expected)
            || hex(&digest) != desc.chain
            || serde_json::to_string(&frame).map_err(|_| bad())? != payload
        {
            return Err(bad());
        }
        apply(&mut state, pending, digest);
        next = rows.next().ok_or_else(bad)?;
    }
    if next.revision != 1
        || next.value.as_deref() != Some(TAG)
        || rows.next().is_some()
        || tree.report().generation != state.seq + 1
    {
        return Err(bad());
    }
    Ok((state, bytes))
}

pub struct CowStore {
    tree: CowTree,
    state: Replay,
    payload_bytes: u64,
    poisoned: bool,
    pub(crate) secondary_cache: std::cell::RefCell<table::IndexCache>,
}
impl CowStore {
    pub fn create(path: &Path) -> Result<Self> {
        CowTree::create_snapshot(
            path,
            &[Record {
                key: "format".into(),
                revision: 1,
                value: Some(TAG.into()),
            }],
            1,
        )?;
        Self::open(path)
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut tree = CowTree::open(path)?;
        let (state, payload_bytes) = read_history(&mut tree)?;
        Ok(Self {
            tree,
            state,
            payload_bytes,
            poisoned: false,
            secondary_cache: Default::default(),
        })
    }
    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            Err(Error::Unsupported(
                "cow store: previous operation uncertain; reopen".into(),
            ))
        } else {
            Ok(())
        }
    }
    pub fn verify(&mut self) -> Result<()> {
        self.healthy()?;
        let r = read_history(&mut self.tree);
        match r {
            Ok((state, bytes))
                if state.seq == self.state.seq
                    && state.hash == self.state.hash
                    && bytes == self.payload_bytes =>
            {
                Ok(())
            }
            Ok(_) => {
                self.poisoned = true;
                Err(bad())
            }
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }
    pub fn sequence(&self) -> u64 {
        self.state.seq
    }
    pub fn report(&self) -> super::cow_tree::Report {
        self.tree.report()
    }
    pub fn lookup_receipt(&self, key: &str) -> Result<Option<NativeCommit>> {
        self.healthy()?;
        if key.len() < 12 || !valid_id(key) {
            return Err(bad());
        }
        Ok(self.state.receipts.get(key).map(|(_, r)| r.clone()))
    }
    pub fn scan_namespace(
        &self,
        namespace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NativeEntry>> {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        self.healthy()?;
        if !valid_id(namespace)
            || after.is_some_and(|k| !valid_id(k))
            || !(1..=256).contains(&limit)
        {
            return Err(bad());
        }
        let lower = match after {
            Some(k) => Excluded((namespace.to_owned(), k.to_owned())),
            None => Included((namespace.to_owned(), String::new())),
        };
        Ok(self
            .state
            .entries
            .range((lower, Unbounded))
            .take_while(|((ns, _), _)| ns == namespace)
            .take(limit)
            .map(|(_, r)| r.clone())
            .collect())
    }
    pub fn scan_namespace_range(
        &self,
        namespace: &str,
        start: &str,
        end: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NativeEntry>> {
        use std::ops::Bound::{Excluded, Included};
        self.healthy()?;
        if !valid_id(namespace)
            || !valid_id(start)
            || !valid_id(end)
            || after.is_some_and(|k| !valid_id(k) || k < start || k >= end)
            || !(1..=256).contains(&limit)
        {
            return Err(bad());
        }
        if start >= end {
            return Ok(vec![]);
        }
        let lower = match after {
            Some(k) => Excluded((namespace.to_owned(), k.to_owned())),
            None => Included((namespace.to_owned(), start.to_owned())),
        };
        Ok(self
            .state
            .entries
            .range((lower, Excluded((namespace.to_owned(), end.to_owned()))))
            .take(limit)
            .map(|(_, r)| r.clone())
            .collect())
    }
}
impl TransactionalStore for CowStore {
    fn read(&self, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
        self.healthy()?;
        if !valid_id(namespace) || !valid_id(key) {
            return Err(bad());
        }
        Ok(self
            .state
            .entries
            .get(&(namespace.to_owned(), key.to_owned()))
            .cloned())
    }
    fn commit_batch(&mut self, request_key: &str, ops: &[NativeMutation]) -> Result<NativeCommit> {
        self.healthy()?;
        validate_batch(request_key, ops)?;
        let fp = batch_fingerprint(ops)?;
        if let Some((stored, r)) = self.state.receipts.get(request_key) {
            if stored != &fp {
                return Err(Error::Conflict(
                    "cow store: request key payload mismatch".into(),
                ));
            }
            return Ok(NativeCommit {
                replayed: true,
                ..r.clone()
            });
        }
        let pending = prepare(&self.state, request_key, ops)?;
        let frame = Frame {
            seq: pending.result.sequence,
            request_key: request_key.into(),
            operations: ops.to_vec(),
        };
        let (parts, digest, size) = frame_parts(&frame, &self.state.hash)?;
        let projected = self.payload_bytes.checked_add(size).ok_or_else(cap)?;
        if projected > MAX_FRAME_BYTES
            || frame.seq > MAX_LOG_TRANSACTIONS
            || self.tree.report().rows + parts.len() as u64 > MAX_PHYSICAL_ROWS
        {
            return Err(cap());
        }
        // Commit uses the COW ownership check and publishes all fragments at
        // once. Any error is treated conservatively as requiring reopen.
        if let Err(e) = self.tree.commit_frame(&parts) {
            self.poisoned = true;
            return Err(e);
        }
        let result = pending.result.clone();
        apply(&mut self.state, pending, digest);
        self.payload_bytes = projected;
        self.secondary_cache.get_mut().retain(|(table, _), _| {
            let ns = format!("sedravasql_table.{table}");
            let catalog = format!("sedravasql_index_catalog.{table}");
            !result
                .entries
                .iter()
                .any(|e| e.namespace == ns || e.namespace == catalog)
        });
        Ok(result)
    }
}
