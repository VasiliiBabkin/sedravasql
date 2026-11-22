# SedravaSQL 0.1 — experimental prototype

**Status: source code available on feature branch; not released or production tested.**
This is an embedded, single-file SQLite-backed **key/value** prototype, not a
general-purpose SQL server or a replacement for PostgreSQL.

## Requirements

- Rust toolchain 1.89 or newer (Cargo included)
- C compiler available to build bundled SQLite
- Git
- Python 3 for the optional shell smoke test

The Rust crate uses rusqlite 0.43 with the bundled SQLite feature. The dependency
versions in Cargo.toml are version ranges, not a locked dependency graph:
**Cargo.lock has not yet been generated and checked in.** Until that happens,
builds are not fully reproducible. Record the actual dependencies and toolchain.

## Clone and build

    git clone https://github.com/VasiliiBabkin/sedravasql.git
    cd sedravasql
    cargo build
    cargo test

To try this unmerged prototype, check out the feature branch first:

    git fetch origin feat/prototype-v0.1
    git switch feat/prototype-v0.1

The command-line binary is target/debug/sedravasql (or target\debug\sedravasql.exe
on Windows). Use only disposable demonstration data.

## Quick start — POSIX shell

    ./target/debug/sedravasql init --db demo.db
    ./target/debug/sedravasql put --db demo.db --key task:1 --value pending
    ./target/debug/sedravasql put --db demo.db --key task:2 --value done
    ./target/debug/sedravasql get --db demo.db --key task:1
    ./target/debug/sedravasql list --db demo.db --prefix task:
    ./target/debug/sedravasql query --db demo.db --sql "SELECT key, value FROM sedravasql_kv WHERE key = ?1" --param task:1
    ./target/debug/sedravasql audit --db demo.db

The CLI prints JSON to stdout and errors to stderr with nonzero status.

To delete with a preview:

    ./target/debug/sedravasql preview-delete --db demo.db --prefix task:

Copy the displayed 64-character token and pass it unchanged:

    ./target/debug/sedravasql apply-delete --db demo.db --prefix task: --token YOUR_TOKEN

Before applying, SedravaSQL recomputes the token and row set inside an
IMMEDIATE write transaction. It will refuse stale previews, empty matches,
and deletions of more than 10 records. This count is a prototype hard-coded
policy, **not** a configurable authorization system. Preview does not reserve
rows. No undo/restore command exists.

## Supported surface

| Operation | API / CLI | Notes |
| --- | --- | --- |
| Initialize | init | Refuses SQLite DB with unrelated tables |
| Upsert | put | Exactly one TEXT key/value, audited atomically |
| Read | get, list | Literal key/prefix, at most 1000 results |
| Read-only SQL | query | Single SELECT without semicolon; bound TEXT parameters only |
| Audit | audit | Last 100 managed operations; key and hashes, no old values |
| Preview deletion | preview-delete | Non-empty literal key prefix, at most 10000 candidates |
| Apply deletion | apply-delete | Matching token, at most 10 rows, rechecked in write txn |

Key limit: 256 UTF-8 bytes; values: 1 MiB UTF-8 bytes.
The only application table is sedravasql_kv. No custom tables, arbitrary
CREATE/INSERT/UPDATE, joins across user-managed tables, server, network
protocol, backup/restore, schema migration or encryption is offered.

Read-only SQL is intended for trusted local callers, **not** for processing
untrusted SQL as a security sandbox. SELECT may expose all local database
information available to the process.

## Data safety and limitations

- The database is an ordinary, unencrypted SQLite file. Protect it with OS
  permissions and backups managed separately.
- The CLI uses SQLite transactions with a 3-second busy timeout; configured
  durability and actual filesystem/power-loss guarantees have **not** been
  tested on supported platforms.
- Each managed put/delete and its audit insertion are in one SQLite transaction;
  tests cover rollback on injected audit failures, but test execution and
  crash recovery still require independent verification.
- Audit stores TEXT keys and SHA-256 hashes. Keys can contain sensitive data,
  and hashes may reveal information about low-entropy values. They are **not**
  confidentiality protection or a tamper-proof log.
- A privileged process using SQLite directly can alter rows and audit tables.
  The CLI cannot enforce invariants on such external writes.
- A preview token incorporates a per-database random instance identifier and is a **state fingerprint**, not a password, signature,
  authorization, durable snapshot, or historical backup.
- Rechecking uses a database revision for managed writes and a fingerprint
  of matching keys and values. External writes are not necessarily reflected
  in the revision and may evade detection if they restore identical content.
- No backup tool or validated recovery process is implemented.
- This release makes no claims about safe handling of production data,
  security assurance, crash consistency under power failure, or benchmark speed.

## How to check it

    cargo fmt --all -- --check
    cargo test --all-targets
    bash scripts/smoke.sh

The script uses a temporary directory and disposable database; it verifies a
successful preview/apply sequence and that a stale preview is refused. The
unit tests cover more cases; see src/lib.rs.

**Test status at initial submission:** tests are included but have **not yet
been run in an environment with Rust installed**. Do not interpret test code
as a passing result. See docs/VERIFICATION.md and docs/CLAIMS.md.

## Dependencies

- SQLite: https://www.sqlite.org/
- rusqlite: https://docs.rs/rusqlite/0.43.0/rusqlite/
- Rust: https://doc.rust-lang.org/book/
- License: ../LICENSE (Apache-2.0)

The dedication to Dmitry Grabylnikov remains in DEDICATION.md.
