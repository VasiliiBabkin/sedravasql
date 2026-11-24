//! B6 opt-in atomic managed multi-key PUT, with full stored response.
//! This operates on legacy SQLite records/audit/receipts, not engine_batch_*.
use crate::{
    digest, row_digest, valid_id, valid_request, validate_change, Change, Database, Error, Receipt,
    Result,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const BATCH_SCHEMA: &str = "sedravasql.managed-batch.v1";
const LIMIT: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPut {
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedBatchReceipt {
    pub schema: String,
    pub scope: String,
    pub request_key: String,
    pub fingerprint: String,
    pub revisions: Vec<u64>,
    pub application_response: String,
    pub replayed: bool,
}

fn validate(scope: &str, request: &str, ops: &[ManagedPut], response: &str) -> Result<()> {
    if !valid_id(scope) || !valid_request(request) {
        return Err(Error::Invalid("invalid managed batch scope or request"));
    }
    if !(2..=LIMIT).contains(&ops.len()) {
        return Err(Error::Invalid("managed batch requires 2..4 unique keys"));
    }
    if response.is_empty()
        || response.len() > 16_384
        || serde_json::from_str::<serde_json::Value>(response).is_err()
    {
        return Err(Error::Invalid(
            "managed batch response must be bounded valid JSON",
        ));
    }
    let mut seen = BTreeSet::new();
    for op in ops {
        if !valid_id(&op.namespace)
            || !valid_id(&op.key)
            || op.expected_revision >= i64::MAX as u64
            || !seen.insert((&op.namespace, &op.key))
        {
            return Err(Error::Invalid(
                "invalid, duplicate or overflowing managed key",
            ));
        }
        validate_change(&Change::Put(&op.value))?;
    }
    Ok(())
}
fn fingerprint(scope: &str, ops: &[ManagedPut], response: &str) -> String {
    digest(&serde_json::to_vec(&(BATCH_SCHEMA, scope, ops, response)).expect("static tuple"))
}
fn row_request(scope: &str, request: &str, index: usize) -> String {
    let bytes = serde_json::to_vec(&(BATCH_SCHEMA, scope, request, index)).expect("static tuple");
    format!("b9_{:x}", Sha256::digest(bytes))
}
fn parse_receipt(
    scope: &str,
    request: &str,
    stored: &str,
    body: &str,
) -> Result<ManagedBatchReceipt> {
    let receipt: ManagedBatchReceipt = serde_json::from_str(body)
        .map_err(|_| Error::Unsupported("corrupt managed batch receipt".into()))?;
    if receipt.schema != BATCH_SCHEMA
        || receipt.scope != scope
        || receipt.request_key != request
        || receipt.fingerprint != stored
        || receipt.replayed
        || !(2..=LIMIT).contains(&receipt.revisions.len())
        || receipt
            .revisions
            .iter()
            .any(|r| *r == 0 || *r > i64::MAX as u64)
        || receipt.application_response.is_empty()
        || receipt.application_response.len() > 16_384
        || serde_json::from_str::<serde_json::Value>(&receipt.application_response).is_err()
        || stored.len() != 64
        || !stored
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::Unsupported(
            "inconsistent managed batch receipt".into(),
        ));
    }
    Ok(receipt)
}

impl Database {
    /// Explicit, opt-in schema extension. Legacy tables and user_version stay
    /// unchanged, and all original read/mutate/verify semantics remain.
    pub fn enable_managed_batches(&mut self) -> Result<()> {
        Self::verify_schema(&self.conn)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS managed_batch_receipts (
                scope TEXT NOT NULL, request_key TEXT NOT NULL,
                fingerprint TEXT NOT NULL, result_json TEXT NOT NULL,
                PRIMARY KEY(scope,request_key)
             ) WITHOUT ROWID;",
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn lookup_managed_batch(
        &self,
        scope: &str,
        request: &str,
    ) -> Result<Option<ManagedBatchReceipt>> {
        if !valid_id(scope) || !valid_request(request) {
            return Err(Error::Invalid("invalid managed batch lookup"));
        }
        let found = self
            .conn
            .query_row(
                "SELECT fingerprint,result_json FROM managed_batch_receipts
             WHERE scope=?1 AND request_key=?2",
                params![scope, request],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        found
            .map(|(fp, json)| parse_receipt(scope, request, &fp, &json))
            .transpose()
    }

    /// One SQLite IMMEDIATE transaction changes ALL managed KV rows and their
    /// audit+legacy receipt pairs plus the immutable overarching receipt.
    /// A CAS rejection rolls the WHOLE transaction back, even for earlier rows.
    pub fn commit_managed_batch(
        &mut self,
        scope: &str,
        request: &str,
        ops: &[ManagedPut],
        response: &str,
    ) -> Result<ManagedBatchReceipt> {
        validate(scope, request, ops, response)?;
        let fp = fingerprint(scope, ops, response);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior = tx
            .query_row(
                "SELECT fingerprint,result_json FROM managed_batch_receipts
             WHERE scope=?1 AND request_key=?2",
                params![scope, request],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((stored, json)) = prior {
            if stored != fp {
                return Err(Error::Conflict("managed batch idempotency conflict".into()));
            }
            let mut result = parse_receipt(scope, request, &stored, &json)?;
            if result.revisions.len() != ops.len()
                || result.application_response != response
                || result
                    .revisions
                    .iter()
                    .zip(ops)
                    .any(|(r, op)| *r != op.expected_revision + 1)
            {
                return Err(Error::Unsupported(
                    "batch receipt disagrees with operations".into(),
                ));
            }
            result.replayed = true;
            return Ok(result);
        }
        let mut old_values = Vec::with_capacity(ops.len());
        for op in ops {
            let existing = tx
                .query_row(
                    "SELECT value,revision FROM records WHERE namespace=?1 AND key=?2",
                    params![&op.namespace, &op.key],
                    |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            let current = existing.as_ref().map_or(0, |(_, r)| *r);
            if current < 0 || current as u64 != op.expected_revision {
                return Err(Error::Conflict(format!(
                    "managed batch stale {}/{}",
                    op.namespace, op.key
                )));
            }
            old_values.push(existing.and_then(|(v, _)| v));
        }
        let mut revisions = Vec::with_capacity(ops.len());
        for (index, op) in ops.iter().enumerate() {
            let revision = op.expected_revision + 1;
            let request_key = row_request(scope, request, index);
            let oldhash = row_digest(old_values[index].as_deref());
            let newhash = row_digest(Some(&op.value));
            tx.execute(
                "INSERT INTO records(namespace,key,value,revision) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value,revision=excluded.revision",
                params![&op.namespace,&op.key,&op.value,revision as i64]
            )?;
            if index == 0 {
                fault("after_first_value");
            }
            tx.execute(
                "INSERT INTO audit(namespace,key,action,before_revision,after_revision,
                 before_sha256,after_sha256,request_key)
                 VALUES(?1,?2,'put',?3,?4,?5,?6,?7)",
                params![
                    &op.namespace,
                    &op.key,
                    op.expected_revision as i64,
                    revision as i64,
                    oldhash,
                    newhash,
                    &request_key
                ],
            )?;
            let audit_id = tx.last_insert_rowid();
            let receipt = Receipt {
                audit_id,
                namespace: op.namespace.clone(),
                key: op.key.clone(),
                revision,
                operation: "put".into(),
                value_sha256: newhash,
                replayed: false,
                application_response: Some(response.into()),
            };
            let json = serde_json::to_string(&receipt)
                .map_err(|_| Error::Invalid("cannot serialize managed batch receipt"))?;
            let individual_fp = digest(
                &serde_json::to_vec(&(
                    "sedravasql.mutation.with-response.v1",
                    &op.namespace,
                    &op.key,
                    "put",
                    Some(&op.value),
                    op.expected_revision,
                    response,
                ))
                .expect("static tuple"),
            );
            tx.execute(
                "INSERT INTO receipts(namespace,request_key,request_sha256,audit_id,response_json)
                 VALUES(?1,?2,?3,?4,?5)",
                params![&op.namespace, &request_key, individual_fp, audit_id, json],
            )?;
            revisions.push(revision);
        }
        fault("after_all_rows");
        let receipt = ManagedBatchReceipt {
            schema: BATCH_SCHEMA.into(),
            scope: scope.into(),
            request_key: request.into(),
            fingerprint: fp.clone(),
            revisions,
            application_response: response.into(),
            replayed: false,
        };
        let json = serde_json::to_string(&receipt)
            .map_err(|_| Error::Invalid("cannot serialize managed batch receipt"))?;
        tx.execute(
            "INSERT INTO managed_batch_receipts(scope,request_key,fingerprint,result_json)
             VALUES(?1,?2,?3,?4)",
            params![scope, request, fp, json],
        )?;
        fault("before_commit");
        tx.commit()?;
        fault("after_commit");
        Ok(receipt)
    }
}

#[cfg(debug_assertions)]
fn fault(point: &str) {
    if std::env::var("SEDRAVASQL_MANAGED_BATCH_TEST_FAULT")
        .ok()
        .as_deref()
        == Some(point)
    {
        eprintln!("SEDRAVASQL_MANAGED_BATCH_TEST_FAULT:{point}");
        std::process::exit(86);
    }
}
#[cfg(not(debug_assertions))]
fn fault(_: &str) {}
