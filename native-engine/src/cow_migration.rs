//! B34a offline NativeLog -> independent COW migration image.
//! A preservation format, not yet a SQL or TransactionalStore backend.
use super::{
    backup::same_file,
    cow_tree::{CowTree, Record},
    Error, NativeCommit, NativeEntry, NativeLog, Result,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

const CHUNK: usize = 48 * 1024;
const MAX_ROWS: usize = 32_768;
const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
fn bad() -> Error {
    Error::Invalid("cow migration: invalid image")
}
fn cap() -> Error {
    Error::Capacity("cow migration: image budget")
}
fn json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| bad())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn address(ns: &str, key: &str) -> Result<String> {
    if !super::valid_id(ns) || !super::valid_id(key) {
        return Err(bad());
    }
    Ok(format!("e-{}", digest(json(&(ns, key))?.as_bytes())))
}
fn receipt_address(key: &str) -> Result<String> {
    if key.len() < 12 || !super::valid_id(key) {
        return Err(bad());
    }
    Ok(format!("r-{}", digest(key.as_bytes())))
}
fn side(path: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = path.file_name().ok_or_else(bad)?.to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}
fn normalized(path: &Path) -> Result<PathBuf> {
    Ok(fs::canonicalize(path.parent().ok_or_else(bad)?)?.join(path.file_name().ok_or_else(bad)?))
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationManifest {
    pub format: String,
    pub source_sequence: u64,
    pub source_hash: String,
    pub entries: usize,
    pub receipts: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalReceipt {
    pub request_key: String,
    pub fingerprint: String,
    pub result: NativeCommit,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parts {
    count: usize,
    bytes: usize,
    sha256: String,
}
#[derive(Default)]
struct Builder {
    rows: Vec<Record>,
    bytes: usize,
}
impl Builder {
    fn push(&mut self, key: String, revision: u64, value: String) -> Result<()> {
        self.bytes = self.bytes.checked_add(value.len()).ok_or_else(cap)?;
        if self.bytes > MAX_PAYLOAD || self.rows.len() >= MAX_ROWS {
            return Err(cap());
        }
        self.rows.push(Record {
            key,
            revision,
            value: Some(value),
        });
        Ok(())
    }
    fn object<T: Serialize>(&mut self, key: String, revision: u64, obj: &T) -> Result<()> {
        let encoded = json(obj)?;
        let mut at = 0;
        let mut count = 0;
        while at < encoded.len() {
            let mut end = (at + CHUNK).min(encoded.len());
            while !encoded.is_char_boundary(end) {
                end -= 1;
            }
            self.push(
                format!("{key}-{count:04}"),
                revision,
                encoded[at..end].to_owned(),
            )?;
            count += 1;
            at = end;
        }
        self.push(
            key,
            revision,
            json(&Parts {
                count,
                bytes: encoded.len(),
                sha256: digest(encoded.as_bytes()),
            })?,
        )
    }
}

/// Keep the NativeLog exclusively locked and revalidate the journal from disk
/// before building a new image. The source is never rewritten or deleted.
/// No automatic replacement of any destination. Size budgets fail closed.
pub fn migrate_copy(source: &Path, destination: &Path) -> Result<MigrationManifest> {
    super::private_parent(source)?;
    super::private_parent(destination)?;
    let src = normalized(source)?;
    let dst = normalized(destination)?;
    let reserved = [src.clone(), side(&src, ".lock")?];
    for path in [
        dst.clone(),
        side(&dst, ".head")?,
        side(&dst, ".head.tmp")?,
        side(&dst, ".lock")?,
    ] {
        if reserved.contains(&path) {
            return Err(bad());
        }
    }
    let mut db = NativeLog::open(source)?;
    same_file(&db.file, source)?;
    db.verify()?;
    let (manifest, rows) = build_image(&db)?;
    // Source must still be the same verified file before publication.
    same_file(&db.file, source)?;
    db.verify()?;
    CowTree::create_snapshot(destination, &rows, manifest.source_sequence.max(1))?;
    Ok(manifest)
}

fn build_image(db: &NativeLog) -> Result<(MigrationManifest, Vec<Record>)> {
    let state = &db.state;
    let manifest = MigrationManifest {
        format: "sedravasql-native-log-cow-image-v1".into(),
        source_sequence: state.seq,
        source_hash: state.hash.iter().map(|b| format!("{b:02x}")).collect(),
        entries: state.entries.len(),
        receipts: state.receipts.len(),
    };
    let mut b = Builder::default();
    b.push("manifest".into(), state.seq.max(1), json(&manifest)?)?;
    for row in state.entries.values() {
        b.object(address(&row.namespace, &row.key)?, row.revision, row)?;
    }
    for (key, (fingerprint, result)) in &state.receipts {
        b.object(
            receipt_address(key)?,
            result.sequence,
            &HistoricalReceipt {
                request_key: key.clone(),
                fingerprint: fingerprint.clone(),
                result: result.clone(),
            },
        )?;
    }
    b.rows.sort_by(|a, b| a.key.cmp(&b.key));
    Ok((manifest, b.rows))
}

pub struct MigrationImage {
    tree: CowTree,
    manifest: MigrationManifest,
}
impl MigrationImage {
    pub fn open(path: &Path) -> Result<Self> {
        let mut tree = CowTree::open(path)?;
        let row = tree.get("manifest")?.ok_or_else(bad)?;
        let manifest: MigrationManifest =
            serde_json::from_str(row.value.as_deref().ok_or_else(bad)?).map_err(|_| bad())?;
        if manifest.format != "sedravasql-native-log-cow-image-v1"
            || manifest.source_sequence.max(1) != tree.report().generation
            || row.revision != tree.report().generation
            || manifest.source_hash.len() != 64
            || !manifest.source_hash.bytes().all(|b| b.is_ascii_hexdigit())
            || manifest.entries > MAX_ROWS
            || manifest.receipts > MAX_ROWS
        {
            return Err(bad());
        }
        Ok(Self { tree, manifest })
    }
    /// Verify every physical record and the complete manifest against a
    /// fresh source replay. A changed source returns false, never modifies it.
    pub fn matches_source(&mut self, path: &Path) -> Result<bool> {
        let mut db = NativeLog::open(path)?;
        same_file(&db.file, path)?;
        db.verify()?;
        let (manifest, rows) = build_image(&db)?;
        Ok(self.manifest == manifest && self.tree.snapshot_rows()? == rows)
    }

    pub fn manifest(&self) -> &MigrationManifest {
        &self.manifest
    }
    fn object<T: DeserializeOwned>(&mut self, key: &str) -> Result<Option<(u64, T)>> {
        let Some(row) = self.tree.get(key)? else {
            return Ok(None);
        };
        let parts: Parts =
            serde_json::from_str(row.value.as_deref().ok_or_else(bad)?).map_err(|_| bad())?;
        if parts.count == 0
            || parts.count > MAX_PAYLOAD.div_ceil(CHUNK) + 1
            || parts.bytes > MAX_PAYLOAD
        {
            return Err(bad());
        }
        let mut data = String::with_capacity(parts.bytes);
        for i in 0..parts.count {
            let part = self.tree.get(&format!("{key}-{i:04}"))?.ok_or_else(bad)?;
            let value = part.value.ok_or_else(bad)?;
            if part.revision != row.revision
                || value.is_empty()
                || value.len() > CHUNK
                || data.len().checked_add(value.len()).ok_or_else(cap)? > parts.bytes
            {
                return Err(bad());
            }
            data.push_str(&value);
        }
        if data.len() != parts.bytes || digest(data.as_bytes()) != parts.sha256 {
            return Err(bad());
        }
        Ok(Some((
            row.revision,
            serde_json::from_str(&data).map_err(|_| bad())?,
        )))
    }
    pub fn entry(&mut self, namespace: &str, key: &str) -> Result<Option<NativeEntry>> {
        let result = self.object::<NativeEntry>(&address(namespace, key)?)?;
        result
            .map(|(rev, row)| {
                if row.namespace != namespace || row.key != key || row.revision != rev {
                    return Err(bad());
                }
                Ok(row)
            })
            .transpose()
    }
    pub fn receipt(&mut self, key: &str) -> Result<Option<HistoricalReceipt>> {
        let result = self.object::<HistoricalReceipt>(&receipt_address(key)?)?;
        result
            .map(|(rev, row)| {
                if row.request_key != key || row.result.sequence != rev || row.result.replayed {
                    return Err(bad());
                }
                Ok(row)
            })
            .transpose()
    }
    /// Verify the original NativeLog fingerprint, then return exactly its
    /// historical commit result. Unknown keys return None; never execute.
    pub fn replay(
        &mut self,
        key: &str,
        ops: &[super::NativeMutation],
    ) -> Result<Option<NativeCommit>> {
        super::validate_batch(key, ops)?;
        let Some(receipt) = self.receipt(key)? else {
            return Ok(None);
        };
        if receipt.fingerprint != super::batch_fingerprint(ops)? {
            return Err(Error::Conflict(
                "cow migration: request payload mismatch".into(),
            ));
        }
        Ok(Some(NativeCommit {
            replayed: true,
            ..receipt.result
        }))
    }
}
