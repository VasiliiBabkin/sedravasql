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
