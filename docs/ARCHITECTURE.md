# SedravaSQL — architecture proposal

**Status:** proposal, not implemented. No code or compatibility guarantees exist yet.

## Problem statement

A conventional database can execute a valid SQL statement that is logically wrong for an application. SedravaSQL aims to make application-level safety policies, reviewable change plans, history, and recovery testing first-class features.

This project **does not** claim to understand arbitrary business intent, eliminate all application errors, or prevent loss of data under every failure mode.

## Initial scope

1. Single-node, embedded use first; no distributed consensus in v0.1.
2. Rust service/library layer proposed, with SQLite underneath.
3. Restricted, documented SQL surface; not automatically full SQLite or PostgreSQL compatibility.
4. Managed data mutations through a controlled API; direct writes to the underlying file are outside the proposed audit and policy guarantees.
5. Metadata, policies, and audit entries for a committed operation should be stored atomically with the data mutation wherever supported by the storage engine. Tests must prove the chosen implementation meets that property.
6. No networking, remote access, replication, encryption-at-rest, or multi-tenant isolation guarantee in the first milestone.

## Suggested conceptual components

- **Storage adapter:** connection, transactions, parameter binding, database file management.
- **Policy engine:** rules describing which changes are allowed; explicitly scoped guarantees.
- **Preview engine:** dry-run or projected impact; read snapshot and assumptions documented.
- **Mutation executor:** validates policy again in the actual write transaction. A preview is not a promise: state may change between preview and commit.
- **Audit history:** append-only logical history within the managed API, with rollback semantics and retention policy to be defined.
- **Recovery verifier:** creates backups through supported mechanisms and checks restoration in a disposable environment.
- **CLI:** inspect status, execute documented operations, generate and inspect reports.

## Critical constraints

- A separate history log written after a data commit can become inconsistent if the process fails between operations. Avoid claiming atomic history until both are transactionally coordinated and tested.
- Previews can become stale. Applying a preview must re-check conditions and permissions within an appropriate write transaction.
- WAL files and connection lifetimes matter for backups; never assume that copying a live SQLite main database file alone creates a consistent backup.
- Storage durability depends on actual SQLite configuration, filesystem, operating system, and hardware. Test and document the chosen configuration.
- A database-level audit trail is not inherently tamper-proof against a privileged administrator or direct file modification.
- Rollback of a database transaction does not undo external side effects, such as messages sent to users or payments performed elsewhere.

## Architecture decisions needed before implementation

- What is the smallest supported SQL subset?
- What operation types are eligible for preview?
- What is the isolation/locking model for preview + commit?
- How are policies defined, versioned, and authorized?
- What is included in change history, and what is excluded to protect sensitive data?
- How are backup retention, encryption, redaction, and deletion handled?
- Which SQLite version, journal mode, synchronization settings, and backup API will be supported?

See [roadmap](ROADMAP.md), [verification policy](VERIFICATION.md), and [primary references](REFERENCES.md).
