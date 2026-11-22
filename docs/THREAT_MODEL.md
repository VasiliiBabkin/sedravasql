# SedravaSQL v0.1 threat model (draft)

## Trust assumptions

The experimental CLI and library are designed for **trusted local code**
operating on a database file controlled by the same operating-system user.

A caller who can modify the SQLite file, execute code as that user, or replace
the binary is **outside** the protected boundary. The current implementation
is NOT a secure multi-tenant service and offers no network authentication,
authorization, encryption, privilege isolation, or tamper-proof audit.

## Data at risk

- The SQLite file contains plaintext keys and values.
- The audit table includes plaintext keys, hashes of previous/new values and
  SQLite-generated timestamps. Keys and low-entropy hashes may leak data.
- Temporary files, caches, filesystem snapshots and external backup tools are
  not managed by SedravaSQL.

## Properties being explored

1. All writes through the managed API insert corresponding audit rows in the
   same SQLite transaction, otherwise roll back.
2. Prefix deletion requires a preview token and rechecks it inside an IMMEDIATE
   transaction before deleting anything.
3. Preview tokens are scoped to a database instance identifier generated at initialization; copied databases share that identifier, and the token is not an authorization secret.
4. Prototype policy rejects deletion of over ten matching records, even if the
   user possesses a valid token.
5. The public read-only SELECT command cannot directly issue mutation SQL.

## Important exclusions

- Malicious or buggy privileged users directly editing the database.
- Availability attacks, disk exhaustion, rogue SQLite extensions, data corruption,
  malicious SQL performance attacks or hardware power-loss guarantees.
- Full recovery to an earlier state; no backup or undo implementation exists.
- Cryptographic integrity of audit records or preview token authenticity.
- Concurrency beyond SQLite's documented write transaction semantics.
- Guarantees against stale filesystem backup copies or OS/hardware faults.

These are intended properties, **not verified security claims**. Published
claims must cite tests and execution environments per docs/VERIFICATION.md.

## Required follow-up

- Establish data-retention/redaction requirements and scrubbed test fixtures.
- Add failure injection for audit/transaction and separate-process concurrency.
- Generate Cargo.lock; run and archive exact toolchain and dependency versions.
- Define backup and restore design with correctness checks before important data.
