# SedravaSQL — 0.1.0-alpha.1

**Experimental embedded Rust + SQLite database with reviewable mutations.**

[Русский](README.ru.md) · [Implementation contract](docs/IMPLEMENTED_V0_1.ru.md)

> **Runnable alpha, NOT production-ready.** Tested on Ubuntu x86_64.
> This is a constrained typed key/value API, **not** a general SQL server,
> full SQL dialect or PostgreSQL replacement. Do not store valuable data.

Build (Rust 1.85+, Cargo, C compiler for bundled SQLite):

```sh
git clone https://github.com/VasiliiBabkin/sedravasql.git
cd sedravasql
cargo build --locked
cargo test --locked --all-targets
python3 scripts/verify_local.py
```

Example using a separate private local directory:

```sh
mkdir -m 700 -p /tmp/sedravasql-demo
DB=/tmp/sedravasql-demo/data.sqlite
./target/debug/sedravasql init "$DB"
./target/debug/sedravasql plan-put "$DB" demo hello "Hello"
./target/debug/sedravasql put "$DB" demo hello "Hello" \
  --expect 0 --request-key example-request-0001
./target/debug/sedravasql get "$DB" demo hello
./target/debug/sedravasql verify "$DB"
./target/debug/sedravasql backup "$DB" /tmp/sedravasql-demo/backup.sqlite
```

Old revision rejects new writes. On Unix the database file must be owner-private (0600) and have
exactly one hardlink; existing public-mode/hardlinked files are refused.
Create a private parent directory. These checks are not complete
protection from malicious same-UID directory races.

An identical repeated request returns
`"replayed":true` without another audit record. `delete` requires an
explicit preview and `--confirm`. All mutations use parameterized SQL,
WAL, synchronous=FULL and one atomic transaction for value, audit and
idempotency receipt.

**Independent VDS evidence:** 4 Rust tests, native CLI integration,
12 concurrent processes (one accepted, 11 stale-revision conflicts),
4 injected crash stages around SQLite COMMIT, and an online backup
containing committed WAL data from a separate live writer. These tests
**do not prove physical power-loss durability or universal reliability**.

Features not implemented: arbitrary SQL, general relational schema,
SQL server, replication, user roles, encryption, automatic migration,
or production-grade crash recovery matrix.

[Architecture](docs/ARCHITECTURE.md) · [Roadmap](docs/ROADMAP.md) ·
[Claims](docs/CLAIMS.md) · [Verification](docs/VERIFICATION.md).

License: [Apache-2.0](LICENSE).

Dedicated to the memory of **Дмитрий Грабыльников**.
[Dedication](DEDICATION.md).

## Experimental SQLite-free engine

A separate [native-engine crate](native-engine/) implements a small local
single-writer append-only log with bounded atomic batches, CAS revisions,
durable request keys and a standalone preview CLI. This is **not** a
release, general SQL implementation, or safe replacement for the existing
SQLite-backed Database API. Do **not** use it for valuable data.
[Architecture, limitations and verification commands (RU)](docs/SELF_HOSTED_STORAGE.ru.md).

The initial experimental [shared batch transaction contract](docs/BATCH_CONTRACT_A1.ru.md) now runs on the SQLite-free NativeLog and an explicit isolated SQLite adapter, with one cross-backend test suite. The established SQLite API and Sedrava A6 are unchanged.

Experimental B1: [read-only recovery plan and verified-prefix copy](docs/NATIVE_RECOVERY_COPY_B1.ru.md) for the separate native engine. Never mutates the source or removes stale locks. No production recovery claim.

Experimental B2: [operator-approved offline copy of a healthy native log](docs/NATIVE_BACKUP_B2.ru.md). It does not provide a production backup, checkpoint or online snapshot.

Experimental B3: [read-only deterministic native snapshot](docs/NATIVE_SNAPSHOT_B3.ru.md)
of a stopped, fully verified log, preserving entries and receipts.
No restore, checkpoint, fast startup, compaction or production use.

Experimental B4: operator-approved [restore from a verified snapshot](docs/NATIVE_RESTORE_B4.ru.md) to a NEW byte-identical native log. Preserves receipts and CAS history; no in-place replacement, online restore, compaction or production guarantee.

**Project website:** https://sql.virtreal.ru/ · [Russian getting-started guide](https://sql.virtreal.ru/docs/). Static site sources and deployment verification are in [site/](site/README.md); no live database endpoint is published.

B5.1 research: [offline fault-input matrix](docs/NATIVE_FAULT_MATRIX_B51.ru.md)
for B3/B4 exact restore and corrupt, truncated or extended snapshots.
No actual process crash/power-loss verification.

Experimental B5.2: 24 controlled NativeLog process exits
at three debug-only write boundaries; torn frames rejected and
synced receipts replayed. Not hardware power-loss validation.
See docs/NATIVE_CRASH_B52.ru.md.

B6 experimental [opt-in managed multi-key SQLite transaction](docs/MANAGED_BATCH_B6.ru.md):
2–4 CAS writes, legacy per-row audit/receipts and one durable group response.
Existing callers require no migration, and there is no production claim.

Experimental B7: [consistent multi-key managed SQLite read snapshot](docs/MANAGED_SNAPSHOT_B7.ru.md).
Read 1–16 distinct namespace/key records within one read-only WAL
transaction, compatible with B6 atomic writes. No schema migration,
general SQL query feature, NativeLog switching or production claim.

NativeLog B8 research: [verified offline immutable point-lookup sidecar](docs/NATIVE_INDEX_B8.ru.md)
derived from an approved B3 snapshot. Sorted directory, deterministic
contents, SHA-256 and strict full-file verification. NOT a fast live
on-disk index, checkpoint, receipt-preserving restore or production DB.

NativeLog B9: verified per-session direct disk reads from immutable
B8 SEDRIDX1 index. Full validation at open, then binary-search
directory + seek/read/row SHA per key. Sample: 54 lookups, only
4358 row-data bytes after open of 79924-byte index.
Not a live writable journal index; docs/NATIVE_INDEX_B9.ru.md.

**Native B10 (experimental):** bounded offline index range reads; [protocol and verification](docs/NATIVE_INDEX_B10.ru.md).

**Native B11 (experimental):** sequential verified range reads with one data seek. [Protocol](docs/NATIVE_INDEX_B11.ru.md).

**Native B12 (experimental):** [digest-bound offline range pagination](docs/NATIVE_INDEX_B12.ru.md).

**Native B13 (experimental):** [coherent journal reads and durable receipt lookup](docs/NATIVE_SNAPSHOT_B13.ru.md).

**Native B14 (experimental):** [staged replay without full state cloning](docs/NATIVE_REPLAY_B14.ru.md).

**Native B15a (experimental):** [journal write budgets](docs/NATIVE_BUDGET_B15A.ru.md).

Experimental [B15b archives](docs/NATIVE_ARCHIVE_B15B.ru.md) and [B16a typed tables](docs/NATIVE_TABLE_B16A.ru.md).

Experimental [native SQL B16b/B16c](docs/NATIVE_SQL_B16B.ru.md).

Experimental [B17 analytics and access plans](docs/NATIVE_SQL_B17.ru.md).

Experimental [B18 JOIN](docs/NATIVE_SQL_B18.ru.md).

Experimental [B19 indexes and analytical filters](docs/NATIVE_SQL_B19.ru.md).
