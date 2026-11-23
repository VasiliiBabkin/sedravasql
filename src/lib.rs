//! SedravaSQL 0.1 alpha: a deliberately small, typed, embedded transaction API.
//! SQLite provides the WAL, isolation, transaction atomicity and online backup.
//! No claim of physical power-loss testing, encryption, distributed SQL or
//! protection against a malicious process able to rewrite the same DB files.
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error as StdError;
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub const SCHEMA_VERSION: i64 = 1;
pub const MAX_VALUE_BYTES: usize = 65_536;
const MAX_ID_BYTES: usize = 128;

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Conflict(String),
    Unsupported(String),
    Sql(rusqlite::Error),
    Io(std::io::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "INVALID_ARGUMENT: {s}"),
            Self::Conflict(s) => write!(f, "CONFLICT: {s}"),
            Self::Unsupported(s) => write!(f, "UNSUPPORTED: {s}"),
            Self::Sql(s) => write!(f, "SQLITE_ERROR: {s}"),
            Self::Io(s) => write!(f, "IO_ERROR: {s}"),
        }
    }
}
impl StdError for Error {}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
fn valid_request(s: &str) -> bool {
    s.len() >= 12 && valid_id(s)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn row_digest(value: Option<&str>) -> String {
    value.map_or_else(
        || digest(b"sedravasql.deleted.v1"),
        |v| digest(v.as_bytes()),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub namespace: String,
    pub key: String,
    pub value: Option<String>,
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub operation: String,
    pub namespace: String,
    pub key: String,
    pub existing: bool,
    pub expected_revision: u64,
    pub proposed_sha256: String,
    pub needs_confirmation: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub audit_id: i64,
    pub namespace: String,
    pub key: String,
    pub revision: u64,
    pub operation: String,
    pub value_sha256: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    pub id: i64,
    pub namespace: String,
    pub key: String,
    pub action: String,
    pub before_revision: u64,
    pub after_revision: u64,
    pub before_sha256: String,
    pub after_sha256: String,
    pub request_key: String,
}

pub enum Change<'a> {
    Put(&'a str),
    Delete { confirm: bool },
}
impl Change<'_> {
    fn operation(&self) -> &'static str {
        match self {
            Self::Put(_) => "put",
            Self::Delete { .. } => "delete",
        }
    }
    fn value(&self) -> Option<&str> {
        match self {
            Self::Put(v) => Some(v),
            Self::Delete { .. } => None,
        }
    }
}

// On Unix the managed database is a private, single-link regular file.
// SQLite's WAL/SHM siblings inherit database access restrictions. Files
// with public bits or multiple hardlinks are rejected rather than chmodded.
fn inspect_database_path(path: &Path, create: bool) -> Result<()> {
    use std::fs::{self, OpenOptions};
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(path)?;
            file.metadata()?
        }
        Err(e) => return Err(Error::Io(e)),
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(Error::Invalid(
            "database path must be a regular non-symlink file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid(
                "database file must be private mode 0600, single hardlink",
            ));
        }
    }
    Ok(())
}

pub struct Database {
    conn: Connection,
}
impl Database {
    fn connect_config(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;",
        )?;
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if mode.to_ascii_lowercase() != "wal" {
            return Err(Error::Unsupported("WAL mode unavailable".into()));
        }
        let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        if sync != 2 {
            return Err(Error::Unsupported(format!(
                "synchronous FULL required, got {sync}"
            )));
        }
        Ok(())
    }
    pub fn init(path: &Path) -> Result<Self> {
        inspect_database_path(path, true)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::connect_config(&conn)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            let count:i64=conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type IN ('table','view','trigger') AND name NOT LIKE 'sqlite_%'",
                [],|r|r.get(0))?;
            if count != 0 {
                return Err(Error::Unsupported(
                    "nonempty unversioned database; no implicit migration".into(),
                ));
            }
            conn.execute_batch(
                r#"
              BEGIN IMMEDIATE;
              CREATE TABLE records (
                namespace TEXT NOT NULL, key TEXT NOT NULL,
                value TEXT, revision INTEGER NOT NULL CHECK(revision>=1),
                PRIMARY KEY(namespace,key)
              ) WITHOUT ROWID;
              CREATE TABLE audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                namespace TEXT NOT NULL, key TEXT NOT NULL,
                action TEXT NOT NULL CHECK(action IN ('put','delete')),
                before_revision INTEGER NOT NULL,
                after_revision INTEGER NOT NULL,
                before_sha256 TEXT NOT NULL, after_sha256 TEXT NOT NULL,
                request_key TEXT NOT NULL,
                UNIQUE(namespace,request_key)
              );
              CREATE TABLE receipts (
                namespace TEXT NOT NULL, request_key TEXT NOT NULL,
                request_sha256 TEXT NOT NULL,
                audit_id INTEGER NOT NULL REFERENCES audit(id),
                response_json TEXT NOT NULL,
                PRIMARY KEY(namespace,request_key)
              ) WITHOUT ROWID;
              PRAGMA user_version=1;
              COMMIT;
            "#,
            )?;
        } else if version != SCHEMA_VERSION {
            return Err(Error::Unsupported(format!(
                "unknown database user_version={version}"
            )));
        }
        Self::verify_schema(&conn)?;
        Ok(Self { conn })
    }
    pub fn open(path: &Path) -> Result<Self> {
        inspect_database_path(path, false)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::connect_config(&conn)?;
        Self::verify_schema(&conn)?;
        Ok(Self { conn })
    }
    fn verify_schema(conn: &Connection) -> Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            return Err(Error::Unsupported(format!(
                "schema version {version} not supported"
            )));
        }
        for table in ["records", "audit", "receipts"] {
            let found: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                params![table],
                |r| r.get(0),
            )?;
            if !found {
                return Err(Error::Unsupported(format!("missing managed table {table}")));
            }
        }
        Ok(())
    }
    pub fn sqlite_version(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))?)
    }
    pub fn read(&self, namespace: &str, key: &str) -> Result<Option<Entry>> {
        if !valid_id(namespace) || !valid_id(key) {
            return Err(Error::Invalid(
                "namespace/key must be 1..128 ASCII [A-Za-z0-9_.-]",
            ));
        }
        let row = self
            .conn
            .query_row(
                "SELECT value, revision FROM records WHERE namespace=?1 AND key=?2",
                params![namespace, key],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(row.map(|(value, revision)| Entry {
            namespace: namespace.into(),
            key: key.into(),
            value,
            revision: revision as u64,
        }))
    }
    pub fn list(&self, namespace: &str) -> Result<Vec<Entry>> {
        if !valid_id(namespace) {
            return Err(Error::Invalid("invalid namespace"));
        }
        let mut st=self.conn.prepare(
            "SELECT key, value, revision FROM records WHERE namespace=?1 AND value IS NOT NULL ORDER BY key LIMIT 1000")?;
        let rows = st.query_map(params![namespace], |r| {
            Ok(Entry {
                namespace: namespace.into(),
                key: r.get(0)?,
                value: r.get(1)?,
                revision: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn preview(&self, namespace: &str, key: &str, change: Change<'_>) -> Result<Plan> {
        let before = self.read(namespace, key)?;
        validate_change(&change)?;
        let expected_revision = before.as_ref().map_or(0, |e| e.revision);
        Ok(Plan {
            operation: change.operation().into(),
            namespace: namespace.into(),
            key: key.into(),
            existing: before.as_ref().is_some_and(|e| e.value.is_some()),
            expected_revision,
            proposed_sha256: row_digest(change.value()),
            needs_confirmation: matches!(change, Change::Delete { .. }),
        })
    }
    pub fn mutate(
        &mut self,
        namespace: &str,
        key: &str,
        change: Change<'_>,
        expected_revision: u64,
        request_key: &str,
    ) -> Result<Receipt> {
        if !valid_id(namespace) || !valid_id(key) {
            return Err(Error::Invalid("invalid namespace or key"));
        }
        if !valid_request(request_key) {
            return Err(Error::Invalid(
                "request-key must be 12..128 ASCII [A-Za-z0-9_.-]",
            ));
        }
        validate_change(&change)?;
        if matches!(change, Change::Delete { confirm: false }) {
            return Err(Error::Invalid("delete requires explicit confirmation"));
        }
        if expected_revision > i64::MAX as u64 {
            return Err(Error::Invalid("revision exceeds SQLite int64"));
        }
        let operation = change.operation();
        let new_value = change.value();
        let fingerprint = digest(
            &serde_json::to_vec(&(
                "sedravasql.mutation.v1",
                namespace,
                key,
                operation,
                new_value,
                expected_revision,
            ))
            .expect("static serde tuple"),
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous:Option<(String,String)>=tx.query_row(
            "SELECT request_sha256,response_json FROM receipts WHERE namespace=?1 AND request_key=?2",
            params![namespace,request_key],|r|Ok((r.get(0)?,r.get(1)?))
        ).optional()?;
        if let Some((stored, body)) = previous {
            if stored != fingerprint {
                return Err(Error::Conflict(
                    "idempotency key already used for different mutation".into(),
                ));
            }
            let mut receipt: Receipt = serde_json::from_str(&body)
                .map_err(|_| Error::Unsupported("corrupted stored idempotency receipt".into()))?;
            receipt.replayed = true;
            tx.commit()?;
            return Ok(receipt);
        }
        let before: Option<(Option<String>, i64)> = tx
            .query_row(
                "SELECT value,revision FROM records WHERE namespace=?1 AND key=?2",
                params![namespace, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let actual = before.as_ref().map_or(0, |v| v.1 as u64);
        if actual != expected_revision {
            return Err(Error::Conflict(format!(
                "stale mutation preview: expected revision={expected_revision} actual={actual}"
            )));
        }
        if matches!(change, Change::Delete { .. }) && before.as_ref().is_none_or(|b| b.0.is_none())
        {
            return Err(Error::Conflict("cannot delete missing key".into()));
        }
        let next = actual
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or(Error::Invalid("revision overflow"))?;
        let oldhash = row_digest(before.as_ref().and_then(|v| v.0.as_deref()));
        let newhash = row_digest(new_value);
        tx.execute(
            "INSERT INTO records(namespace,key,value,revision) VALUES(?1,?2,?3,?4)
             ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value, revision=excluded.revision",
            params![namespace,key,new_value,next as i64])?;
        debug_fault("after_value_write");
        tx.execute(
            "INSERT INTO audit(namespace,key,action,before_revision,after_revision,before_sha256,after_sha256,request_key)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![namespace,key,operation,actual as i64,next as i64,oldhash,newhash,request_key])?;
        debug_fault("after_audit_write");
        let audit_id = tx.last_insert_rowid();
        let receipt = Receipt {
            audit_id,
            namespace: namespace.into(),
            key: key.into(),
            revision: next,
            operation: operation.into(),
            value_sha256: newhash,
            replayed: false,
        };
        let response_json =
            serde_json::to_string(&receipt).map_err(|_| Error::Invalid("cannot encode receipt"))?;
        tx.execute(
            "INSERT INTO receipts(namespace,request_key,request_sha256,audit_id,response_json)
             VALUES(?1,?2,?3,?4,?5)",
            params![namespace, request_key, fingerprint, audit_id, response_json],
        )?;
        debug_fault("before_commit");
        tx.commit()?;
        debug_fault("after_commit");
        Ok(receipt)
    }
    pub fn audit(&self, limit: u32) -> Result<Vec<Audit>> {
        if limit == 0 || limit > 1000 {
            return Err(Error::Invalid("audit limit must be 1..1000"));
        }
        let mut st=self.conn.prepare(
            "SELECT id,namespace,key,action,before_revision,after_revision,before_sha256,after_sha256,request_key
             FROM audit ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map(params![limit], |r| {
            Ok(Audit {
                id: r.get(0)?,
                namespace: r.get(1)?,
                key: r.get(2)?,
                action: r.get(3)?,
                before_revision: r.get::<_, i64>(4)? as u64,
                after_revision: r.get::<_, i64>(5)? as u64,
                before_sha256: r.get(6)?,
                after_sha256: r.get(7)?,
                request_key: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn verify(&self) -> Result<()> {
        let check: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(Error::Unsupported(format!(
                "SQLite integrity_check={check}"
            )));
        }
        Self::verify_schema(&self.conn)?;
        let count:i64=self.conn.query_row(
            "SELECT count(*) FROM audit a LEFT JOIN receipts r ON a.id=r.audit_id WHERE r.audit_id IS NULL",[],|r|r.get(0))?;
        if count != 0 {
            return Err(Error::Unsupported("audit/receipt mismatch".into()));
        }
        let count:i64=self.conn.query_row(
            "SELECT count(*) FROM receipts r LEFT JOIN audit a ON r.audit_id=a.id WHERE a.id IS NULL",[],|r|r.get(0))?;
        if count != 0 {
            return Err(Error::Unsupported("orphan receipt".into()));
        }
        Ok(())
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        // SQLite backup API captures one consistent read transaction even in WAL mode.
        // Never copy only the live .db file. Never overwrite a backup.
        use rusqlite::backup::Backup;
        use std::fs::OpenOptions;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let _guard = opts.open(destination)?;
        let result = (|| -> Result<()> {
            let mut dest = Connection::open_with_flags(
                destination,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            {
                let backup = Backup::new(&self.conn, &mut dest)?;
                backup.run_to_completion(64, Duration::from_millis(5), None)?;
            }
            let check: String = dest.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if check != "ok" {
                return Err(Error::Unsupported(format!(
                    "backup integrity_check={check}"
                )));
            }
            drop(dest);
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(destination);
        }
        result
    }
}
fn validate_change(change: &Change<'_>) -> Result<()> {
    if let Change::Put(value) = change {
        if value.len() > MAX_VALUE_BYTES {
            return Err(Error::Invalid("value exceeds 65536 UTF-8 bytes"));
        }
        if value.chars().any(|c| c == '\0') {
            return Err(Error::Invalid("NUL in value is forbidden"));
        }
    }
    Ok(())
}

// Debug-child fault injection is omitted from optimized/release builds.
#[cfg(debug_assertions)]
fn debug_fault(point: &str) {
    if std::env::var("SEDRAVASQL_TEST_FAULT").ok().as_deref() == Some(point) {
        eprintln!("SEDRAVASQL_TEST_FAULT:{point}");
        std::process::exit(86);
    }
}
#[cfg(not(debug_assertions))]
fn debug_fault(_point: &str) {}
