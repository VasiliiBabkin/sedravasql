# SedravaSQL

**Safety-first embedded database experiment — Rust + SQLite prototype.**

[Русская версия](README.ru.md)

> **Status: experimental source prototype.** Code and tests have been added, but a Rust build and test run have **not yet been verified**. There is no stable API, release, backup/recovery feature, or production-readiness claim. **Do not store important data in SedravaSQL.**

## Purpose

SedravaSQL explores verifiable data changes, explicit safety policies, understandable history, and eventually tested recovery procedures.

The experimental source implementation is a Rust library and CLI backed by SQLite. It currently manages only a TEXT key/value table. Source code is **not** a published release or proof of correctness. SQL compatibility, failure guarantees, performance, and platform support are not established.

SedravaSQL is designed as a separate project from the [Sedrava programming language](https://github.com/VasiliiBabkin/sedrava). Language bindings may follow; the database is not intended to require Sedrava.

## Current availability

| Capability | Status |
| --- | --- |
| Design documents and project principles | Drafted |
| Rust core and CLI source | Written; build not yet verified |
| SQL interface | Read-only SELECT source; not tested |
| Preview and safety checks for prefix deletion | Source prototype; not tested |
| Audit history of managed changes | Source prototype; not tested |
| Backup verification and recovery | Not implemented |
| Stable API and releases | Not available |
| Production use | **Not supported** |

No benchmarks or reliability results have been published.

## Start here

- [Русское описание](README.ru.md)
- [Prototype quick start, commands, limits, tests](docs/PROTOTYPE.md)
- [Threat model and exclusions](docs/THREAT_MODEL.md)
- [Scope and architecture](docs/ARCHITECTURE.md)
- [Roadmap and acceptance criteria](docs/ROADMAP.md)
- [Evidence and verification policy](docs/VERIFICATION.md)
- [Claims register](docs/CLAIMS.md)
- [Technical references](docs/REFERENCES.md)
- [How to contribute](CONTRIBUTING.md)
- [Security reporting](SECURITY.md)
- [Dedication](DEDICATION.md)

**Build (experimental, not yet independently verified):** install Rust 1.89+ and a C compiler; run `cargo build` and `cargo test`. See [PROTOTYPE.md](docs/PROTOTYPE.md) for disposable-data examples and limitations. There is no installer or release package.

## Honesty and evidence

Project status, implemented features, measured characteristics, and external reference material must remain distinguishable. Every future reliability or performance claim should link to a reproducible procedure and dated result. See [verification policy](docs/VERIFICATION.md).

## License

SedravaSQL is licensed under the **Apache License, Version 2.0**, matching [Sedrava](https://github.com/VasiliiBabkin/sedrava/blob/main/LICENSE). See the complete terms in [LICENSE](LICENSE). Contributions must follow [CONTRIBUTING.md](CONTRIBUTING.md); third-party dependencies retain their own licenses.

## In memory

This project is dedicated to the memory of **Дмитрий Грабыльников**, a friend of the creator who shared his enthusiasm for bold, unconventional ideas.

[Read the dedication](DEDICATION.md).
