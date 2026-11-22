# Roadmap — SedravaSQL

**Status:** proposed milestones. These are not releases, commitments, or dates. Completion must be demonstrated by tests and documented commands.

## Milestone 0 — public foundation

- [x] Public README with truthful status and non-production warning
- [x] Architecture, roadmap, and evidence policy drafted
- [x] Dedication published
- [ ] Choose and publish a project license
- [ ] Decide the final repository slug and stable project links
- [ ] Agree on minimal user-facing commands and compatibility policy
- [ ] Define initial threat model and data-retention choices

## Milestone 1 — executable prototype (candidate v0.1)

- [ ] Versioned Rust project with reproducible build instructions
- [ ] Create/open database through a deliberately minimal interface
- [ ] Parameterized CRUD operations and basic documented SQL support
- [ ] Explicit transaction handling; tests for commit/rollback semantics
- [ ] First limited policy rules and mutation preview
- [ ] Revalidate policies and target rows at transaction execution
- [ ] Audit records committed atomically with managed changes
- [ ] Error taxonomy and human-readable CLI diagnostics
- [ ] Local test script with real commands and expected output
- [ ] Publish platform support and known limitations

**Acceptance gate:** external user can clone, build, run a complete example, see test results, and reproduce at least one rejected unsafe operation. No production recommendation.

## Milestone 2 — durability and recovery research

- [ ] Crash-injection tests across operation boundaries
- [ ] Disk-full and I/O-failure test plan
- [ ] Concurrent transaction and stale-preview tests
- [ ] Verified backup-and-restore procedure using supported SQLite mechanisms
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
