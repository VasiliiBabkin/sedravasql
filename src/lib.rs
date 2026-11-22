//! Experimental SedravaSQL embedded storage API.
//!
//! Not production-ready. Operations through this library write audit entries
//! transactionally. Direct SQLite file access bypasses those controls.
//! Audit history stores record keys and hashes, not previous values.

use anyhow::{bail, Context, Result};
use rusqlite::{
    params, params_from_iter, types::ValueRef, Connection, OpenFlags, OptionalExtension,
    TransactionBehavior,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// Hard-coded prototype policy, not a permission or access-control boundary.
pub const MAX_DELETE_ROWS: usize = 10;
const MAX_PREVIEW_ROWS: usize = 10_000;
const MAX_OUTPUT_ROWS: usize = 1_000;
const MAX_KEY_BYTES: usize = 256;
const MAX_VALUE_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub prefix: String,
    pub count: usize,
    pub revision: i64,
    pub token: String,
    pub max_delete_rows: usize,
}

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub seq: i64,
    pub action: String,
    pub key: String,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
    pub timestamp_utc: String,
}

#[derive(Debug, Serialize)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

struct Snapshot {
    records: Vec<Record>,
    revision: i64,
    token: String,
}

fn connect(path: &Path, flags: OpenFlags) -> Result<Connection> {
    if path.as_os_str().is_empty() {
        bail!("database path must not be empty");
    }
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(3))?;
    Ok(conn)
}

fn open_existing(path: &Path, writable: bool) -> Result<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn = connect(path, flags)?;
    ensure_initialized(&conn)?;
    Ok(conn)
}

fn ensure_initialized(conn: &Connection) -> Result<()> {
    let marker_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='sedravasql_meta')",
        [],
        |row| row.get(0),
    )?;
    if !marker_exists {
        bail!("not a SedravaSQL database; run sedravasql init --db PATH first");
    }
    let version: i64 = conn
        .query_row(
            "SELECT schema_version FROM sedravasql_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .context("missing or malformed SedravaSQL schema marker")?;
    if version != 1 {
        bail!("unsupported SedravaSQL schema version {version}; expected 1");
    }
    for table in ["sedravasql_kv", "sedravasql_audit"] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("database schema incomplete: missing {table}");
        }
    }
    Ok(())
}

/// Initialize a new SQLite file. Existing unrelated tables cause refusal.
/// Re-initializing a valid SedravaSQL database is a no-op.
pub fn initialize(path: &Path) -> Result<bool> {
    let mut conn = connect(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing_tables: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*'",
        [],
        |row| row.get(0),
    )?;
    if existing_tables > 0 {
        ensure_initialized(&tx)?;
        tx.commit()?;
        return Ok(false);
    }
    tx.execute_batch(
        "CREATE TABLE sedravasql_meta (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            schema_version INTEGER NOT NULL CHECK(schema_version = 1),
            revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0)
        );
        INSERT INTO sedravasql_meta(id, schema_version, revision) VALUES (1, 1, 0);
        CREATE TABLE sedravasql_kv (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        CREATE TABLE sedravasql_audit (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            action TEXT NOT NULL CHECK(action IN ('PUT', 'DELETE')),
            key TEXT NOT NULL,
            before_sha256 TEXT,
            after_sha256 TEXT,
            timestamp_utc TEXT NOT NULL DEFAULT
                (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );",
    )?;
    tx.commit()?;
    Ok(true)
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES || key.contains('\0') {
        bail!("key must contain 1..={MAX_KEY_BYTES} UTF-8 bytes and no NUL");
    }
    Ok(())
}

fn validate_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty() || prefix.len() > MAX_KEY_BYTES || prefix.contains('\0') {
        bail!("bulk-delete prefix must contain 1..={MAX_KEY_BYTES} UTF-8 bytes and no NUL");
    }
    Ok(())
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// Upsert one key. The corresponding audit entry is committed atomically.
pub fn put(path: &Path, key: &str, value: &str) -> Result<()> {
    validate_key(key)?;
    if value.len() > MAX_VALUE_BYTES {
        bail!("value exceeds {MAX_VALUE_BYTES} UTF-8 bytes");
    }
    let mut conn = open_existing(path, true)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before: Option<String> = tx
        .query_row(
            "SELECT value FROM sedravasql_kv WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?;
    tx.execute(
        "INSERT INTO sedravasql_kv(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    tx.execute(
        "INSERT INTO sedravasql_audit(action, key, before_sha256, after_sha256)
         VALUES ('PUT', ?1, ?2, ?3)",
        params![key, before.as_deref().map(digest), digest(value)],
    )?;
    tx.execute(
        "UPDATE sedravasql_meta SET revision = revision + 1 WHERE id = 1",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn get(path: &Path, key: &str) -> Result<Option<String>> {
    validate_key(key)?;
    let conn = open_existing(path, false)?;
    conn.query_row(
        "SELECT value FROM sedravasql_kv WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn list(path: &Path, prefix: &str) -> Result<Vec<Record>> {
    if prefix.len() > MAX_KEY_BYTES || prefix.contains('\0') {
        bail!("prefix must contain at most {MAX_KEY_BYTES} UTF-8 bytes and no NUL");
    }
    let conn = open_existing(path, false)?;
    read_records(&conn, prefix, MAX_OUTPUT_ROWS)
}

fn read_records(conn: &Connection, prefix: &str, limit: usize) -> Result<Vec<Record>> {
    let mut stmt = conn.prepare(
        "SELECT key, value FROM sedravasql_kv
         WHERE substr(key, 1, length(?1)) = ?1
         ORDER BY key COLLATE BINARY",
    )?;
    let mut rows = stmt.query([prefix])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        if records.len() >= limit {
            bail!("result exceeds {limit} rows; narrow the prefix");
        }
        records.push(Record {
            key: row.get(0)?,
            value: row.get(1)?,
        });
    }
    Ok(records)
}

fn frame_hash(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn snapshot(conn: &Connection, prefix: &str) -> Result<Snapshot> {
    validate_prefix(prefix)?;
    let revision: i64 = conn.query_row(
        "SELECT revision FROM sedravasql_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    let records = read_records(conn, prefix, MAX_PREVIEW_ROWS)?;
    let mut hasher = Sha256::new();
    hasher.update(b"SedravaSQL/bulk-delete-preview/v1");
    frame_hash(&mut hasher, prefix.as_bytes());
    hasher.update(revision.to_le_bytes());
    hasher.update((records.len() as u64).to_le_bytes());
    for record in &records {
        frame_hash(&mut hasher, record.key.as_bytes());
        frame_hash(&mut hasher, record.value.as_bytes());
    }
    Ok(Snapshot {
        records,
        revision,
        token: hex::encode(hasher.finalize()),
    })
}

/// Preview a prefix deletion, without reserving or locking matching rows.
pub fn preview_delete(path: &Path, prefix: &str) -> Result<Preview> {
    let mut conn = open_existing(path, false)?;
    // A read transaction keeps revision and matching rows on one snapshot.
    // It does not reserve those rows for the later apply operation.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let snap = snapshot(&tx, prefix)?;
    tx.commit()?;
    Ok(Preview {
        prefix: prefix.to_owned(),
        count: snap.records.len(),
        revision: snap.revision,
        token: snap.token,
        max_delete_rows: MAX_DELETE_ROWS,
    })
}

/// Recheck the preview inside a BEGIN IMMEDIATE transaction, then delete.
pub fn apply_delete(path: &Path, prefix: &str, expected_token: &str) -> Result<usize> {
    validate_prefix(prefix)?;
    if expected_token.len() != 64
        || !expected_token.bytes().all(|c| c.is_ascii_hexdigit())
    {
        bail!("expected a 64-character hexadecimal preview token");
    }
    let mut conn = open_existing(path, true)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let snap = snapshot(&tx, prefix)?;
    if snap.token != expected_token.to_ascii_lowercase() {
        bail!("stale preview: records or revision changed; preview again");
    }
    if snap.records.is_empty() {
        bail!("refusing an empty bulk deletion");
    }
    if snap.records.len() > MAX_DELETE_ROWS {
        bail!(
            "policy rejected deletion: {} rows exceed maximum {}",
            snap.records.len(),
            MAX_DELETE_ROWS
        );
    }
    for record in &snap.records {
        let deleted = tx.execute(
            "DELETE FROM sedravasql_kv WHERE key = ?1",
            [record.key.as_str()],
        )?;
        if deleted != 1 {
            bail!("concurrent or unexpected row count while deleting");
        }
        tx.execute(
            "INSERT INTO sedravasql_audit(action, key, before_sha256, after_sha256)
             VALUES ('DELETE', ?1, ?2, NULL)",
            params![record.key.as_str(), digest(&record.value)],
        )?;
    }
    tx.execute(
        "UPDATE sedravasql_meta SET revision = revision + 1 WHERE id = 1",
        [],
    )?;
    tx.commit()?;
    Ok(snap.records.len())
}

pub fn audit(path: &Path) -> Result<Vec<AuditEvent>> {
    let conn = open_existing(path, false)?;
    let mut stmt = conn.prepare(
        "SELECT seq, action, key, before_sha256, after_sha256, timestamp_utc
         FROM sedravasql_audit ORDER BY seq DESC LIMIT 100",
    )?;
    let events = stmt
        .query_map([], |r| {
            Ok(AuditEvent {
                seq: r.get(0)?,
                action: r.get(1)?,
                key: r.get(2)?,
                before_sha256: r.get(3)?,
                after_sha256: r.get(4)?,
                timestamp_utc: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(events)
}

/// Single read-only SELECT. Bound parameters are TEXT. No semicolons permitted.
pub fn query(path: &Path, sql: &str, args: &[String]) -> Result<QueryOutput> {
    if sql.contains(';')
        || !sql
            .split_whitespace()
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("SELECT"))
    {
        bail!("only one read-only SELECT without a semicolon is supported");
    }
    let conn = open_existing(path, false)?;
    let mut stmt = conn.prepare(sql)?;
    if !stmt.readonly() {
        bail!("SQLite classified the statement as writable; rejected");
    }
    if stmt.parameter_count() != args.len() {
        bail!(
            "query expects {} TEXT parameters, received {}",
            stmt.parameter_count(),
            args.len()
        );
    }
    let columns = stmt.column_names().map(|c| c.to_owned()).collect();
    let count = stmt.column_count();
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        if output.len() >= MAX_OUTPUT_ROWS {
            bail!("query result exceeds {MAX_OUTPUT_ROWS} rows; add a LIMIT");
        }
        let mut cells = Vec::with_capacity(count);
        for col in 0..count {
            let value = match row.get_ref(col)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(v) => Value::from(v),
                ValueRef::Real(v) => serde_json::to_value(v)?,
                ValueRef::Text(v) => Value::from(
                    std::str::from_utf8(v).context("invalid UTF-8 in query output")?,
                ),
                ValueRef::Blob(v) => Value::from(format!("hex:{}", hex::encode(v))),
            };
            cells.push(value);
        }
        output.push(cells);
    }
    Ok(QueryOutput { columns, rows: output })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn db() -> (TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.db");
        initialize(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn initialize_and_reopen() {
        let (_dir, path) = db();
        assert!(!initialize(&path).unwrap());
    }

    #[test]
    fn upsert_and_audit_hashes() {
        let (_dir, path) = db();
        put(&path, "sample", "one").unwrap();
        put(&path, "sample", "two").unwrap();
        assert_eq!(get(&path, "sample").unwrap().as_deref(), Some("two"));
        let events = audit(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].before_sha256, Some(digest("one")));
        assert_eq!(events[0].after_sha256, Some(digest("two")));
    }

    #[test]
    fn preview_and_delete_exact() {
        let (_dir, path) = db();
        put(&path, "test:a", "one").unwrap();
        put(&path, "test:b", "two").unwrap();
        put(&path, "other", "kept").unwrap();
        let p = preview_delete(&path, "test:").unwrap();
        assert_eq!(p.count, 2);
        assert_eq!(apply_delete(&path, "test:", &p.token).unwrap(), 2);
        assert_eq!(get(&path, "test:a").unwrap(), None);
        assert_eq!(get(&path, "other").unwrap().as_deref(), Some("kept"));
        assert_eq!(audit(&path).unwrap().iter().filter(|a| a.action == "DELETE").count(), 2);
    }

    #[test]
    fn stale_preview_is_rejected() {
        let (_dir, path) = db();
        put(&path, "test:a", "v1").unwrap();
        let p = preview_delete(&path, "test:").unwrap();
        put(&path, "test:a", "v2").unwrap();
        assert!(apply_delete(&path, "test:", &p.token).unwrap_err().to_string().contains("stale"));
        assert_eq!(get(&path, "test:a").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn unrelated_managed_write_invalidates_preview() {
        let (_dir, path) = db();
        put(&path, "test:a", "v").unwrap();
        let p = preview_delete(&path, "test:").unwrap();
        put(&path, "other", "v").unwrap();
        assert!(apply_delete(&path, "test:", &p.token).is_err());
    }

    #[test]
    fn bulk_delete_policy_blocks_more_than_ten_rows() {
        let (_dir, path) = db();
        for index in 0..11 {
            put(&path, &format!("test:{index}"), "v").unwrap();
        }
        let p = preview_delete(&path, "test:").unwrap();
        assert_eq!(p.count, 11);
        assert!(apply_delete(&path, "test:", &p.token).unwrap_err().to_string().contains("policy"));
        assert_eq!(list(&path, "test:").unwrap().len(), 11);
    }

    #[test]
    fn empty_prefix_and_empty_deletion_rejected() {
        let (_dir, path) = db();
        assert!(preview_delete(&path, "").is_err());
        let p = preview_delete(&path, "absent").unwrap();
        assert_eq!(p.count, 0);
        assert!(apply_delete(&path, "absent", &p.token).is_err());
    }

    #[test]
    fn audit_failure_rolls_back_data_change() {
        let (_dir, path) = db();
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TRIGGER block_audit BEFORE INSERT ON sedravasql_audit
             BEGIN SELECT RAISE(ABORT, 'audit rejected'); END;",
        ).unwrap();
        assert!(put(&path, "blocked", "value").is_err());
        assert_eq!(get(&path, "blocked").unwrap(), None);
    }

    #[test]
    fn bulk_delete_audit_failure_rolls_back_all_rows() {
        let (_dir, path) = db();
        put(&path, "batch:a", "one").unwrap();
        put(&path, "batch:b", "two").unwrap();
        let p = preview_delete(&path, "batch:").unwrap();
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TRIGGER block_delete_audit BEFORE INSERT ON sedravasql_audit
             WHEN NEW.action = 'DELETE'
             BEGIN SELECT RAISE(ABORT, 'audit rejected'); END;",
        ).unwrap();
        assert!(apply_delete(&path, "batch:", &p.token).is_err());
        assert_eq!(list(&path, "batch:").unwrap().len(), 2);
        assert_eq!(audit(&path).unwrap().iter().filter(|a| a.action == "DELETE").count(), 0);
    }

    #[test]
    fn direct_sqlite_write_changes_preview_fingerprint() {
        let (_dir, path) = db();
        put(&path, "key:one", "old").unwrap();
        let p = preview_delete(&path, "key:").unwrap();
        let c = Connection::open(&path).unwrap();
        c.execute("UPDATE sedravasql_kv SET value = 'external' WHERE key = 'key:one'", []).unwrap();
        assert!(apply_delete(&path, "key:", &p.token).is_err());
        assert_eq!(get(&path, "key:one").unwrap().as_deref(), Some("external"));
    }

    #[test]
    fn sql_readonly_and_bound_params() {
        let (_dir, path) = db();
        put(&path, "a", "quoted'value").unwrap();
        let q = query(
            &path,
            "SELECT value FROM sedravasql_kv WHERE key = ?1",
            &["a".to_owned()],
        ).unwrap();
        assert_eq!(q.rows, vec![vec![Value::from("quoted'value")]]);
        assert!(query(&path, "DELETE FROM sedravasql_kv", &[]).is_err());
        assert!(query(&path, "SELECT 1; DELETE FROM sedravasql_kv", &[]).is_err());
    }

    #[test]
    fn init_refuses_unrelated_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE important(value TEXT);").unwrap();
        assert!(initialize(&path).is_err());
        assert_eq!(c.query_row("SELECT COUNT(*) FROM important", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn prefixes_are_literal_not_like_wildcards() {
        let (_dir, path) = db();
        put(&path, "a%foo", "matches").unwrap();
        put(&path, "abc", "not a match").unwrap();
        assert_eq!(list(&path, "a%").unwrap().len(), 1);
    }
}
