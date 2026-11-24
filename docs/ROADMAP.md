# Roadmap — SedravaSQL

**Status:** proposed milestones. These are not releases, commitments, or dates. Completion must be demonstrated by tests and documented commands.

## Milestone 0 — public foundation

- [x] Public README with truthful status and non-production warning
- [x] Architecture, roadmap, and evidence policy drafted
- [x] Dedication published
- [x] Choose and publish a project license: Apache-2.0 (same as Sedrava)
- [x] Confirm canonical repository slug and links: `VasiliiBabkin/sedravasql`
- [ ] Agree on minimal user-facing commands and compatibility policy
- [ ] Define initial threat model and data-retention choices

## Milestone 1 — executable prototype (candidate v0.1)

- [x] Versioned Rust project with reproducible build instructions
- [x] Create/open database through a deliberately minimal interface
- [ ] Parameterized CRUD operations and basic documented SQL support
- [x] Explicit transaction handling; tests for commit/rollback semantics
- [ ] First limited policy rules and mutation preview
- [ ] Revalidate policies and target rows at transaction execution
- [x] Audit records committed atomically with managed changes
- [x] Error taxonomy and human-readable CLI diagnostics
- [x] Local test script with real commands and expected output
- [ ] Publish platform support and known limitations

**Acceptance gate:** external user can clone, build, run a complete example, see test results, and reproduce at least one rejected unsafe operation. No production recommendation.

## Milestone 2 — durability and recovery research

- [ ] Crash-injection tests across operation boundaries
- [ ] Disk-full and I/O-failure test plan
- [x] Concurrent transaction and stale-preview tests
- [x] Verified backup-and-restore procedure using supported SQLite mechanisms
- [ ] Integrity checks on restored data; document what they do not detect
- [ ] Versioned schema migration and rollback limitations
- [ ] Publish test matrix: OS, SQLite version, filesystem, configuration, evidence
- [ ] Failure and recovery report with unresolved findings

## Milestone 3 — ecosystem and integration (only after gates pass)

- [ ] Stable API proposal with compatibility policy
- [ ] Sedrava integration design
- [ ] Python/JavaScript integrations evaluated against demand
- [ ] Administrative UI and observability
- [ ] Performance comparison with relevant alternatives, including overhead of safety features

## Not planned for initial releases

Distributed consensus, transparent multi-node replication, complete PostgreSQL compatibility, automatic understanding of arbitrary business intent, and guaranteed recovery without usable backup/history.

## Release rule

A release is not 'stable', 'production-ready', 'safe', 'secure', or 'faster' unless each relevant claim has an evidence record and the scope of the test is clear.

[Verification process](VERIFICATION.md) · [Claim register](CLAIMS.md).

## Independent native storage (experimental)

The long-term target is a standalone SedravaSQL engine that does not depend
on SQLite, as specified in [this ADR](SELF_HOSTED_STORAGE.ru.md).
The separate native-engine/ Rust crate is an initial single-writer,
checksummed append-only multi-key prototype with no rusqlite dependency.
The original SQLite API and Sedrava A5/A6 remain unchanged.
Recovery, snapshots, compaction, on-disk indexing, schema operations,
real-world power-loss tests and a stable API are **not implemented**.

## Batch Transaction Contract A1 (2026-11-23)

The experimental SQLite-free NativeLog and an explicit legacy-isolated SQLite adapter now implement one unstable multi-key TransactionalStore interface. Same test corpus checks atomic 1..16-key batches, CAS, delete/tombstones, idempotent replay and 46 sequential commits on both. The original SQLite Database API and Sedrava Server A6 have not been migrated. See [contract, verification and limitations](BATCH_CONTRACT_A1.ru.md). Production NO-GO; issue #9 still open.

### Native recovery B1 (experimental)

An operator-gated read-only recovery plan and copy-only verified-prefix extraction have been implemented. Complete bad checksums always fail closed; no automatic stale-lock cleanup or source truncation. This is not production recovery or physical power-loss proof. See [protocol](NATIVE_RECOVERY_COPY_B1.ru.md).

### Native B2 offline copy (experimental, 2026-11-23)

The operator-approved backup-plan / backup-copy interface creates and
validates a byte-identical NEW file from a fully verified idle native
journal. B1 recovery remains independent. Five new Rust tests plus
a separate child-process CLI suite PASS. Not live backup, checkpoint,
production restore or physical power-loss assurance.
[Protocol (RU)](NATIVE_BACKUP_B2.ru.md).

### Native snapshot B3 (experimental, 2026-11-23)

SEDRSNP1 materializes sorted entries and idempotency receipts from a
complete idle native log to a new private file using an operator-approved
SHA-256. Independent verification, five Rust tests and child-process CLI
tests PASS. **Not** a writable restore, checkpoint, online backup,
compaction, or disk index. [Protocol](NATIVE_SNAPSHOT_B3.ru.md).

### Native B4: explicit restore from snapshot (experimental)

Controlled restore-copy recreates all original journal frames and
verifies exact log SHA-256/chain hash, state, receipts and CAS before
writing a NEW private SEDRLOG1. Five new Rust tests and separate CLI
8-commit smoke PASS, B1/B2/B3/SQLite regression maintained.
This does not implement live restore, automatic checkpoint/compaction,
indexing or physical power-loss guarantees.
[Operator protocol](NATIVE_RESTORE_B4.ru.md).

### Native B5.1 offline fault-input matrix (2026-11-23)

A child-process CLI matrix checks 5 positive exact restore/replay
roundtrips, 145 flipped-byte, 60 truncated, 15 appended-tail snapshots,
and 20 existing partial output files. All rejection rules PASS.
This is OFFLINE file corruption testing, not proof of real power-loss
recovery, online checkpoint, compaction, or indexing.
[RU protocol](NATIVE_FAULT_MATRIX_B51.ru.md).

### Native B5.2 controlled process exits (2026-11-24)

Adds an after_payload DEBUG-only fault hook alongside existing
after_length and after_sync. Twenty-four child process exits
(8 per hook) PASS. Sixteen torn final frames remain invalid
and require operator-approved B1 copy; eight fully synced
commits preserve idempotency and accept further writes.
No physical power-loss, online recovery, compaction or index.
See NATIVE_CRASH_B52.ru.md.

### B6 managed multi-key SQLite transaction (experimental)

Explicitly opt-in API commits 2–4 managed KV PUTs, per-row audit/receipts
and one full application response in one SQLite transaction. A caller can
include a separately validated Sedrava server state as another managed
row. Tests cover rollback, replay, isolation and original API compatibility.
[Contract](MANAGED_BATCH_B6.ru.md).

### B7 managed multi-key read snapshot (experimental)

Database::read_snapshot accepts 1–16 unique, literal-compatible
namespace/key pairs and loads them within a single deferred SQLite
WAL transaction, so one call cannot mix data across different
commit generations. Tests include 180 concurrent B6 two-key commits
and 900 repeated reads, invalid inputs, missing/tombstone handling,
and unchanged audit. No schema change, general query capability,
cross-request snapshot or power-loss guarantee.
[RU protocol](MANAGED_SNAPSHOT_B7.ru.md).

### Native B8: offline verified lookup sidecar (2026-11-24)

SEDRIDX1 is an immutable file generated only from a SHA-approved B3
snapshot into a new 0600 file under a 0700 parent. Includes sorted
namespace/key directory, source hashes and full row data, but excludes
receipts. Strict verification of checksum/ordering/offsets/digests before
binary-search lookup; **full-file validation means no faster-asymptotic
live lookup claim**. Four new Rust tests and independent CLI corruption/
truncation/determinism tests PASS. Writable journal, B1–B5 recovery and
original SQLite DB unchanged. Still NO GO for production.
[RU details](NATIVE_INDEX_B8.ru.md).

### Native B9: direct disk seek after one verified open

VerifiedIndexReader reads immutable B8 SEDRIDX1. Full file verified
once at open; further lookups binary-search sorted in-memory keys
and seek/read only target records, checking row SHA-256 and metadata.
CLI index-get-many handles 1..256 queries in one verified session.
VDS experiment: 54 queries (4 hits), 79,924-byte index, 4,358 row
data bytes after open. Four new Rust tests and real CLI PASS.
Not a live or production journaling index. See NATIVE_INDEX_B9.ru.md.

### Native B10: bounded offline index ranges

Half-open lexical ranges in a single namespace, 1..256 rows, with tombstones and checked direct disk reads. B8 format unchanged; 37 native/21 SQLite Rust tests, strict Clippy and B9/B10 CLI PASS on VDS. Still an offline index, not live storage. [Protocol](NATIVE_INDEX_B10.ru.md).

### Native B11: sequential verified range reads

Two directory bounds searches and one explicit row-data seek for nonempty ranges; per-row integrity and metadata checks preserved. O(log N + K) directory operations. [RU protocol](NATIVE_INDEX_B11.ru.md). Still offline, not a mutable index.

### Native B12: digest-bound range pagination

Explicit continuation bound to immutable index SHA-256, namespace and exact bounds. Includes tombstones and rejects stale generation; not an authenticated cursor. VDS: 39 native, 21 SQLite Rust tests; B9–B12 CLI PASS. [Protocol](NATIVE_INDEX_B12.ru.md).

### Native B13: committed-generation snapshot reads and receipt lookup

Coherent 1..16 key reads and historical receipts on one exclusive native journal handle. [RU protocol](NATIVE_SNAPSHOT_B13.ru.md). VDS: 40 native / 21 SQLite Rust tests and B9–B13 CLI PASS. No multi-process MVCC.

### Native B14: staged replay and commit

No full accumulated-state clone per frame; prepare validated delta, publish only after sync or verified replay. Existing format and durable receipts preserved. [Protocol](NATIVE_REPLAY_B14.ru.md). Compaction and resource budgets remain pending.

### Native B15a: bounded write admission

[Byte and transaction ceilings](NATIVE_BUDGET_B15A.ru.md), exact frame accounting, historical replay at capacity. B15b checkpoint/compaction pending.

### B15b: compressed offline journal archive

[SEDRARC1 exact restoration](NATIVE_ARCHIVE_B15B.ru.md), all receipts preserved; active compaction remains pending.

### B16a: typed table API

[Immutable schema and primary key CRUD / atomic multi-table batches](NATIVE_TABLE_B16A.ru.md). SQL grammar/executor and online compaction remain pending.

### B16b/B16c — Native SQL subset and bindings

[SQL grammar, limits, atomic receipts and typed positional bindings](NATIVE_SQL_B16B.ru.md). JOIN, aggregates, secondary indexes and live compaction pending.

### B17 — aggregates, grouping and access plan

[Native analytical SELECT and EXPLAIN](NATIVE_SQL_B17.ru.md). JOIN and secondary indexes pending.

### B18 — two-table native JOIN

[INNER/LEFT JOIN, bounded query-local index](NATIVE_SQL_B18.ru.md). Persistent secondary indexes pending.

### B19 — index definitions, derived secondary cache, DISTINCT/HAVING

[SQL and cache semantics](NATIVE_SQL_B19.ru.md). Durable index pages and cost optimizer remain pending.

### B20 — foreign keys and disjoint-row SQL transactions

[Constraints and atomic transaction subset](NATIVE_SQL_B20.ru.md). General transaction sessions and disk index pages pending.

### B21/B22 — immutable checked page image and disk search tree

[SEDRPG01 format and limits](NATIVE_PAGES_B21_B22.ru.md). Writable paged storage and live SQL index integration pending.
