//! Explicitly experimental CLI for the SQLite-free SedravaSQL append-log.
//! NOT the existing stable or alpha SQLite CLI. Do not use for valuable data.
use sedravasql_native_engine::backup::{backup_to_new_file, plan_backup};
use sedravasql_native_engine::index::{
    create_index, lookup_index, verify_index, VerifiedIndexReader,
};
use sedravasql_native_engine::recovery::{plan_recovery, recover_to_new_file};
use sedravasql_native_engine::restore::restore_to_new_file;
use sedravasql_native_engine::snapshot::{create_snapshot, verify_snapshot};
use sedravasql_native_engine::{NativeLog, NativeMutation, TransactionalStore};
use std::{env, error::Error, fs, path::Path};

fn usage() -> &'static str {
    "USAGE: sedravasql-native-preview <init FILE | verify FILE | read FILE NAMESPACE KEY | batch FILE REQUEST_KEY JSON_FILE | recovery-plan FILE | recovery-copy SOURCE DEST EXPECTED_SHA256 | backup-plan SOURCE | backup-copy SOURCE DEST EXPECTED_SHA256 | snapshot-create SOURCE DEST EXPECTED_SHA256 | snapshot-verify SNAPSHOT | restore-copy SNAPSHOT NEW_DEST APPROVED_SNAPSHOT_SHA256 | index-create SNAPSHOT NEW_INDEX APPROVED_SNAPSHOT_SHA256 | index-verify INDEX | index-get INDEX NAMESPACE KEY | index-get-many INDEX REQUESTS_JSON | index-range INDEX NAMESPACE START END LIMIT>\n\
     EXPERIMENTAL / SINGLE WRITER / NO AUTOMATIC RECOVERY / NOT FOR VALUABLE DATA"
}
fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args {
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
        [_, cmd, file, ns, key] if cmd == "read" => {
            let db = NativeLog::open(Path::new(file))?;
            println!("{}", serde_json::to_string(&db.read(ns, key)?)?);
        }
        [_, cmd, file, request, input] if cmd == "batch" => {
            let json = Path::new(input);
            let meta = fs::metadata(json)?;
            if !meta.is_file() || meta.len() > 1_100_000 {
                return Err("JSON file must be a regular file <= 1100000 bytes".into());
            }
            let operations: Vec<NativeMutation> = serde_json::from_slice(&fs::read(json)?)?;
            let mut db = NativeLog::open(Path::new(file))?;
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
