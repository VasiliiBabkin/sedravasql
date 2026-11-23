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

Old revision rejects new writes. An identical repeated request returns
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
