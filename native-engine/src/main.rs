//! Explicitly experimental CLI for the SQLite-free SedravaSQL append-log.
//! NOT the existing stable or alpha SQLite CLI. Do not use for valuable data.
use sedravasql_native_engine::backup::{backup_to_new_file, plan_backup};
use sedravasql_native_engine::recovery::{plan_recovery, recover_to_new_file};
use sedravasql_native_engine::snapshot::{create_snapshot, verify_snapshot};
use sedravasql_native_engine::{NativeLog, NativeMutation, TransactionalStore};
use std::{env, error::Error, fs, path::Path};

fn usage() -> &'static str {
    "USAGE: sedravasql-native-preview <init FILE | verify FILE | read FILE NAMESPACE KEY | batch FILE REQUEST_KEY JSON_FILE | recovery-plan FILE | recovery-copy SOURCE DEST EXPECTED_SHA256 | backup-plan SOURCE | backup-copy SOURCE DEST EXPECTED_SHA256 | snapshot-create SOURCE DEST EXPECTED_SHA256 | snapshot-verify SNAPSHOT>\n\
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
