# Claims register

**As of 2026-11-22:** experimental Rust source and unit/smoke tests are present on branch feat/prototype-v0.1. No successful build or test execution has yet been recorded; no release, reliability benchmark or verified production feature exists.

| ID | Statement | Type | Evidence/status |
| --- | --- | --- | --- |
| S-001 | A Rust/SQLite source prototype has been written. | Source status | src/lib.rs, src/main.rs, Cargo.toml on feat/prototype-v0.1; **build unverified** |
| S-002 | Rust + SQLite dependency is declared. | Source status | [Cargo.toml](../Cargo.toml); **compile not verified** |
| S-003 | Managed writes, token preview/apply and audit operations have source implementations. | Experimental source, NOT verified | [Library source](../src/lib.rs), [limitations](PROTOTYPE.md); unit tests exist, not run |
| S-004 | A stable interface and production guarantees are not currently offered. | Current limitation | [README](../README.md); no release and no validated test run |
| B-001 | SQLite documents transactional ACID behavior subject to its operating conditions. | External fact | [SQLite transactions](https://www.sqlite.org/transactional.html); **not a claim about SedravaSQL** |
| B-002 | SQLite documentation describes crash, I/O and corruption testing. | External fact | [SQLite test process](https://www.sqlite.org/testing.html); **not evidence that SedravaSQL passed tests** |

## When code exists

Each new implemented claim must link to a commit, executable test, reproduced output and bounded configuration in [VERIFICATION.md](VERIFICATION.md). Add date and evidence reference. Never mark a planned feature as implemented on the basis of a document alone.

There are **no proven speed, safety, security, or durability improvements** of SedravaSQL over other databases at this time.
