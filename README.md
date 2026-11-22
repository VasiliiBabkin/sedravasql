# SedravaSQL

**Safety-first SQL database project — early design stage.**

[Русская версия](README.ru.md)

> **Status: planning / documentation only.** There is no database engine, runnable CLI, stable API, release, installation procedure, or production-readiness claim at this time. Do not store important data in SedravaSQL.

## Purpose

SedravaSQL is a proposed database system focused on verifiable data changes, explicit safety policies, understandable history, and tested recovery procedures.

The intended first implementation is a Rust-based layer using SQLite as its transactional storage engine. This is an **architecture proposal**, not a shipped implementation. SQL compatibility, failure guarantees, performance, and platform support will only be described as available once implemented and tested.

SedravaSQL is designed as a separate project from the [Sedrava programming language](https://github.com/VasiliiBabkin/sedrava). Language bindings may follow; the database is not intended to require Sedrava.

## Current availability

| Capability | Status |
| --- | --- |
| Design documents and project principles | In progress |
| Rust core and executable | Not implemented |
| SQL interface | Not implemented |
| Preview and safety checks for bulk changes | Proposed |
| Auditable change history | Proposed |
| Backup verification and recovery | Proposed |
| Stable API and releases | Not available |
| Production use | **Not supported** |

No benchmarks or reliability results have been published.

## Start here

- [Русское описание](README.ru.md)
- [Scope and architecture](docs/ARCHITECTURE.md)
- [Roadmap and acceptance criteria](docs/ROADMAP.md)
- [Evidence and verification policy](docs/VERIFICATION.md)
- [Claims register](docs/CLAIMS.md)
- [Technical references](docs/REFERENCES.md)
- [How to contribute](CONTRIBUTING.md)
- [Security reporting](SECURITY.md)
- [Dedication](DEDICATION.md)

**Installation:** there is nothing to install yet. When the first runnable version exists, this section will contain verified, copy-and-paste commands, supported platforms, expected output, and troubleshooting steps.

## Honesty and evidence

Project status, implemented features, measured characteristics, and external reference material must remain distinguishable. Every future reliability or performance claim should link to a reproducible procedure and dated result. See [verification policy](docs/VERIFICATION.md).

## License

SedravaSQL is licensed under the **Apache License, Version 2.0**, matching [Sedrava](https://github.com/VasiliiBabkin/sedrava/blob/main/LICENSE). See the complete terms in [LICENSE](LICENSE). Contributions must follow [CONTRIBUTING.md](CONTRIBUTING.md); third-party dependencies retain their own licenses.

## In memory

This project is dedicated to the memory of **Дмитрий Грабыльников**, a friend of the creator who shared his enthusiasm for bold, unconventional ideas.

[Read the dedication](DEDICATION.md).
