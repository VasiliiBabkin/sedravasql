//! B16b: bounded single-statement SQL subset over the native table adapter.
//! No external SQL engine. Identifiers case-sensitive; keywords case-insensitive.
use super::{valid_id, Error, NativeChange, NativeLog, NativeMutation, Result, TransactionalStore};
use crate::table::{Cell, Column, ColumnType, Row, TableMutation, TableSchema, Tables};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};
const RECEIPTS: &str = "sedravasql_sql_receipts";
const MAX_SQL: usize = 16_384;
const MAX_SCAN: usize = 4096;
const MAX_WRITE: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Text(String),
    Int(i32),
    P(char),
    Op(String),
    Param,
    Bound(Cell),
}
fn lex(sql: &str) -> Result<Vec<Tok>> {
    if sql.len() > MAX_SQL {
        return Err(Error::Capacity("sql: statement too large"));
    }
    let mut it = sql.chars().peekable();
    let mut out = Vec::new();
    while let Some(c) = it.next() {
        let tok = match c {
            c if c.is_ascii_whitespace() => continue,
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut w = String::from(c);
                while it
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    w.push(it.next().unwrap());
                }
                Tok::Word(w)
            }
            c if c.is_ascii_digit() || c == '-' => {
                let mut n = String::from(c);
                while it.peek().is_some_and(|c| c.is_ascii_digit()) {
                    n.push(it.next().unwrap());
                }
                Tok::Int(
                    n.parse()
                        .map_err(|_| Error::Invalid("sql: invalid i32 literal"))?,
                )
            }
            '\'' => {
                let mut v = String::new();
                loop {
                    match it.next() {
                        Some('\'') if it.peek() == Some(&'\'') => {
                            it.next();
                            v.push('\'');
                        }
                        Some('\'') => break,
                        Some(c) if !c.is_control() => v.push(c),
                        _ => return Err(Error::Invalid("sql: invalid or unterminated string")),
                    }
                }
                if v.chars().count() > 1024 {
                    return Err(Error::Capacity("sql: string literal too large"));
                }
                Tok::Text(v)
            }
            '?' => Tok::Param,
            '(' | ')' | ',' | '*' | ';' => Tok::P(c),
            '=' | '<' | '>' | '!' => {
                let mut op = String::from(c);
                if (c == '<' && it.peek() == Some(&'>')) || (c != '=' && it.peek() == Some(&'=')) {
                    op.push(it.next().unwrap());
                }
                if op == "!" {
                    return Err(Error::Invalid("sql: invalid comparison"));
                }
                Tok::Op(op)
            }
            _ => return Err(Error::Invalid("sql: unexpected character")),
        };
        out.push(tok);
        if out.len() > 512 {
            return Err(Error::Capacity("sql: token limit"));
        }
    }
    Ok(out)
}
#[derive(Debug, Clone, Serialize)]
struct Predicate {
    column: String,
    op: String,
    value: Cell,
}
#[derive(Debug, Clone, Serialize)]
enum Projection {
    Column(String),
    Aggregate {
        function: String,
        column: Option<String>,
        label: String,
    },
}
#[derive(Debug, Clone, Serialize)]
enum Statement {
    Create(TableSchema),
    Insert {
        table: String,
        columns: Vec<String>,
        values: Vec<Cell>,
    },
    Select {
        table: String,
        columns: Vec<String>,
        where_: Vec<Predicate>,
        order: Option<(String, bool)>,
        limit: usize,
    },
    Aggregate {
        table: String,
        projections: Vec<Projection>,
        where_: Vec<Predicate>,
        group: Option<String>,
        order: Option<(String, bool)>,
        limit: usize,
    },
    Update {
        table: String,
        assignments: Row,
        where_: Vec<Predicate>,
    },
    Delete {
        table: String,
        where_: Vec<Predicate>,
    },
}
struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}
impl Parser {
    fn next(&mut self) -> Option<Tok> {
        let v = self.tokens.get(self.pos).cloned();
        if v.is_some() {
            self.pos += 1;
        }
        v
    }
    fn word(&mut self, w: &str) -> bool {
        if matches!(self.tokens.get(self.pos),Some(Tok::Word(v)) if v.eq_ignore_ascii_case(w)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn require_word(&mut self, w: &str) -> Result<()> {
        if self.word(w) {
            Ok(())
        } else {
            Err(Error::Invalid("sql: expected keyword"))
        }
    }
    fn punct(&mut self, c: char) -> bool {
        if self.tokens.get(self.pos) == Some(&Tok::P(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn require_punct(&mut self, c: char) -> Result<()> {
        if self.punct(c) {
            Ok(())
        } else {
            Err(Error::Invalid("sql: expected punctuation"))
        }
    }
    fn ident(&mut self) -> Result<String> {
        match self.next() {
            Some(Tok::Word(v))
                if v.len() <= 48
                    && v.as_bytes()[0].is_ascii_alphabetic()
                    && ![
                        "SELECT", "FROM", "WHERE", "ORDER", "BY", "LIMIT", "INSERT", "INTO",
                        "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "PRIMARY", "KEY",
                        "INT", "INTEGER", "TEXT", "BOOL", "BOOLEAN", "TRUE", "FALSE", "AND", "OR",
                        "ASC", "DESC", "NULL",
                    ]
                    .iter()
                    .any(|k| v.eq_ignore_ascii_case(k)) =>
            {
                Ok(v)
            }
            _ => Err(Error::Invalid("sql: expected nonreserved identifier")),
        }
    }
    fn literal(&mut self) -> Result<Cell> {
        match self.next() {
            Some(Tok::Bound(v)) => Ok(v),
            Some(Tok::Text(v)) => Ok(Cell::Text(v)),
            Some(Tok::Int(v)) => Ok(Cell::Int(v)),
            Some(Tok::Word(v)) if v.eq_ignore_ascii_case("true") => Ok(Cell::Bool(true)),
            Some(Tok::Word(v)) if v.eq_ignore_ascii_case("false") => Ok(Cell::Bool(false)),
            _ => Err(Error::Invalid("sql: expected scalar literal")),
        }
    }
    fn equal(&mut self) -> Result<()> {
        if self.next() == Some(Tok::Op("=".into())) {
            Ok(())
        } else {
            Err(Error::Invalid("sql: expected ="))
        }
    }
    fn names(&mut self) -> Result<Vec<String>> {
        let mut cols = vec![self.ident()?];
        while self.punct(',') {
            cols.push(self.ident()?);
            if cols.len() > 16 {
                return Err(Error::Capacity("sql: columns"));
            }
        }
        unique(&cols)?;
        Ok(cols)
    }
    fn where_clause(&mut self, required: bool) -> Result<Vec<Predicate>> {
        if !self.word("WHERE") {
            return if required {
                Err(Error::Invalid("sql: mutation requires WHERE"))
            } else {
                Ok(vec![])
            };
        }
        let mut out = Vec::new();
        loop {
            let column = self.ident()?;
            let op = match self.next() {
                Some(Tok::Op(v)) => v,
                _ => return Err(Error::Invalid("sql: expected comparison")),
            };
            out.push(Predicate {
                column,
                op,
                value: self.literal()?,
            });
            if out.len() > 16 {
                return Err(Error::Capacity("sql: predicate limit"));
            }
            if !self.word("AND") {
                break;
            }
        }
        Ok(out)
    }
    fn parse(mut self) -> Result<Statement> {
        let result = if self.word("CREATE") {
            self.require_word("TABLE")?;
            let name = self.ident()?;
            self.require_punct('(')?;
            let mut columns = Vec::new();
            let mut pk = None;
            loop {
                let col = self.ident()?;
                let kind = if self.word("INT") || self.word("INTEGER") {
                    ColumnType::Int
                } else if self.word("BOOL") || self.word("BOOLEAN") {
                    ColumnType::Bool
                } else if self.word("TEXT") {
                    ColumnType::Text
                } else {
                    return Err(Error::Invalid("sql: unknown column type"));
                };
                if self.word("PRIMARY") {
                    self.require_word("KEY")?;
                    if pk.replace(col.clone()).is_some() {
                        return Err(Error::Invalid("sql: multiple primary keys"));
                    }
                }
                columns.push(Column { name: col, kind });
                if columns.len() > 16 {
                    return Err(Error::Capacity("sql: column limit"));
                }
                if !self.punct(',') {
                    break;
                }
            }
            self.require_punct(')')?;
            let schema = TableSchema {
                name,
                primary_key: pk.ok_or(Error::Invalid("sql: primary key required"))?,
                columns,
            };
            schema.validate()?;
            Statement::Create(schema)
        } else if self.word("INSERT") {
            self.require_word("INTO")?;
            let table = self.ident()?;
            self.require_punct('(')?;
            let columns = self.names()?;
            self.require_punct(')')?;
            self.require_word("VALUES")?;
            self.require_punct('(')?;
            let mut values = vec![self.literal()?];
            while self.punct(',') {
                values.push(self.literal()?);
                if values.len() > 16 {
                    return Err(Error::Capacity("sql: values limit"));
                }
            }
            self.require_punct(')')?;
            if columns.len() != values.len() {
                return Err(Error::Invalid("sql: column/value count mismatch"));
            }
            Statement::Insert {
                table,
                columns,
                values,
            }
        } else if self.word("SELECT") {
            let star = self.punct('*');
            let mut projections = Vec::new();
            if !star {
                loop {
                    let name = self.ident()?;
                    if self.punct('(') {
                        let function = name.to_ascii_uppercase();
                        if !matches!(function.as_str(), "COUNT" | "SUM" | "MIN" | "MAX" | "AVG") {
                            return Err(Error::Invalid("sql: unsupported aggregate"));
                        }
                        let column = if self.punct('*') {
                            None
                        } else {
                            Some(self.ident()?)
                        };
                        if column.is_none() && function != "COUNT" {
                            return Err(Error::Invalid("sql: only COUNT accepts *"));
                        }
                        self.require_punct(')')?;
                        let label = if self.word("AS") {
                            self.ident()?
                        } else {
                            format!("{}({})", function, column.as_deref().unwrap_or("*"))
                        };
                        projections.push(Projection::Aggregate {
                            function,
                            column,
                            label,
                        });
                    } else {
                        projections.push(Projection::Column(name));
                    }
                    if projections.len() > 16 {
                        return Err(Error::Capacity("sql: projection limit"));
                    }
                    if !self.punct(',') {
                        break;
                    }
                }
            }
            self.require_word("FROM")?;
            let table = self.ident()?;
            let where_ = self.where_clause(false)?;
            let group = if self.word("GROUP") {
                self.require_word("BY")?;
                Some(self.ident()?)
            } else {
                None
            };
            let order = if self.word("ORDER") {
                self.require_word("BY")?;
                let col = self.ident()?;
                let desc = self.word("DESC");
                if !desc {
                    self.word("ASC");
                }
                Some((col, desc))
            } else {
                None
            };
            let limit = if self.word("LIMIT") {
                match self.next() {
                    Some(Tok::Int(n)) if (1..=256).contains(&n) => n as usize,
                    _ => return Err(Error::Invalid("sql: LIMIT must be 1..256")),
                }
            } else {
                100
            };
            if group.is_some()
                || projections
                    .iter()
                    .any(|p| matches!(p, Projection::Aggregate { .. }))
            {
                if star {
                    return Err(Error::Invalid("sql: aggregate SELECT * unsupported"));
                }
                Statement::Aggregate {
                    table,
                    projections,
                    where_,
                    group,
                    order,
                    limit,
                }
            } else {
                let columns = projections
                    .into_iter()
                    .map(|p| match p {
                        Projection::Column(n) => n,
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                unique(&columns)?;
                Statement::Select {
                    table,
                    columns,
                    where_,
                    order,
                    limit,
                }
            }
        } else if self.word("UPDATE") {
            let table = self.ident()?;
            self.require_word("SET")?;
            let mut assignments = Row::new();
            loop {
                let col = self.ident()?;
                self.equal()?;
                if assignments.insert(col, self.literal()?).is_some() {
                    return Err(Error::Invalid("sql: duplicate assignment"));
                }
                if assignments.len() > 16 {
                    return Err(Error::Capacity("sql: assignment limit"));
                }
                if !self.punct(',') {
                    break;
                }
            }
            Statement::Update {
                table,
                assignments,
                where_: self.where_clause(true)?,
            }
        } else if self.word("DELETE") {
            self.require_word("FROM")?;
            let table = self.ident()?;
            Statement::Delete {
                table,
                where_: self.where_clause(true)?,
            }
        } else {
            return Err(Error::Invalid("sql: unsupported statement"));
        };
        self.punct(';');
        if self.pos != self.tokens.len() {
            return Err(Error::Invalid(
                "sql: trailing tokens or multiple statements",
            ));
        }
        Ok(result)
    }
}
fn unique(names: &[String]) -> Result<()> {
    let mut set = HashSet::new();
    if names.iter().any(|n| !set.insert(n)) {
        Err(Error::Invalid("sql: duplicate column"))
    } else {
        Ok(())
    }
}
fn column(schema: &TableSchema, name: &str) -> Result<ColumnType> {
    schema
        .columns
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.kind)
        .ok_or(Error::Invalid("sql: unknown column"))
}
fn type_ok(kind: ColumnType, value: &Cell) -> bool {
    matches!(
        (kind, value),
        (ColumnType::Int, Cell::Int(_))
            | (ColumnType::Bool, Cell::Bool(_))
            | (ColumnType::Text, Cell::Text(_))
    )
}
fn cmp(a: &Cell, b: &Cell) -> Result<Ordering> {
    match (a, b) {
        (Cell::Int(a), Cell::Int(b)) => Ok(a.cmp(b)),
        (Cell::Bool(a), Cell::Bool(b)) => Ok(a.cmp(b)),
        (Cell::Text(a), Cell::Text(b)) => Ok(a.cmp(b)),
        _ => Err(Error::Invalid("sql: incompatible comparison")),
    }
}
fn predicates(schema: &TableSchema, where_: &[Predicate]) -> Result<()> {
    for p in where_ {
        let kind = column(schema, &p.column)?;
        if !type_ok(kind, &p.value)
            || (kind == ColumnType::Bool && !matches!(p.op.as_str(), "=" | "!=" | "<>"))
        {
            return Err(Error::Invalid("sql: invalid predicate type"));
        }
    }
    Ok(())
}
fn matches_row(row: &Row, where_: &[Predicate]) -> Result<bool> {
    for p in where_ {
        let order = cmp(
            row.get(&p.column)
                .ok_or(Error::Invalid("sql: missing stored column"))?,
            &p.value,
        )?;
        let yes = match p.op.as_str() {
            "=" => order == Ordering::Equal,
            "!=" | "<>" => order != Ordering::Equal,
            "<" => order == Ordering::Less,
            ">" => order == Ordering::Greater,
            "<=" => order != Ordering::Greater,
            ">=" => order != Ordering::Less,
            _ => return Err(Error::Invalid("sql: comparison")),
        };
        if !yes {
            return Ok(false);
        }
    }
    Ok(true)
}
fn matching(
    t: &Tables<'_>,
    schema: &TableSchema,
    where_: &[Predicate],
) -> Result<Vec<(String, u64, Row)>> {
    predicates(schema, where_)?;
    // Primary-key equality avoids scanning the rest of the table.
    if let Some(p) = where_
        .iter()
        .find(|p| p.column == schema.primary_key && p.op == "=")
    {
        if let Cell::Text(key) = &p.value {
            if !valid_id(key) {
                return Ok(vec![]);
            }
            if let Some(r) = t.get(&schema.name, key)? {
                if let Some(values) = r.values {
                    if matches_row(&values, where_)? {
                        return Ok(vec![(r.key, r.revision, values)]);
                    }
                }
            }
            return Ok(vec![]);
        }
    }
    let mut after = None;
    let mut seen = 0;
    let mut out = Vec::new();
    loop {
        let page = t.scan(&schema.name, after.as_deref(), 256)?;
        if page.is_empty() {
            break;
        }
        for row in page {
            seen += 1;
            if seen > MAX_SCAN {
                return Err(Error::Capacity("sql: scan budget exceeded"));
            }
            after = Some(row.key.clone());
            if let Some(values) = row.values {
                if matches_row(&values, where_)? {
                    out.push((row.key, row.revision, values));
                }
            }
        }
    }
    Ok(out)
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SqlResult {
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<Cell>>,
    },
    AggregateRows {
        columns: Vec<String>,
        rows: Vec<Vec<serde_json::Value>>,
    },
    Write {
        affected: usize,
        sequence: u64,
        replayed: bool,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
    affected: usize,
    sequence: u64,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn required_schema(t: &Tables<'_>, table: &str) -> Result<TableSchema> {
    t.schema(table)?.ok_or(Error::Invalid("sql: unknown table"))
}

/// Single statement; writes require caller-supplied stable request key.
/// Replays are checked before evaluating current rows; response receipt is
/// committed in the same frame, including zero-row mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlKind {
    Query,
    Write,
}

fn parse_bound(sql: &str, params: &[Cell]) -> Result<Statement> {
    if params.len() > 32 {
        return Err(Error::Capacity("sql: parameter limit"));
    }
    let mut tokens = lex(sql)?;
    let mut supplied = params.iter();
    for token in &mut tokens {
        if *token == Tok::Param {
            let value = supplied
                .next()
                .ok_or(Error::Invalid("sql: missing parameter"))?;
            if let Cell::Text(t) = value {
                if t.chars().count() > 1024 || t.chars().any(char::is_control) {
                    return Err(Error::Invalid("sql: invalid text parameter"));
                }
            }
            *token = Tok::Bound(value.clone());
        }
    }
    if supplied.next().is_some() {
        return Err(Error::Invalid("sql: surplus parameter"));
    }
    Parser { tokens, pos: 0 }.parse()
}
/// Syntax and binding validation only; schema-dependent checks execute on the journal.
pub fn check(sql: &str, params: &[Cell]) -> Result<SqlKind> {
    Ok(
        if matches!(
            parse_bound(sql, params)?,
            Statement::Select { .. } | Statement::Aggregate { .. }
        ) {
            SqlKind::Query
        } else {
            SqlKind::Write
        },
    )
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Int(i32),
    Bool(bool),
    Text(String),
    All,
}
fn key(c: &Cell) -> GroupKey {
    match c {
        Cell::Int(n) => GroupKey::Int(*n),
        Cell::Bool(b) => GroupKey::Bool(*b),
        Cell::Text(t) => GroupKey::Text(t.clone()),
    }
}
fn json_cell(c: &Cell) -> serde_json::Value {
    match c {
        Cell::Int(n) => serde_json::json!(n),
        Cell::Bool(b) => serde_json::json!(b),
        Cell::Text(t) => serde_json::json!(t),
    }
}
fn aggregate(function: &str, column: Option<&str>, rows: &[Row]) -> serde_json::Value {
    use serde_json::{json, Value};
    if function == "COUNT" {
        return json!(rows.len());
    }
    if rows.is_empty() {
        return Value::Null;
    }
    let col = column.expect("validated column");
    match function {
        "SUM" | "AVG" => {
            let sum: i64 = rows
                .iter()
                .map(|r| match r[col] {
                    Cell::Int(n) => i64::from(n),
                    _ => unreachable!(),
                })
                .sum();
            if function == "SUM" {
                json!(sum)
            } else {
                json!(sum as f64 / rows.len() as f64)
            }
        }
        "MIN" | "MAX" => {
            let mut best = &rows[0][col];
            for row in &rows[1..] {
                let v = &row[col];
                let o = cmp(v, best).expect("validated column");
                if (function == "MIN" && o == Ordering::Less)
                    || (function == "MAX" && o == Ordering::Greater)
                {
                    best = v;
                }
            }
            json_cell(best)
        }
        _ => unreachable!(),
    }
}
fn result_cmp(a: &serde_json::Value, b: &serde_json::Value) -> Ordering {
    use serde_json::Value;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => a.as_f64().unwrap().total_cmp(&b.as_f64().unwrap()),
        },
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => unreachable!("homogeneous validated projections"),
    }
}
fn aggregate_query(
    t: &Tables<'_>,
    schema: &TableSchema,
    projections: &[Projection],
    where_: &[Predicate],
    group: Option<&str>,
    order: Option<&(String, bool)>,
    limit: usize,
) -> Result<SqlResult> {
    if let Some(g) = group {
        column(schema, g)?;
    }
    let mut labels = Vec::new();
    for p in projections {
        match p {
            Projection::Column(c) => {
                if group != Some(c.as_str()) {
                    return Err(Error::Invalid("sql: projected column must be GROUP BY key"));
                }
                column(schema, c)?;
                labels.push(c.clone());
            }
            Projection::Aggregate {
                function,
                column: c,
                label,
            } => {
                if let Some(c) = c {
                    let kind = column(schema, c)?;
                    if matches!(function.as_str(), "SUM" | "AVG") && kind != ColumnType::Int {
                        return Err(Error::Invalid("sql: SUM/AVG require INT"));
                    }
                }
                labels.push(label.clone());
            }
        }
    }
    unique(&labels)?;
    let sort = if let Some((c, d)) = order {
        Some((
            labels.iter().position(|n| n == c).ok_or(Error::Invalid(
                "sql: aggregate ORDER BY requires projected name or alias",
            ))?,
            *d,
        ))
    } else {
        None
    };
    let mut groups: BTreeMap<GroupKey, Vec<Row>> = BTreeMap::new();
    if group.is_none() {
        groups.insert(GroupKey::All, vec![]);
    }
    for (_, _, row) in matching(t, schema, where_)? {
        let k = group.map(|g| key(&row[g])).unwrap_or(GroupKey::All);
        groups.entry(k).or_default().push(row);
    }
    let mut rows: Vec<Vec<serde_json::Value>> = groups
        .into_values()
        .map(|rows| {
            projections
                .iter()
                .map(|p| match p {
                    Projection::Column(c) => json_cell(&rows[0][c]),
                    Projection::Aggregate {
                        function, column, ..
                    } => aggregate(function, column.as_deref(), &rows),
                })
                .collect()
        })
        .collect();
    if let Some((i, desc)) = sort {
        rows.sort_by(|a, b| {
            let o = result_cmp(&a[i], &b[i]);
            if desc {
                o.reverse()
            } else {
                o
            }
        });
    }
    rows.truncate(limit);
    Ok(SqlResult::AggregateRows {
        columns: labels,
        rows,
    })
}
pub fn execute(db: &mut NativeLog, sql: &str, request: Option<&str>) -> Result<SqlResult> {
    execute_bound(db, sql, &[], request)
}
pub fn execute_bound(
    db: &mut NativeLog,
    sql: &str,
    params: &[Cell],
    request: Option<&str>,
) -> Result<SqlResult> {
    let stmt = parse_bound(sql, params)?;
    if let Statement::Aggregate {
        table,
        projections,
        where_,
        group,
        order,
        limit,
    } = &stmt
    {
        if request.is_some() {
            return Err(Error::Invalid("sql: SELECT does not take request key"));
        }
        let t = Tables::new(db);
        let schema = required_schema(&t, table)?;
        return aggregate_query(
            &t,
            &schema,
            projections,
            where_,
            group.as_deref(),
            order.as_ref(),
            *limit,
        );
    }
    if let Statement::Select {
        table,
        columns,
        where_,
        order,
        limit,
    } = &stmt
    {
        if request.is_some() {
            return Err(Error::Invalid("sql: SELECT does not take request key"));
        }
        let t = Tables::new(db);
        let schema = required_schema(&t, table)?;
        let cols = if columns.is_empty() {
            schema.columns.iter().map(|c| c.name.clone()).collect()
        } else {
            columns.clone()
        };
        for c in &cols {
            column(&schema, c)?;
        }
        if let Some((c, _)) = order {
            column(&schema, c)?;
        }
        let mut rows = matching(&t, &schema, where_)?;
        if let Some((c, desc)) = order {
            // All rows have already been validated against the same schema.
            rows.sort_by(|a, b| {
                let o = cmp(&a.2[c], &b.2[c]).expect("validated homogeneous columns");
                let o = if *desc { o.reverse() } else { o };
                o.then_with(|| a.0.cmp(&b.0))
            });
        }
        let rows = rows
            .into_iter()
            .take(*limit)
            .map(|(_, _, r)| cols.iter().map(|c| r[c].clone()).collect())
            .collect();
        return Ok(SqlResult::Rows {
            columns: cols,
            rows,
        });
    }
    let request = request.ok_or(Error::Invalid("sql: write requires request key"))?;
    if request.len() < 12 || !valid_id(request) {
        return Err(Error::Invalid("sql: invalid request key"));
    }
    let key = format!("s{}", hash(request.as_bytes()));
    let fingerprint =
        hash(&serde_json::to_vec(&stmt).map_err(|_| Error::Invalid("sql: fingerprint encoding"))?);
    let stored = db.read(RECEIPTS, &key)?;
    let engine = db.lookup_receipt(&key)?;
    match (stored, engine) {
        (Some(row), Some(commit)) => {
            let receipt: Receipt = serde_json::from_str(
                row.value
                    .as_deref()
                    .ok_or(Error::Invalid("sql: deleted receipt"))?,
            )
            .map_err(|_| Error::Invalid("sql: invalid receipt"))?;
            if row.revision != 1
                || receipt.sequence != commit.sequence
                || receipt.affected.checked_add(1) != Some(commit.entries.len())
                || commit.entries.last() != Some(&row)
            {
                return Err(Error::Invalid("sql: inconsistent receipt"));
            }
            if receipt.fingerprint != fingerprint {
                return Err(Error::Conflict(
                    "sql: request key reused with different statement".into(),
                ));
            }
            return Ok(SqlResult::Write {
                affected: receipt.affected,
                sequence: receipt.sequence,
                replayed: true,
            });
        }
        (None, None) => {}
        _ => return Err(Error::Invalid("sql: incomplete receipt")),
    }
    let mut ops = Vec::new();
    let t = Tables::new(db);
    match stmt {
        Statement::Create(schema) => ops.push(t.prepare_create(&schema)?),
        Statement::Insert {
            table,
            columns,
            values,
        } => {
            let row = columns.into_iter().zip(values).collect();
            ops.push(t.prepare_mutation(&TableMutation::Put {
                table,
                expected_revision: 0,
                row,
            })?);
        }
        Statement::Update {
            table,
            assignments,
            where_,
        } => {
            let schema = required_schema(&t, &table)?;
            for (c, v) in &assignments {
                if c == &schema.primary_key || !type_ok(column(&schema, c)?, v) {
                    return Err(Error::Invalid(
                        "sql: invalid SET column/type or primary-key change",
                    ));
                }
            }
            let rows = matching(&t, &schema, &where_)?;
            if rows.len() > MAX_WRITE {
                return Err(Error::Capacity("sql: mutation matches more than 15 rows"));
            }
            for (_, rev, mut row) in rows {
                row.extend(assignments.clone());
                ops.push(t.prepare_mutation(&TableMutation::Put {
                    table: table.clone(),
                    expected_revision: rev,
                    row,
                })?);
            }
        }
        Statement::Delete { table, where_ } => {
            let schema = required_schema(&t, &table)?;
            let rows = matching(&t, &schema, &where_)?;
            if rows.len() > MAX_WRITE {
                return Err(Error::Capacity("sql: mutation matches more than 15 rows"));
            }
            for (key, rev, _) in rows {
                ops.push(t.prepare_mutation(&TableMutation::Delete {
                    table: table.clone(),
                    key,
                    expected_revision: rev,
                })?);
            }
        }
        Statement::Select { .. } | Statement::Aggregate { .. } => unreachable!(),
    }
    let affected = ops.len();
    let sequence = db
        .sequence()
        .checked_add(1)
        .ok_or(Error::Capacity("sql: sequence overflow"))?;
    let receipt = Receipt {
        fingerprint,
        affected,
        sequence,
    };
    ops.push(NativeMutation {
        namespace: RECEIPTS.into(),
        key: key.clone(),
        expected_revision: 0,
        change: NativeChange::Put(
            serde_json::to_string(&receipt).map_err(|_| Error::Invalid("sql: receipt encoding"))?,
        ),
    });
    let commit = db.commit_batch(&key, &ops)?;
    Ok(SqlResult::Write {
        affected,
        sequence: commit.sequence,
        replayed: commit.replayed,
    })
}

#[derive(Debug, Serialize)]
pub struct QueryPlan {
    pub access: &'static str,
    pub aggregate: bool,
    pub group_by: Option<String>,
    pub sort: bool,
    pub limit: usize,
}
pub fn explain(db: &mut NativeLog, sql: &str, params: &[Cell]) -> Result<QueryPlan> {
    let stmt = parse_bound(sql, params)?;
    let (table, where_, aggregate, group_by, sort, limit) = match stmt {
        Statement::Select {
            table,
            where_,
            order,
            limit,
            ..
        } => (table, where_, false, None, order.is_some(), limit),
        Statement::Aggregate {
            table,
            where_,
            group,
            order,
            limit,
            ..
        } => (table, where_, true, group, order.is_some(), limit),
        _ => return Err(Error::Invalid("sql: explain requires SELECT")),
    };
    let t = Tables::new(db);
    let schema = required_schema(&t, &table)?;
    predicates(&schema, &where_)?;
    let access = if where_
        .iter()
        .any(|p| p.column == schema.primary_key && p.op == "=")
    {
        "primary_key_lookup"
    } else {
        "bounded_table_scan"
    };
    Ok(QueryPlan {
        access,
        aggregate,
        group_by,
        sort,
        limit,
    })
}
