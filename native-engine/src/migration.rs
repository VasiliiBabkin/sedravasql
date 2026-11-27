//! A24: ordered additive schema migrations for an exclusive NativeLog.
//! One DDL + one immutable history row in the same durable frame.
use crate::{sql, Error, NativeChange, NativeLog, NativeMutation, Result, TransactionalStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HISTORY: &str = "sedravasql_migrations_v1";
const MAX_STEPS: usize = 128;
const MAX_PLAN: usize = 65536;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub steps: Vec<Step>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub version: u32,
    pub name: String,
    pub sql: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub version: u32,
    pub name: String,
    pub sha256: String,
    pub sequence: u64,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub current_version: u32,
    pub target_version: u32,
    pub applied: Vec<Applied>,
    pub pending: Vec<u32>,
}
fn key(version: u32) -> String {
    format!("{version:08}")
}
fn request(version: u32) -> String {
    format!("migration-v1-{version:08}")
}
fn checksum(step: &Step) -> Result<String> {
    let data = serde_json::to_vec(&("sedravasql.migration.v1", step))
        .map_err(|_| Error::Invalid("migration: checksum encoding"))?;
    Ok(format!("{:x}", Sha256::digest(data)))
}
impl Plan {
    /// Read a bounded manifest from an operator-controlled path.
    /// The caller must keep the file and its parent directories trusted.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        use std::io::Read;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.len() > MAX_PLAN as u64 {
            return Err(Error::Invalid(
                "migration: regular bounded manifest required",
            ));
        }
        let file = std::fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::Invalid("migration: regular manifest required"));
        }
        let mut raw = Vec::new();
        file.take(MAX_PLAN as u64 + 1).read_to_end(&mut raw)?;
        Self::parse(&raw)
    }
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if raw.len() > MAX_PLAN {
            return Err(Error::Capacity("migration: manifest exceeds 64 KiB"));
        }
        let plan: Self = serde_json::from_slice(raw)
            .map_err(|_| Error::Invalid("migration: invalid manifest JSON"))?;
        plan.validate()?;
        Ok(plan)
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1 || self.steps.is_empty() || self.steps.len() > MAX_STEPS {
            return Err(Error::Invalid("migration: manifest format or step count"));
        }
        let mut names = std::collections::BTreeSet::new();
        let mut bytes = 0usize;
        for (i, step) in self.steps.iter().enumerate() {
            bytes = bytes
                .checked_add(step.sql.len() + step.name.len())
                .ok_or(Error::Capacity("migration: plan size"))?;
            if step.version != i as u32 + 1
                || step.name.is_empty()
                || step.name.len() > 64
                || !step
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !names.insert(&step.name)
            {
                return Err(Error::Invalid(
                    "migration: noncontiguous versions or invalid/duplicate name",
                ));
            }
            sql::migration_check(&step.sql)?;
        }
        if bytes > MAX_PLAN {
            return Err(Error::Capacity("migration: plan size"));
        }
        Ok(())
    }
}
fn history(db: &NativeLog) -> Result<Vec<Applied>> {
    let rows = db.scan_namespace(HISTORY, None, MAX_STEPS + 1)?;
    if rows.len() > MAX_STEPS {
        return Err(Error::Capacity("migration: history size"));
    }
    let mut out: Vec<Applied> = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let version = i as u32 + 1;
        let value = row
            .value
            .as_deref()
            .ok_or(Error::Invalid("migration: tombstone history"))?;
        let item: Applied = serde_json::from_str(value)
            .map_err(|_| Error::Invalid("migration: invalid history record"))?;
        if row.revision != 1
            || row.key != key(version)
            || item.version != version
            || item.sequence == 0
            || item.sequence > db.sequence()
            || out
                .last()
                .is_some_and(|prev| prev.sequence >= item.sequence)
            || item.sha256.len() != 64
            || !item
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("migration: history invariant"));
        }
        let receipt = db
            .lookup_receipt(&request(version))?
            .ok_or(Error::Invalid("migration: missing atomic receipt"))?;
        if receipt.sequence != item.sequence
            || receipt.entries.len() != 2
            || receipt.entries.last() != Some(&row)
            || receipt.entries[0].revision != 1
        {
            return Err(Error::Invalid("migration: history/receipt mismatch"));
        }
        let catalog = &receipt.entries[0];
        if catalog.value.is_none()
            || !(catalog.namespace == "sedravasql_table_catalog"
                || catalog.namespace == "sedravasql_fk_catalog"
                || catalog.namespace.starts_with("sedravasql_index_catalog."))
        {
            return Err(Error::Invalid("migration: invalid catalog receipt"));
        }
        if db.read(&catalog.namespace, &catalog.key)?.as_ref() != Some(catalog) {
            return Err(Error::Conflict("migration: applied catalog drift".into()));
        }
        out.push(item);
    }
    Ok(out)
}
pub fn status(db: &NativeLog, plan: &Plan) -> Result<Report> {
    plan.validate()?;
    let applied = history(db)?;
    if applied.len() > plan.steps.len() {
        return Err(Error::Conflict("migration: downgrade refused".into()));
    }
    for (record, step) in applied.iter().zip(&plan.steps) {
        if record.name != step.name || record.sha256 != checksum(step)? {
            return Err(Error::Conflict(format!(
                "migration: history differs at version {}",
                record.version
            )));
        }
    }
    Ok(Report {
        current_version: applied.len() as u32,
        target_version: plan.steps.len() as u32,
        pending: plan.steps[applied.len()..]
            .iter()
            .map(|step| step.version)
            .collect(),
        applied,
    })
}
/// Earlier completed versions survive a later refusal. Each version is atomic.
pub fn apply(db: &mut NativeLog, plan: &Plan) -> Result<Report> {
    let report = status(db, plan)?;
    for step in &plan.steps[report.current_version as usize..] {
        let op = sql::migration_prepare(db, &step.sql)?;
        let record = Applied {
            version: step.version,
            name: step.name.clone(),
            sha256: checksum(step)?,
            sequence: db
                .sequence()
                .checked_add(1)
                .ok_or(Error::Capacity("migration: sequence"))?,
        };
        let marker = NativeMutation {
            namespace: HISTORY.into(),
            key: key(step.version),
            expected_revision: 0,
            change: NativeChange::Put(
                serde_json::to_string(&record)
                    .map_err(|_| Error::Invalid("migration: history encoding"))?,
            ),
        };
        let commit = db.commit_batch(&request(step.version), &[op, marker])?;
        if commit.replayed || commit.sequence != record.sequence {
            return Err(Error::Invalid("migration: unexpected receipt"));
        }
    }
    status(db, plan)
}
