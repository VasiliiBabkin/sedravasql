//! Explicitly experimental CLI for the SQLite-free SedravaSQL append-log.
//! NOT the existing stable or alpha SQLite CLI. Do not use for valuable data.
use sedravasql_native_engine::archive::{create_archive, restore_archive, verify_archive};
use sedravasql_native_engine::backup::{backup_to_new_file, plan_backup};
use sedravasql_native_engine::index::{
    create_index, lookup_index, verify_index, RangeCursor, VerifiedIndexReader,
};
use sedravasql_native_engine::recovery::{plan_recovery, recover_to_new_file};
use sedravasql_native_engine::restore::restore_to_new_file;
use sedravasql_native_engine::snapshot::{create_snapshot, verify_snapshot};
use sedravasql_native_engine::table::{Row, TableMutation, TableSchema, Tables};
use sedravasql_native_engine::{NativeLimits, NativeLog, NativeMutation, TransactionalStore};
use std::{env, error::Error, fs, path::Path};

fn usage() -> &'static str {
    "USAGE: sedravasql-native-preview <init FILE | verify FILE | read FILE NAMESPACE KEY | read-many FILE REQUESTS_JSON | receipt FILE REQUEST_KEY | batch FILE REQUEST_KEY JSON_FILE [MAX_BYTES MAX_TRANSACTIONS] | usage FILE [MAX_BYTES MAX_TRANSACTIONS] | recovery-plan FILE | recovery-copy SOURCE DEST EXPECTED_SHA256 | sql FILE SQL [REQUEST_KEY] | sql-transaction FILE REQUEST_KEY STATEMENTS_JSON | sql-sequential FILE REQUEST_KEY STATEMENTS_JSON | explain FILE SQL [PARAMS_JSON] | sql-bind FILE SQL PARAMS_JSON [REQUEST_KEY] | table-batch FILE REQUEST_KEY CHANGES_JSON | table-create FILE REQUEST_KEY SCHEMA_JSON | table-schema FILE TABLE | table-put FILE REQUEST_KEY TABLE EXPECTED_REVISION ROW_JSON | table-delete FILE REQUEST_KEY TABLE KEY EXPECTED_REVISION | table-get FILE TABLE KEY | table-scan FILE TABLE LIMIT [AFTER] | archive-create SOURCE DEST EXPECTED_SHA256 | archive-verify ARCHIVE | archive-restore ARCHIVE DEST EXPECTED_SHA256 | backup-plan SOURCE | backup-copy SOURCE DEST EXPECTED_SHA256 | snapshot-create SOURCE DEST EXPECTED_SHA256 | snapshot-verify SNAPSHOT | restore-copy SNAPSHOT NEW_DEST APPROVED_SNAPSHOT_SHA256 | tree-init FILE SLOTS | tree-verify FILE | tree-get FILE KEY | tree-range FILE START END LIMIT | tree-put FILE KEY EXPECTED_REVISION VALUE_JSON | pager-init FILE SLOTS | pager-verify FILE | pager-get FILE NAMESPACE KEY | pager-commit FILE CHANGES_JSON | page-create SNAPSHOT NEW_IMAGE APPROVED_SHA256 | migration-copy LOG NEW_IMAGE | migration-info IMAGE | migration-read IMAGE NAMESPACE KEY | migration-receipt IMAGE REQUEST_KEY | migration-match-log IMAGE LOG | page-build-log LOG NEW_IMAGE | page-refresh LOG IMAGE STAGING APPROVED_OLD_SHA256 | page-verify IMAGE | page-get IMAGE NAMESPACE KEY | page-match-log IMAGE LOG | index-create SNAPSHOT NEW_INDEX APPROVED_SNAPSHOT_SHA256 | index-verify INDEX | index-get INDEX NAMESPACE KEY | index-get-many INDEX REQUESTS_JSON | index-range INDEX NAMESPACE START END LIMIT | index-page INDEX NAMESPACE START END LIMIT [CURSOR_JSON]>\n\
     EXPERIMENTAL / SINGLE WRITER / NO AUTOMATIC RECOVERY / NOT FOR VALUABLE DATA"
}
fn limits(args: &[String]) -> Result<NativeLimits, Box<dyn Error>> {
    match args {
        [] => Ok(NativeLimits::default()),
        [bytes, tx] => Ok(NativeLimits {
            max_bytes: bytes.parse()?,
            max_transactions: tx.parse()?,
        }
        .validate()?),
        _ => Err("expected both MAX_BYTES MAX_TRANSACTIONS".into()),
    }
}
fn table_input<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, Box<dyn Error>> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > 65536 {
        return Err("table: invalid JSON input file".into());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args {
        #[cfg(unix)]
        [_, cmd, log, dest] if cmd == "migration-copy" => {
            let m = sedravasql_native_engine::cow_migration::migrate_copy(
                Path::new(log),
                Path::new(dest),
            )?;
            println!("{}", serde_json::to_string(&m)?);
        }
        #[cfg(unix)]
        [_, cmd, file] if cmd == "migration-info" => {
            let image =
                sedravasql_native_engine::cow_migration::MigrationImage::open(Path::new(file))?;
            println!("{}", serde_json::to_string(image.manifest())?);
        }
        #[cfg(unix)]
        [_, cmd, file, namespace, key] if cmd == "migration-read" => {
            let mut image =
                sedravasql_native_engine::cow_migration::MigrationImage::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&image.entry(namespace, key)?)?);
        }
        #[cfg(unix)]
        [_, cmd, file, key] if cmd == "migration-receipt" => {
            let mut image =
                sedravasql_native_engine::cow_migration::MigrationImage::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&image.receipt(key)?)?);
        }
        #[cfg(unix)]
        [_, cmd, file, log] if cmd == "migration-match-log" => {
            let mut image =
                sedravasql_native_engine::cow_migration::MigrationImage::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::json!({"matches":image.matches_source(Path::new(log))?})
            );
        }
        [_, cmd, log, dest] if cmd == "page-build-log" => {
            let mut db = NativeLog::open(Path::new(log))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::pages::build_from_log(
                    &mut db,
                    Path::new(dest)
                )?)?
            );
        }
        [_, cmd, log, image, staging, approved] if cmd == "page-refresh" => {
            let mut db = NativeLog::open(Path::new(log))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::pages::refresh(
                    &mut db,
                    Path::new(image),
                    Path::new(staging),
                    approved
                )?)?
            );
        }
        [_, cmd, source, dest, approved] if cmd == "page-create" => {
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::pages::create(
                    Path::new(source),
                    Path::new(dest),
                    approved
                )?)?
            );
        }
        [_, cmd, file] if cmd == "page-verify" => {
            println!(
                "{}",
                serde_json::to_string(
                    &sedravasql_native_engine::pages::PageReader::open(Path::new(file))?.report()
                )?
            );
        }
        [_, cmd, file, namespace, key] if cmd == "page-get" => {
            let mut reader = sedravasql_native_engine::pages::PageReader::open(Path::new(file))?;
            let row = reader.get(namespace, key)?;
            println!(
                "{}",
                serde_json::json!({"row":row,"pages_read_after_open":reader.pages_read()})
            );
        }
        [_, cmd, file, log] if cmd == "page-match-log" => {
            let reader = sedravasql_native_engine::pages::PageReader::open(Path::new(file))?;
            let mut log = NativeLog::open(Path::new(log))?;
            println!(
                "{}",
                serde_json::json!({"matches":reader.matches_log(&mut log)?})
            );
        }
        [_, cmd, file, slots] if cmd == "tree-init" => {
            let tree =
                sedravasql_native_engine::tree::Tree::create(Path::new(file), slots.parse()?)?;
            println!("{}", serde_json::to_string(&tree.report())?);
        }
        [_, cmd, file] if cmd == "tree-verify" => {
            let tree = sedravasql_native_engine::tree::Tree::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&tree.report())?);
        }
        [_, cmd, file, key] if cmd == "tree-get" => {
            let mut tree = sedravasql_native_engine::tree::Tree::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&tree.get(key)?)?);
        }
        [_, cmd, file, start, end, limit] if cmd == "tree-range" => {
            let mut tree = sedravasql_native_engine::tree::Tree::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&tree.range(start, end, limit.parse()?)?)?
            );
        }
        [_, cmd, file, key, rev, value] if cmd == "tree-put" => {
            let mut tree = sedravasql_native_engine::tree::Tree::open(Path::new(file))?;
            let value: Option<String> = table_input(value)?;
            println!(
                "{}",
                serde_json::to_string(&tree.put(key, rev.parse()?, value)?)?
            );
        }
        [_, cmd, file, slots] if cmd == "pager-init" => {
            let n = slots.parse::<u32>()?;
            let db = sedravasql_native_engine::pager::Pager::create(Path::new(file), n)?;
            println!("{}", serde_json::to_string(&db.report())?);
        }
        [_, cmd, file] if cmd == "pager-verify" => {
            let db = sedravasql_native_engine::pager::Pager::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.report())?);
        }
        [_, cmd, file, ns, key] if cmd == "pager-get" => {
            let mut db = sedravasql_native_engine::pager::Pager::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.get(ns, key)?)?);
        }
        [_, cmd, file, changes] if cmd == "pager-commit" => {
            let changes: Vec<sedravasql_native_engine::pager::Change> = table_input(changes)?;
            let mut db = sedravasql_native_engine::pager::Pager::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.commit(&changes)?)?);
        }
        [_, cmd, file] if cmd == "init" => {
            let mut db = NativeLog::create(Path::new(file))?;
            db.verify()?;
            println!(r#"{{"engine":"native-log-v1","created":true}}"#);
        }
        [_, cmd, file] if cmd == "recovery-plan" => {
            let plan = plan_recovery(Path::new(file))?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [_, cmd, source, destination, digest] if cmd == "recovery-copy" => {
            let plan = recover_to_new_file(Path::new(source), Path::new(destination), digest)?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [_, cmd, file, request, statements] if cmd == "sql-sequential" => {
            let statements: Vec<sedravasql_native_engine::sql::BoundStatement> =
                table_input(statements)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::sql::execute_sequential(
                    &mut db,
                    &statements,
                    request
                )?)?
            );
        }
        [_, cmd, file, request, statements] if cmd == "sql-transaction" => {
            let statements: Vec<sedravasql_native_engine::sql::BoundStatement> =
                table_input(statements)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::sql::execute_transaction(
                    &mut db,
                    &statements,
                    request
                )?)?
            );
        }
        [_, cmd, file, sql, params, rest @ ..] if cmd == "sql-bind" => {
            let request = match rest {
                [] => None,
                [s] => Some(s.as_str()),
                _ => return Err("sql-bind: expected optional request key".into()),
            };
            let params: Vec<sedravasql_native_engine::table::Cell> = table_input(params)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::sql::execute_bound(
                    &mut db, sql, &params, request
                )?)?
            );
        }
        [_, cmd, file, sql, rest @ ..] if cmd == "explain" => {
            let params: Vec<sedravasql_native_engine::table::Cell> = match rest {
                [] => vec![],
                [file] => table_input(file)?,
                _ => return Err("explain: expected optional params JSON".into()),
            };
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::sql::explain(
                    &mut db, sql, &params
                )?)?
            );
        }
        [_, cmd, file, sql, rest @ ..] if cmd == "sql" => {
            let request = match rest {
                [] => None,
                [s] => Some(s.as_str()),
                _ => return Err("sql: expected optional request key".into()),
            };
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&sedravasql_native_engine::sql::execute(
                    &mut db, sql, request
                )?)?
            );
        }
        [_, cmd, file, request, input] if cmd == "table-batch" => {
            let changes: Vec<TableMutation> = table_input(input)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).batch(request, &changes)?)?
            );
        }
        [_, cmd, file, request, input] if cmd == "table-create" => {
            let schema: TableSchema = table_input(input)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).create(request, &schema)?)?
            );
        }
        [_, cmd, file, table] if cmd == "table-schema" => {
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).schema(table)?)?
            );
        }
        [_, cmd, file, request, table, rev, input] if cmd == "table-put" => {
            let row: Row = table_input(input)?;
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).put(
                    request,
                    table,
                    rev.parse()?,
                    row
                )?)?
            );
        }
        [_, cmd, file, request, table, key, rev] if cmd == "table-delete" => {
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).delete(
                    request,
                    table,
                    key,
                    rev.parse()?
                )?)?
            );
        }
        [_, cmd, file, table, key] if cmd == "table-get" => {
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).get(table, key)?)?
            );
        }
        [_, cmd, file, table, limit, rest @ ..] if cmd == "table-scan" => {
            let after = match rest {
                [] => None,
                [s] => Some(s.as_str()),
                _ => return Err("table: expected optional after key".into()),
            };
            let mut db = NativeLog::open(Path::new(file))?;
            println!(
                "{}",
                serde_json::to_string(&Tables::new(&mut db).scan(table, after, limit.parse()?)?)?
            );
        }
        [_, cmd, source, dest, digest] if cmd == "archive-create" => {
            println!(
                "{}",
                serde_json::to_string(&create_archive(
                    Path::new(source),
                    Path::new(dest),
                    digest
                )?)?
            );
        }
        [_, cmd, file] if cmd == "archive-verify" => {
            println!(
                "{}",
                serde_json::to_string(&verify_archive(Path::new(file))?)?
            );
        }
        [_, cmd, source, dest, digest] if cmd == "archive-restore" => {
            println!(
                "{}",
                serde_json::to_string(&restore_archive(
                    Path::new(source),
                    Path::new(dest),
                    digest
                )?)?
            );
        }
        [_, cmd, file] if cmd == "backup-plan" => {
            let plan = plan_backup(Path::new(file))?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [_, cmd, source, destination, digest] if cmd == "backup-copy" => {
            let plan = backup_to_new_file(Path::new(source), Path::new(destination), digest)?;
            println!("{}", serde_json::to_string(&plan)?);
        }
        [_, cmd, source, dest, digest] if cmd == "snapshot-create" => {
            println!(
                "{}",
                serde_json::to_string(&create_snapshot(
                    Path::new(source),
                    Path::new(dest),
                    digest
                )?)?
            );
        }
        [_, cmd, file] if cmd == "snapshot-verify" => {
            println!(
                "{}",
                serde_json::to_string(&verify_snapshot(Path::new(file))?)?
            );
        }
        [_, cmd, snapshot, dest, approved] if cmd == "restore-copy" => {
            println!(
                "{}",
                serde_json::to_string(&restore_to_new_file(
                    Path::new(snapshot),
                    Path::new(dest),
                    approved
                )?)?
            );
        }
        [_, cmd, snapshot, index, approved] if cmd == "index-create" => {
            println!(
                "{}",
                serde_json::to_string(&create_index(
                    Path::new(snapshot),
                    Path::new(index),
                    approved
                )?)?
            );
        }
        [_, cmd, file] if cmd == "index-verify" => {
            println!(
                "{}",
                serde_json::to_string(&verify_index(Path::new(file))?)?
            );
        }
        [_, cmd, file, inputs] if cmd == "index-get-many" => {
            let input = Path::new(inputs);
            let meta = fs::symlink_metadata(input)?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 16_384 {
                return Err("index-get-many: invalid request file".into());
            }
            let queries: Vec<[String; 2]> = serde_json::from_slice(&fs::read(input)?)?;
            if queries.is_empty() || queries.len() > 256 {
                return Err("index-get-many: requires 1..256 pairs".into());
            }
            // One verified handle for all seek/read operations, no repeated
            // load of entire file. No mutation to the original index.
            let mut reader = VerifiedIndexReader::open(Path::new(file))?;
            let mut answers = Vec::with_capacity(queries.len());
            for item in &queries {
                answers.push(reader.get(&item[0], &item[1])?);
            }
            println!(
                "{}",
                serde_json::json!({
                   "rows":answers,"row_bytes_read":reader.row_bytes_read(),
                   "queries":queries.len(),"index_bytes":reader.report().index_bytes,
                   "index_sha256":reader.report().index_sha256
                })
            );
        }
        [_, cmd, file, namespace, start, end, limit, rest @ ..] if cmd == "index-page" => {
            let cursor: Option<RangeCursor> = match rest {
                [] => None,
                [raw] if raw.len() <= 2048 => Some(serde_json::from_str(raw)?),
                _ => return Err("index-page: invalid cursor argument".into()),
            };
            let mut reader = VerifiedIndexReader::open(Path::new(file))?;
            let page = reader.range_page(namespace, start, end, limit.parse()?, cursor.as_ref())?;
            println!(
                "{}",
                serde_json::json!({
                    "rows": page.rows, "next": page.next, "row_bytes_read": reader.row_bytes_read(),
                    "row_seeks": reader.row_seeks(), "index_sha256": reader.report().index_sha256,
                    "sequence": reader.report().sequence
                })
            );
        }
        [_, cmd, file, namespace, start, end, limit] if cmd == "index-range" => {
            let mut reader = VerifiedIndexReader::open(Path::new(file))?;
            let rows = reader.range(namespace, start, end, limit.parse()?)?;
            println!(
                "{}",
                serde_json::json!({
                    "rows": rows, "row_bytes_read": reader.row_bytes_read(),
                    "row_seeks": reader.row_seeks(),
                    "index_sha256": reader.report().index_sha256,
                    "sequence": reader.report().sequence
                })
            );
        }
        [_, cmd, file, namespace, key] if cmd == "index-get" => {
            println!(
                "{}",
                serde_json::to_string(&lookup_index(Path::new(file), namespace, key)?)?
            );
        }
        [_, cmd, file] if cmd == "verify" => {
            let mut db = NativeLog::open(Path::new(file))?;
            db.verify()?;
            println!(
                r#"{{"engine":"native-log-v1","verified":true,"sequence":{}}}"#,
                db.sequence()
            );
        }
        [_, cmd, file, input] if cmd == "read-many" => {
            let meta = fs::symlink_metadata(input)?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 8192 {
                return Err("read-many: invalid request file".into());
            }
            let keys: Vec<[String; 2]> = serde_json::from_slice(&fs::read(input)?)?;
            let refs: Vec<(&str, &str)> = keys
                .iter()
                .map(|k| (k[0].as_str(), k[1].as_str()))
                .collect();
            let db = NativeLog::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.read_snapshot(&refs)?)?);
        }
        [_, cmd, file, key] if cmd == "receipt" => {
            let db = NativeLog::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.lookup_receipt(key)?)?);
        }
        [_, cmd, file, ns, key] if cmd == "read" => {
            let db = NativeLog::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.read(ns, key)?)?);
        }
        [_, cmd, file, rest @ ..] if cmd == "usage" => {
            let mut db = NativeLog::open(Path::new(file))?;
            db.set_limits(limits(rest)?)?;
            println!("{}", serde_json::to_string(&db.usage()?)?);
        }
        [_, cmd, file, request, input, rest @ ..] if cmd == "batch" => {
            let json = Path::new(input);
            let meta = fs::metadata(json)?;
            if !meta.is_file() || meta.len() > 1_100_000 {
                return Err("JSON file must be a regular file <= 1100000 bytes".into());
            }
            let operations: Vec<NativeMutation> = serde_json::from_slice(&fs::read(json)?)?;
            let mut db = NativeLog::open(Path::new(file))?;
            db.set_limits(limits(rest)?)?;
            let result = db.commit_batch(request, &operations)?;
            println!("{}", serde_json::to_string(&result)?);
        }
        _ => return Err(usage().into()),
    }
    Ok(())
}
fn main() {
    if let Err(err) = run(&env::args().collect::<Vec<_>>()) {
        eprintln!("NATIVE_PREVIEW_ERROR: {err}");
        std::process::exit(2);
    }
}
