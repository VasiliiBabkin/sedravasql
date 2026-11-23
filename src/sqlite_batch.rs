//! Explicit, opt-in experimental batch adapter on existing SQLite v1 files.
//! Legacy Database methods and Server A6 continue to use records/audit/
//! receipts unchanged. Batch tables are created only by init().

use crate::native_log::{
    batch_fingerprint, validate_batch, Error as BatchError, NativeChange, NativeCommit,
    NativeEntry, NativeMutation, Result as BatchResult, TransactionalStore,
};
use crate::Database;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::Path;

fn sql<T>(value: rusqlite::Result<T>) -> BatchResult<T> {
    value.map_err(|err| BatchError::Unsupported(format!("sqlite batch adapter: {err}")))
}

fn row_hash(value: Option<&str>) -> String {
    format!(
        "{:x}",
        Sha256::digest(value.unwrap_or("sedravasql.batch.deleted.v1").as_bytes())
    )
}

/// Experimental SQLite batch adapter with the same contract as NativeLog.
/// Never changes legacy records/audit/receipts and requires explicit init.
pub struct SqliteBatchStore {
    db: Database,
    poisoned: bool,
}
impl SqliteBatchStore {
    pub fn init(path: &Path) -> BatchResult<Self> {
        let mut db = Database::init(path).map_err(|e| BatchError::Unsupported(e.to_string()))?;
        let tx = sql(db
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        sql(tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS engine_batch_meta (
               singleton INTEGER PRIMARY KEY CHECK(singleton=1),
               sequence INTEGER NOT NULL CHECK(sequence>=0)
             );
             CREATE TABLE IF NOT EXISTS engine_batch_entries (
               namespace TEXT NOT NULL, key TEXT NOT NULL,
               value TEXT, revision INTEGER NOT NULL CHECK(revision>=1),
               PRIMARY KEY(namespace,key)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS engine_batch_receipts (
               request_key TEXT PRIMARY KEY,
               fingerprint TEXT NOT NULL, sequence INTEGER NOT NULL UNIQUE,
               result_json TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS engine_batch_audit (
               sequence INTEGER NOT NULL, op_index INTEGER NOT NULL,
               namespace TEXT NOT NULL, key TEXT NOT NULL,
               before_revision INTEGER NOT NULL, after_revision INTEGER NOT NULL,
               before_hash TEXT NOT NULL, after_hash TEXT NOT NULL,
               request_key TEXT NOT NULL,
               PRIMARY KEY(sequence,op_index),
               FOREIGN KEY(sequence) REFERENCES engine_batch_receipts(sequence)
             ) WITHOUT ROWID;
             INSERT OR IGNORE INTO engine_batch_meta(singleton,sequence) VALUES(1,0);",
        ))?;
        sql(tx.commit())?;
        let result = Self {
            db,
            poisoned: false,
        };
        result.verify()?;
        Ok(result)
    }
    /// Opening does NOT initialize tables; untouched v1 databases are rejected.
    pub fn open(path: &Path) -> BatchResult<Self> {
        let db = Database::open(path).map_err(|e| BatchError::Unsupported(e.to_string()))?;
        let store = Self {
            db,
            poisoned: false,
        };
        store.verify()?;
        Ok(store)
    }
    pub fn sequence(&self) -> BatchResult<u64> {
        let seq: i64 = sql(self.db.conn.query_row(
            "SELECT sequence FROM engine_batch_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        if seq < 0 {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: invalid sequence".into(),
            ));
        }
        Ok(seq as u64)
    }
    pub fn verify(&self) -> BatchResult<()> {
        self.db
            .verify()
            .map_err(|e| BatchError::Unsupported(e.to_string()))?;
        let count: i64 =
            sql(self
                .db
                .conn
                .query_row("SELECT count(*) FROM engine_batch_receipts", [], |r| {
                    r.get(0)
                }))?;
        let seq = self.sequence()?;
        if count as u64 != seq {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: receipt/sequence mismatch".into(),
            ));
        }
        let orphans: i64 = sql(self.db.conn.query_row(
            "SELECT count(*) FROM engine_batch_audit a
             LEFT JOIN engine_batch_receipts r ON a.sequence=r.sequence
             WHERE r.sequence IS NULL",
            [],
            |r| r.get(0),
        ))?;
        if orphans != 0 {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: orphan audit".into(),
            ));
        }
        // A valid batch receipt must have 1..16 contiguous audit rows.
        // This catches a locally damaged or manually edited audit ledger
        // even when SQLite's own PRAGMA integrity_check reports "ok".
        let damaged: i64 = sql(self.db.conn.query_row(
            "SELECT count(*) FROM engine_batch_receipts r
             LEFT JOIN (
               SELECT sequence, count(*) AS operations,
                      min(op_index) AS first_i, max(op_index) AS last_i,
                      min(request_key) AS min_request, max(request_key) AS max_request
               FROM engine_batch_audit GROUP BY sequence
             ) a ON a.sequence=r.sequence
             WHERE a.operations IS NULL OR a.operations NOT BETWEEN 1 AND 16
               OR a.first_i != 0 OR a.last_i != a.operations-1
               OR a.min_request != r.request_key OR a.max_request != r.request_key",
            [],
            |r| r.get(0),
        ))?;
        if damaged != 0 {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: incomplete audit/receipt pair".into(),
            ));
        }
        // Cross-check every immutable receipt against the recorded per-op
        // audit. Counting alone is insufficient: a missing last operation
        // would otherwise make a two-key atomic commit look like one key.
        let mut st = sql(self.db.conn.prepare(
            "SELECT sequence,request_key,result_json FROM engine_batch_receipts ORDER BY sequence",
        ))?;
        let rows = sql(st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }))?;
        for row in rows {
            let (sequence, key, json) = sql(row)?;
            let commit: NativeCommit = serde_json::from_str(&json).map_err(|_| {
                BatchError::Unsupported("sqlite batch adapter: invalid persisted receipt".into())
            })?;
            if sequence < 1
                || commit.sequence != sequence as u64
                || commit.replayed
                || commit.entries.is_empty()
                || commit.entries.len() > 16
            {
                return Err(BatchError::Unsupported(
                    "sqlite batch adapter: inconsistent receipt".into(),
                ));
            }
            let mut audit = sql(self.db.conn.prepare(
                "SELECT op_index,namespace,key,after_revision,request_key
                 FROM engine_batch_audit WHERE sequence=?1 ORDER BY op_index",
            ))?;
            let audit_rows = sql(audit.query_map(params![sequence], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            }))?;
            let mut index = 0usize;
            for audit_row in audit_rows {
                let (idx, ns, name, rev, audit_key) = sql(audit_row)?;
                let Some(entry) = commit.entries.get(index) else {
                    return Err(BatchError::Unsupported(
                        "sqlite batch adapter: excess audit rows".into(),
                    ));
                };
                if idx != index as i64
                    || ns != entry.namespace
                    || name != entry.key
                    || rev < 1
                    || rev as u64 != entry.revision
                    || audit_key != key
                {
                    return Err(BatchError::Unsupported(
                        "sqlite batch adapter: audit disagrees with receipt".into(),
                    ));
                }
                index += 1;
            }
            if index != commit.entries.len() {
                return Err(BatchError::Unsupported(
                    "sqlite batch adapter: missing audit operation".into(),
                ));
            }
        }
        Ok(())
    }
}
impl TransactionalStore for SqliteBatchStore {
    fn read(&self, namespace: &str, key: &str) -> BatchResult<Option<NativeEntry>> {
        let valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        if !valid(namespace) || !valid(key) {
            return Err(BatchError::Invalid(
                "sqlite batch adapter: invalid namespace/key",
            ));
        }
        if self.poisoned {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: uncertain commit".into(),
            ));
        }
        sql(self
            .db
            .conn
            .query_row(
                "SELECT value,revision FROM engine_batch_entries WHERE namespace=?1 AND key=?2",
                params![namespace, key],
                |r| {
                    Ok(NativeEntry {
                        namespace: namespace.to_owned(),
                        key: key.to_owned(),
                        value: r.get(0)?,
                        revision: r.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .optional())
    }
    fn commit_batch(
        &mut self,
        request_key: &str,
        ops: &[NativeMutation],
    ) -> BatchResult<NativeCommit> {
        if self.poisoned {
            return Err(BatchError::Unsupported(
                "sqlite batch adapter: uncertain commit".into(),
            ));
        }
        validate_batch(request_key, ops)?;
        let fingerprint = batch_fingerprint(ops)?;
        let tx = sql(self
            .db
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        if let Some((stored, json)) = sql(tx
            .query_row(
                "SELECT fingerprint,result_json FROM engine_batch_receipts WHERE request_key=?1",
                params![request_key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional())?
        {
            if stored != fingerprint {
                return Err(BatchError::Conflict(
                    "sqlite batch adapter: reused request key".into(),
                ));
            }
            let mut old: NativeCommit = serde_json::from_str(&json).map_err(|_| {
                BatchError::Unsupported("sqlite batch adapter: corrupt receipt".into())
            })?;
            if old.replayed {
                return Err(BatchError::Unsupported(
                    "sqlite batch adapter: receipt marked replayed".into(),
                ));
            }
            old.replayed = true;
            return Ok(old);
        }
        let current: i64 = sql(tx.query_row(
            "SELECT sequence FROM engine_batch_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        if !(0..i64::MAX).contains(&current) {
            return Err(BatchError::Invalid(
                "sqlite batch adapter: sequence overflow",
            ));
        }
        let seq = current + 1;
        let mut entries = Vec::with_capacity(ops.len());
        let mut previous = Vec::with_capacity(ops.len());
        for op in ops {
            let row = sql(tx
                .query_row(
                    "SELECT value,revision FROM engine_batch_entries WHERE namespace=?1 AND key=?2",
                    params![&op.namespace, &op.key],
                    |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional())?;
            let actual = row.as_ref().map_or(0, |r| r.1);
            if actual < 0 || actual as u64 != op.expected_revision {
                return Err(BatchError::Conflict(format!(
                    "sqlite batch adapter: stale {}/{}",
                    op.namespace, op.key
                )));
            }
            if matches!(op.change, NativeChange::Delete)
                && row.as_ref().is_none_or(|r| r.0.is_none())
            {
                return Err(BatchError::Conflict(
                    "sqlite batch adapter: cannot delete absent row".into(),
                ));
            }
            let entry = NativeEntry {
                namespace: op.namespace.clone(),
                key: op.key.clone(),
                value: match &op.change {
                    NativeChange::Put(v) => Some(v.clone()),
                    NativeChange::Delete => None,
                },
                revision: op.expected_revision + 1,
            };
            previous.push(row.map(|r| r.0).unwrap_or(None));
            entries.push(entry);
        }
        let response = NativeCommit {
            sequence: seq as u64,
            entries,
            replayed: false,
        };
        let response_json = serde_json::to_string(&response)
            .map_err(|_| BatchError::Invalid("sqlite batch adapter: cannot serialize receipt"))?;
        sql(tx.execute(
            "INSERT INTO engine_batch_receipts(request_key,fingerprint,sequence,result_json)
             VALUES(?1,?2,?3,?4)",
            params![request_key, fingerprint, seq, response_json],
        ))?;
        for (idx, entry) in response.entries.iter().enumerate() {
            sql(tx.execute(
                "INSERT INTO engine_batch_entries(namespace,key,value,revision)
                 VALUES(?1,?2,?3,?4)
                 ON CONFLICT(namespace,key) DO UPDATE SET
                   value=excluded.value, revision=excluded.revision",
                params![
                    &entry.namespace,
                    &entry.key,
                    &entry.value,
                    entry.revision as i64
                ],
            ))?;
            sql(tx.execute(
                "INSERT INTO engine_batch_audit(sequence,op_index,namespace,key,
                 before_revision,after_revision,before_hash,after_hash,request_key)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    seq,
                    idx as i64,
                    &entry.namespace,
                    &entry.key,
                    (entry.revision - 1) as i64,
                    entry.revision as i64,
                    row_hash(previous[idx].as_deref()),
                    row_hash(entry.value.as_deref()),
                    request_key
                ],
            ))?;
        }
        sql(tx.execute(
            "UPDATE engine_batch_meta SET sequence=?1 WHERE singleton=1",
            params![seq],
        ))?;
        if let Err(err) = tx.commit() {
            self.poisoned = true;
            return Err(BatchError::Unsupported(format!(
                "sqlite batch adapter: commit uncertain: {err}"
            )));
        }
        Ok(response)
    }
}
