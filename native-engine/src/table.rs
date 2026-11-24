//! B16a: typed tables on a dedicated, trusted native journal.
//! Raw mutations to reserved namespaces, schema migrations and SQL are unsupported.
use super::{
    valid_id, Error, NativeChange, NativeCommit, NativeEntry, NativeLog, NativeMutation, Result,
    TransactionalStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
const CATALOG: &str = "sedravasql_table_catalog";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    Int,
    Bool,
    Text,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    pub kind: ColumnType,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TableSchema {
    pub name: String,
    pub primary_key: String,
    pub columns: Vec<Column>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum Cell {
    Int(i32),
    Bool(bool),
    Text(String),
}
pub type Row = BTreeMap<String, Cell>;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StoredRow {
    schema_sha256: String,
    values: Row,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TableRow {
    pub key: String,
    pub revision: u64,
    pub values: Option<Row>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
impl TableSchema {
    pub fn validate(&self) -> Result<()> {
        if !identifier(&self.name)
            || !identifier(&self.primary_key)
            || self.columns.is_empty()
            || self.columns.len() > 16
        {
            return Err(Error::Invalid("table: invalid schema"));
        }
        let mut seen = HashSet::new();
        for c in &self.columns {
            if !identifier(&c.name) || !seen.insert(&c.name) {
                return Err(Error::Invalid("table: invalid or duplicate column"));
            }
        }
        if !self
            .columns
            .iter()
            .any(|c| c.name == self.primary_key && c.kind == ColumnType::Text)
        {
            return Err(Error::Invalid("table: primary key must be a text column"));
        }
        Ok(())
    }
    fn encoded(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| Error::Invalid("table: schema encoding"))
    }
    fn hash(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(self.encoded()?.as_bytes())))
    }
    fn namespace(&self) -> String {
        format!("sedravasql_table.{}", self.name)
    }
    fn validate_row(&self, row: &Row) -> Result<String> {
        if row.len() != self.columns.len() {
            return Err(Error::Invalid("table: row column set mismatch"));
        }
        for c in &self.columns {
            let ok = match (c.kind, row.get(&c.name)) {
                (ColumnType::Int, Some(Cell::Int(_))) | (ColumnType::Bool, Some(Cell::Bool(_))) => {
                    true
                }
                (ColumnType::Text, Some(Cell::Text(t))) => {
                    t.chars().count() <= 1024 && !t.chars().any(char::is_control)
                }
                _ => false,
            };
            if !ok {
                return Err(Error::Invalid(
                    "table: missing column or invalid scalar type/value",
                ));
            }
        }
        match row.get(&self.primary_key) {
            Some(Cell::Text(k)) if valid_id(k) => Ok(k.clone()),
            _ => Err(Error::Invalid("table: invalid primary key")),
        }
    }
}
/// A heterogeneous batch of typed row changes across tables in one journal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TableMutation {
    Put {
        table: String,
        expected_revision: u64,
        row: Row,
    },
    Delete {
        table: String,
        key: String,
        expected_revision: u64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexDefinition {
    pub name: String,
    pub table: String,
    pub column: String,
    pub schema_sha256: String,
}
pub(crate) type IndexCache = BTreeMap<(String, String), BTreeMap<Cell, Vec<String>>>;
const INDEX_SCAN: usize = 4096;
const INDEX_CACHE_MAX: usize = 4;
fn index_namespace(table: &str) -> String {
    format!("sedravasql_index_catalog.{table}")
}
pub struct Tables<'a> {
    db: &'a mut NativeLog,
}
impl<'a> Tables<'a> {
    pub fn new(db: &'a mut NativeLog) -> Self {
        Self { db }
    }
    pub(crate) fn prepare_create(&self, schema: &TableSchema) -> Result<NativeMutation> {
        Ok(NativeMutation {
            namespace: CATALOG.into(),
            key: schema.name.clone(),
            expected_revision: 0,
            change: NativeChange::Put(schema.encoded()?),
        })
    }
    pub fn create(&mut self, request: &str, schema: &TableSchema) -> Result<NativeCommit> {
        let op = self.prepare_create(schema)?;
        self.db.commit_batch(request, &[op])
    }
    pub fn schema(&self, table: &str) -> Result<Option<TableSchema>> {
        if !identifier(table) {
            return Err(Error::Invalid("table: invalid name"));
        }
        let Some(row) = self.db.read(CATALOG, table)? else {
            return Ok(None);
        };
        let value = row
            .value
            .ok_or(Error::Invalid("table: deleted schema unsupported"))?;
        let schema: TableSchema = serde_json::from_str(&value)
            .map_err(|_| Error::Invalid("table: invalid stored schema"))?;
        schema.validate()?;
        if schema.name != table || row.revision != 1 {
            return Err(Error::Invalid("table: schema identity/revision mismatch"));
        }
        Ok(Some(schema))
    }
    fn required(&self, table: &str) -> Result<TableSchema> {
        self.schema(table)?
            .ok_or(Error::Invalid("table: unknown table"))
    }
    pub(crate) fn prepare_mutation(&self, change: &TableMutation) -> Result<NativeMutation> {
        match change {
            TableMutation::Put {
                table,
                expected_revision,
                row,
            } => {
                let schema = self.required(table)?;
                let key = schema.validate_row(row)?;
                let encoded = serde_json::to_string(&StoredRow {
                    schema_sha256: schema.hash()?,
                    values: row.clone(),
                })
                .map_err(|_| Error::Invalid("table: row encoding"))?;
                Ok(NativeMutation {
                    namespace: schema.namespace(),
                    key,
                    expected_revision: *expected_revision,
                    change: NativeChange::Put(encoded),
                })
            }
            TableMutation::Delete {
                table,
                key,
                expected_revision,
            } => {
                let schema = self.required(table)?;
                Ok(NativeMutation {
                    namespace: schema.namespace(),
                    key: key.clone(),
                    expected_revision: *expected_revision,
                    change: NativeChange::Delete,
                })
            }
        }
    }
    pub fn batch(&mut self, request: &str, changes: &[TableMutation]) -> Result<NativeCommit> {
        if changes.is_empty() || changes.len() > super::MAX_NATIVE_BATCH {
            return Err(Error::Invalid("table: batch requires 1..16 row changes"));
        }
        let ops = changes
            .iter()
            .map(|c| self.prepare_mutation(c))
            .collect::<Result<Vec<_>>>()?;
        self.db.commit_batch(request, &ops)
    }
    pub fn put(
        &mut self,
        request: &str,
        table: &str,
        expected: u64,
        row: Row,
    ) -> Result<NativeCommit> {
        self.batch(
            request,
            &[TableMutation::Put {
                table: table.into(),
                expected_revision: expected,
                row,
            }],
        )
    }
    pub fn delete(
        &mut self,
        request: &str,
        table: &str,
        key: &str,
        expected: u64,
    ) -> Result<NativeCommit> {
        self.batch(
            request,
            &[TableMutation::Delete {
                table: table.into(),
                key: key.into(),
                expected_revision: expected,
            }],
        )
    }
    fn decode(schema: &TableSchema, row: NativeEntry) -> Result<TableRow> {
        let values = match row.value {
            None => None,
            Some(value) => {
                let stored: StoredRow = serde_json::from_str(&value)
                    .map_err(|_| Error::Invalid("table: invalid stored row"))?;
                if stored.schema_sha256 != schema.hash()?
                    || schema.validate_row(&stored.values)? != row.key
                {
                    return Err(Error::Invalid("table: stored row schema/key mismatch"));
                }
                Some(stored.values)
            }
        };
        Ok(TableRow {
            key: row.key,
            revision: row.revision,
            values,
        })
    }
    pub fn index(&self, table: &str, column: &str) -> Result<Option<IndexDefinition>> {
        let schema = self.required(table)?;
        if !schema.columns.iter().any(|c| c.name == column) {
            return Err(Error::Invalid("index: unknown column"));
        }
        let Some(row) = self.db.read(&index_namespace(table), column)? else {
            return Ok(None);
        };
        let raw = row
            .value
            .ok_or(Error::Invalid("index: unexpected tombstone"))?;
        let definition: IndexDefinition =
            serde_json::from_str(&raw).map_err(|_| Error::Invalid("index: invalid definition"))?;
        if row.revision != 1
            || definition.table != table
            || definition.column != column
            || definition.schema_sha256 != schema.hash()?
            || !identifier(&definition.name)
        {
            return Err(Error::Invalid("index: identity/schema mismatch"));
        }
        Ok(Some(definition))
    }
    pub(crate) fn prepare_index(
        &self,
        name: &str,
        table: &str,
        column: &str,
    ) -> Result<NativeMutation> {
        let schema = self.required(table)?;
        if !identifier(name)
            || column == schema.primary_key
            || !schema.columns.iter().any(|c| c.name == column)
        {
            return Err(Error::Invalid("index: invalid name or secondary column"));
        }
        let ns = index_namespace(table);
        for row in self.db.scan_namespace(&ns, None, 16)? {
            let existing = self
                .index(table, &row.key)?
                .ok_or(Error::Invalid("index: missing definition"))?;
            if existing.name == name || existing.column == column {
                return Err(Error::Conflict("index: existing name or column".into()));
            }
        }
        let def = IndexDefinition {
            name: name.into(),
            table: table.into(),
            column: column.into(),
            schema_sha256: schema.hash()?,
        };
        Ok(NativeMutation {
            namespace: ns,
            key: column.into(),
            expected_revision: 0,
            change: NativeChange::Put(
                serde_json::to_string(&def).map_err(|_| Error::Invalid("index: encoding"))?,
            ),
        })
    }
    pub fn index_cached(&self, table: &str, column: &str) -> bool {
        self.db
            .secondary_cache
            .borrow()
            .contains_key(&(table.into(), column.into()))
    }
    /// Definition persists in the journal. Rebuild a bounded in-memory ordered
    /// index from validated rows on first access after open or table changes.
    pub fn index_lookup(
        &self,
        table: &str,
        column: &str,
        value: &Cell,
    ) -> Result<Option<Vec<TableRow>>> {
        if self.index(table, column)?.is_none() {
            return Ok(None);
        }
        let cache_key = (table.to_owned(), column.to_owned());
        if !self.db.secondary_cache.borrow().contains_key(&cache_key) {
            let mut map: BTreeMap<Cell, Vec<String>> = BTreeMap::new();
            let mut after = None;
            let mut seen = 0;
            loop {
                let page = self.scan(table, after.as_deref(), 256)?;
                if page.is_empty() {
                    break;
                }
                for row in page {
                    seen += 1;
                    if seen > INDEX_SCAN {
                        return Err(Error::Capacity("index: rebuild scan budget"));
                    }
                    after = Some(row.key.clone());
                    if let Some(values) = row.values {
                        let v = values
                            .get(column)
                            .ok_or(Error::Invalid("index: missing value"))?;
                        map.entry(v.clone()).or_default().push(row.key);
                    }
                }
            }
            let mut cache = self.db.secondary_cache.borrow_mut();
            if cache.len() >= INDEX_CACHE_MAX {
                if let Some(first) = cache.keys().next().cloned() {
                    cache.remove(&first);
                }
            }
            cache.insert(cache_key.clone(), map);
        }
        let keys = self
            .db
            .secondary_cache
            .borrow()
            .get(&cache_key)
            .and_then(|m| m.get(value))
            .cloned()
            .unwrap_or_default();
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            let row = self
                .get(table, &key)?
                .ok_or(Error::Invalid("index: missing indexed row"))?;
            if row.values.as_ref().and_then(|r| r.get(column)) != Some(value) {
                return Err(Error::Invalid("index: stale indexed row"));
            }
            rows.push(row);
        }
        Ok(Some(rows))
    }
    pub fn get(&self, table: &str, key: &str) -> Result<Option<TableRow>> {
        let schema = self.required(table)?;
        self.db
            .read(&schema.namespace(), key)?
            .map(|r| Self::decode(&schema, r))
            .transpose()
    }
    /// Primary-key ordered, exclusive after key, includes tombstones.
    pub fn scan(&self, table: &str, after: Option<&str>, limit: usize) -> Result<Vec<TableRow>> {
        let schema = self.required(table)?;
        self.db
            .scan_namespace(&schema.namespace(), after, limit)?
            .into_iter()
            .map(|r| Self::decode(&schema, r))
            .collect()
    }
}
