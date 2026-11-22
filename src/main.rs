use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "sedravasql", version, about = "Experimental safety-first database CLI (NOT production-ready)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize a new SedravaSQL database, without modifying unrelated tables.
    Init {
        #[arg(long)]
        db: PathBuf,
    },
    /// Create or replace one key, with a transactional audit entry.
    Put {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        key: String,
        #[arg(long)]
        value: String,
    },
    /// Fetch a value by key.
    Get {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        key: String,
    },
    /// List up to 1000 records; optional literal key prefix.
    List {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Show the record count and short-lived fingerprint for a deletion.
    PreviewDelete {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        prefix: String,
    },
    /// Apply a previously previewed deletion, after revalidation.
    ApplyDelete {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        prefix: String,
        #[arg(long)]
        token: String,
    },
    /// Show the last 100 audit records (values are not stored).
    Audit {
        #[arg(long)]
        db: PathBuf,
    },
    /// Execute one read-only SELECT; repeat --param for bound TEXT arguments.
    Query {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        sql: String,
        #[arg(long = "param")]
        params: Vec<String>,
    },
}

fn show(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { db } => {
            let created = sedravasql::initialize(&db)?;
            show(&json!({"ok": true, "created": created}))?;
        }
        Command::Put { db, key, value } => {
            sedravasql::put(&db, &key, &value)?;
            show(&json!({"ok": true, "key": key}))?;
        }
        Command::Get { db, key } => {
            let value = sedravasql::get(&db, &key)?;
            show(&json!({"key": key, "value": value}))?;
        }
        Command::List { db, prefix } => show(&sedravasql::list(&db, &prefix)?)?,
        Command::PreviewDelete { db, prefix } => {
            show(&sedravasql::preview_delete(&db, &prefix)?)?;
        }
        Command::ApplyDelete { db, prefix, token } => {
            let deleted = sedravasql::apply_delete(&db, &prefix, &token)?;
            show(&json!({"ok": true, "deleted": deleted}))?;
        }
        Command::Audit { db } => show(&sedravasql::audit(&db)?)?,
        Command::Query { db, sql, params } => {
            show(&sedravasql::query(&db, &sql, &params)?)?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
