# SedravaSQL 0.1.0-alpha.2 — candidate

Experimental, not production-ready. NativeLog 0.0.2 remains the SQLite-free limited SQL engine; the managed SQLite key/value CLI is separate. B37 internal storage contract and A24 additive DDL migrations are included. COW B32–B36 is not adopted as the default backend. No arbitrary ALTER/DROP, general SQL server, physical power-loss guarantee, automatic adoption of old project databases or valuable data. Linux x86-64 GNU is the qualified target; other targets are not certified.

Build and test both crates with Cargo --locked. Native and managed CLIs now provide --version. Database format is not changed by the version labels. Prior A26 and A28 evidence applies to the earlier candidate; final A30 qualification is required before release.
