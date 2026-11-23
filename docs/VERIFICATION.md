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

Observed: four Rust integration tests PASS, real CLI tests PASS,
12 concurrent processes (one success, 11 CAS conflicts), four
debug-only child process fault windows around COMMIT, and SQLite
Backup API capturing committed WAL data from a live writer.
Tests create and remove temporary files. They do not prove power-loss
durability, performance, encryption, cross-OS behavior or production
security. Optimized release does not activate fault injection.
