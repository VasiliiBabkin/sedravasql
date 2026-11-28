//! SedravaSQL alpha CLI. Every mutating operation is parameterized, previewed
//! (CAS revision required), idempotent and auditable inside a single SQLite TX.
use sedravasql::{Change, Database, Error, Result};
use serde::Serialize;
use std::path::Path;
fn help() -> &'static str {
    "SedravaSQL 0.1 alpha (local, experimental)\n\
     sedravasql init FILE\n\
     sedravasql version FILE\n\
     sedravasql get FILE NAMESPACE KEY\n\
     sedravasql list FILE NAMESPACE\n\
     sedravasql plan-put FILE NAMESPACE KEY VALUE\n\
     sedravasql put FILE NAMESPACE KEY VALUE --expect REVISION --request-key KEY\n\
     sedravasql plan-delete FILE NAMESPACE KEY\n\
     sedravasql delete FILE NAMESPACE KEY --expect REVISION --request-key KEY --confirm\n\
     sedravasql audit FILE [LIMIT]\n\
     sedravasql verify FILE\n\
     sedravasql backup FILE NEW_BACKUP_PATH\n\
     Options must be explicitly provided. No arbitrary SQL writes or network access."
}
fn print<T: Serialize>(value: T) -> Result<()> {
    let encoded = serde_json::to_string_pretty(&value)
        .map_err(|_| Error::Invalid("result serialization failed"))?;
    println!("{encoded}");
    Ok(())
}
fn parse_flags(flags: &[String], delete: bool) -> Result<(u64, String)> {
    let mut expect = None;
    let mut request = None;
    let mut confirm = false;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--expect" if expect.is_none() => {
                index += 1;
                let value = flags
                    .get(index)
                    .ok_or(Error::Invalid("missing --expect argument"))?;
                expect = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| Error::Invalid("revision must be unsigned integer"))?,
                );
            }
            "--request-key" if request.is_none() => {
                index += 1;
                request = Some(
                    flags
                        .get(index)
                        .ok_or(Error::Invalid("missing --request-key"))?
                        .clone(),
                );
            }
            "--confirm" if delete && !confirm => confirm = true,
            _ => {
                return Err(Error::Invalid(
                    "unknown, duplicated or unsupported operation flag",
                ))
            }
        }
        index += 1;
    }
    if delete && !confirm {
        return Err(Error::Invalid(
            "delete requires --confirm after reviewing plan-delete",
        ));
    }
    Ok((
        expect.ok_or(Error::Invalid("missing --expect"))?,
        request.ok_or(Error::Invalid("missing --request-key"))?,
    ))
}
fn cli(args: &[String]) -> Result<()> {
    match args {
        [command, path] if command == "init" => {
            let db = Database::init(Path::new(path))?;
            print(serde_json::json!({"ok":true,"schema_version":1,
                "sqlite_version":db.sqlite_version()?,
                "mode":"sqlite-wal","synchronous":"FULL"}))
        }
        [command, path] if command == "version" => {
            let db = Database::open(Path::new(path))?;
            print(serde_json::json!({"schema_version":1,
                "sqlite_version":db.sqlite_version()?,
                "experimental":true}))
        }
        [command, path, n, k] if command == "get" => {
            let db = Database::open(Path::new(path))?;
            print(db.read(n, k)?)
        }
        [command, path, n] if command == "list" => {
            let db = Database::open(Path::new(path))?;
            print(db.list(n)?)
        }
        [command, path, n, k, v] if command == "plan-put" => {
            let db = Database::open(Path::new(path))?;
            print(db.preview(n, k, Change::Put(v))?)
        }
        [command, path, n, k] if command == "plan-delete" => {
            let db = Database::open(Path::new(path))?;
            print(db.preview(n, k, Change::Delete { confirm: false })?)
        }
        [command, path, n, k, v, rest @ ..] if command == "put" => {
            let (rev, request) = parse_flags(rest, false)?;
            let mut db = Database::open(Path::new(path))?;
            print(db.mutate(n, k, Change::Put(v), rev, &request)?)
        }
        [command, path, n, k, rest @ ..] if command == "delete" => {
            let (rev, request) = parse_flags(rest, true)?;
            let mut db = Database::open(Path::new(path))?;
            print(db.mutate(n, k, Change::Delete { confirm: true }, rev, &request)?)
        }
        [command, path] if command == "audit" => {
            let db = Database::open(Path::new(path))?;
            print(db.audit(100)?)
        }
        [command, path, limit] if command == "audit" => {
            let limit = limit
                .parse::<u32>()
                .map_err(|_| Error::Invalid("invalid audit limit"))?;
            let db = Database::open(Path::new(path))?;
            print(db.audit(limit)?)
        }
        [command, path] if command == "verify" => {
            let db = Database::open(Path::new(path))?;
            db.verify()?;
            print(serde_json::json!({"integrity":"ok","schema_version":1}))
        }
        [command, path, destination] if command == "backup" => {
            let db = Database::open(Path::new(path))?;
            db.backup(Path::new(destination))?;
            print(serde_json::json!({"backup":"created","destination":destination}))
        }
        _ => Err(Error::Invalid("unknown command/arguments; run --help")),
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!(
            "sedravasql {} (managed SQLite alpha)",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{}", help());
        return;
    }
    if args.is_empty() {
        eprintln!("{}", help());
        std::process::exit(2);
    }
    if let Err(e) = cli(&args) {
        eprintln!("sedravasql: {e}");
        std::process::exit(2);
    }
}
