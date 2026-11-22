# Claims register

**As of 2026-11-22:** no runnable SedravaSQL implementation and no independent reliability/performance benchmark are documented in this repository.

| ID | Statement | Type | Evidence/status |
| --- | --- | --- | --- |
| S-001 | SedravaSQL is at planning/documentation stage. | Current status | Repository documentation only; no executable published |
| S-002 | Initial Rust + SQLite design is intended. | Proposal | [Architecture](ARCHITECTURE.md); **not implemented** |
| S-003 | Guarded mutation preview and change history are project goals. | Proposal | [Roadmap](ROADMAP.md); **not implemented** |
| S-004 | A stable interface and production guarantees are not currently offered. | Current limitation | [README](../README.md); no release evidence |
| B-001 | SQLite documents transactional ACID behavior subject to its operating conditions. | External fact | [SQLite transactions](https://www.sqlite.org/transactional.html); **not a claim about SedravaSQL** |
| B-002 | SQLite documentation describes crash, I/O and corruption testing. | External fact | [SQLite test process](https://www.sqlite.org/testing.html); **not evidence that SedravaSQL passed tests** |

## When code exists

Each new implemented claim must link to a commit, executable test, reproduced output and bounded configuration in [VERIFICATION.md](VERIFICATION.md). Add date and evidence reference. Never mark a planned feature as implemented on the basis of a document alone.

There are **no proven speed, safety, security, or durability improvements** of SedravaSQL over other databases at this time.
