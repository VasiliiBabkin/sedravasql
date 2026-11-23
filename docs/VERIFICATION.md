# Evidence and verification policy

SedravaSQL treats documentation claims as testable engineering statements.

## Vocabulary

- **Implemented:** feature exists in a committed revision and has runnable usage instructions.
- **Experimental:** runnable but with named limitations or incomplete validation.
- **Planned:** idea/specification, not runnable.
- **Verified:** passed a stated test procedure in a specified environment, with public result artifacts.
- **Not verified:** no adequate published evidence. This includes promises or assumptions about future performance.

None of these labels imply production readiness unless a separate, explicit, bounded release assessment supports it.

## Required record for every performance / reliability claim

1. Exact claim and its boundaries; avoid universal or absolute language.
2. Source commit and released version (if any).
3. Test program or script, arguments, fixtures, seed and expected result.
4. Hardware, operating system, filesystem, SQLite and toolchain versions, relevant configuration.
5. Command(s) to reproduce and unmodified output/artifacts.
6. Results including failures, repetitions and relevant uncertainty.
7. Comparison baseline (if comparative) under matching conditions.
8. Date, author/reviewer, limitations, and known gaps.

Reports should be stored under a future versioned location such as `evidence/<commit-or-release>/` and linked from [CLAIMS.md](CLAIMS.md). The directory is a convention, not existing evidence.

## Minimum test families before recommending real data storage

- API correctness, constraints, input validation and parameterization.
- Atomicity and recovery after process termination at multiple write points.
- Crash and power-loss simulation with documented assumptions.
- Concurrent readers/writers, conflicting updates, and stale previews.
- Disk-full, permission errors, interrupted backup, damaged database file.
- Data-history consistency and rollback across failures.
- Backup creation, isolated restore, integrity verification and application-level invariants.
- Fuzzing and malformed inputs, within a documented scope.

A passing `PRAGMA integrity_check` is useful but is not proof that business data is complete or correct. Follow up with application-level invariants and backup recovery tests.

## Benchmarking rules

- Publish raw results, exact test commands, dataset generator, warm-up and repetition policy.
- Compare equivalent workloads, durability modes, indexes, and hardware.
- Measure the cost of safety checks instead of hiding it.
- Report median, distribution, throughput and error rates as appropriate.
- Do not extrapolate from microbenchmarks to general workload superiority.

## Corrections and transparency

If a claim turns out to be wrong, amend the claim register and user-facing docs, preserve the original result where practical, and explain the correction. Untested functionality must remain labelled as such.

Source material is listed in [REFERENCES.md](REFERENCES.md).

## Initial independent VDS run — 2026-11-23

Ubuntu Linux x86_64; Rust 1.100.0; bundled SQLite from
rusqlite 0.32.1. Reproduce with:

    cargo fmt --all -- --check
    cargo test --locked --all-targets
    cargo build --locked
    python3 scripts/verify_local.py

Observed: five Rust integration tests PASS, real CLI tests PASS,
12 concurrent processes (one success, 11 CAS conflicts), four
debug-only child process fault windows around COMMIT, and SQLite
Backup API capturing committed WAL data from a live writer.
Tests create and remove temporary files. They do not prove power-loss
durability, performance, encryption, cross-OS behavior or production
security. Optimized release does not activate fault injection.

## 2026-11-23 — приложение: durable response API

Исходная база: b5831d7c5961042ff37feaaf4b4fef59b81b5f64.
На Ubuntu x86_64, Rust/Cargo 1.100.0 выполнены
`cargo fmt --all -- --check`,
`cargo test --locked --offline --all-targets`.
Итог: **6 интеграционных тестов PASS**; новый тест сохраняет 45
последовательных ответов, проверяет старые ключи, перезапуск,
точный replay и отказ от конфликтующего повторного запроса.
Тесты выполнялись в временных каталогах; публичные порты и
производственные данные не затрагивались. GitHub Actions не использованы.
Power-loss и долговременное хранение не подтверждены.


## 2026-11-23 — SQLite-free native engine experimental slice

A new **separate Rust crate** native-engine/ has no rusqlite/SQLite
dependency. Verification commands on Ubuntu x86_64 / Rust 1.100.0:

    cargo fmt --manifest-path native-engine/Cargo.toml --all -- --check
    cargo test --manifest-path native-engine/Cargo.toml --locked --offline --all-targets
    cargo test --locked --offline --all-targets
    cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview
    python3 scripts/verify_native_log.py

PASS: 8 native-engine integration tests, 1 SQLite/native-format
isolation test, 6 existing SQLite integration tests.
The separate CLI smoke test wrote two keys in one batch, checked
replay/CAS across independent child processes and two debug-only forced
exits around append: incomplete frame rejected without automatic
truncation, complete synced frame replayed after simulated lost reply.
Everything used private temporary directories, no real data or network
listener; the original Sedrava A6 code remains unchanged.

This does not prove physical power-loss durability, resource limits,
automatic recovery, stable on-disk format or production use.
See [architecture ADR](SELF_HOSTED_STORAGE.ru.md).

## 2026-11-23 — shared batch transaction contract A1

Ubuntu Linux x86_64, Rust/Cargo 1.100.0, VDS. NativeLog and opt-in SqliteBatchStore implement the same TransactionalStore interface. Tests (tests/batch_conformance.rs) cover 46 equal multi-key/single-key responses on both, replay after restart, CAS and ABA, integrity of SQLite per-key audit/receipt, 8 competing SQLite writer handles and SQLite Online Backup API. Formatting, locked offline cargo tests, native-engine dependency-free-of-SQLite check, and clippy with -D warnings were run. No production data or web services changed; no GitHub Actions time consumed. The original SQLite API tests and native CLI crash tests remain passing. This does not establish physical power-loss guarantees, production recovery, or schema migration. See [Batch Contract A1](BATCH_CONTRACT_A1.ru.md).

### 2026-11-23 — Native recovery B1 verification

4 additional native integration tests cover truncated length/payload/checksum, refuse complete-frame corruption, verify SHA-256, preserve source files, reject locked/preexisting paths, and replay multiple retained receipts. Separate child-process CLI smoke uses debug crashes before and after sync_all. No power-loss or production recovery claim. See [protocol](NATIVE_RECOVERY_COPY_B1.ru.md).

## B5.1 native damaged-file matrix

[Reproduction and limits](NATIVE_FAULT_MATRIX_B51.ru.md):
5 restored log roundtrips, 145 flipped bytes, 60 truncated inputs,
15 extra-tail inputs and 20 pre-existing partial targets. No
SIGKILL-while-fsync or physical power-loss testing performed.

## Native B5.2 controlled process-exit matrix

Run scripts/verify_native_crash_b52.py with a DEBUG native preview
binary on disposable files. Eight tests each at after_length,
after_payload and after_sync; controlled exit 86 and verified stale
lock ownership. B1 recovery and exact idempotent replay PASS.
Hardware power-loss is NOT tested. Details: NATIVE_CRASH_B52.ru.md.
