# Primary technical references

These references provide context for **underlying technology and contributor practices**. They are not evidence of SedravaSQL functionality.

Checked during initial documentation: **2026-11-22**.

## SQLite primary documentation

- [What SQLite is](https://www.sqlite.org/about.html) — scope and design.
- [SQLite transactions](https://www.sqlite.org/transactional.html) — described ACID semantics.
- [SQLite transaction syntax and locking](https://www.sqlite.org/lang_transaction.html) — reader/writer behavior.
- [How SQLite is tested](https://www.sqlite.org/testing.html) — fault-injection and other verification methods.
- [Write-Ahead Logging](https://www.sqlite.org/wal.html) — WAL operation and trade-offs.
- [How an SQLite database can become corrupt](https://www.sqlite.org/howtocorrupt.html) — limits of integrity guarantees.
- [SQLite backup API](https://www.sqlite.org/backup.html) — supported backup mechanisms.
- [SQLite PRAGMA commands](https://www.sqlite.org/pragma.html) — `integrity_check`, journal and synchronization configuration.

## GitHub primary documentation

- [About repository READMEs](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes).
- [Licensing a repository](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository).
- [Contributing guidelines](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/setting-guidelines-for-repository-contributors).
- [Security policy](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/add-security-policy).

## Citing evidence properly

Use exact document URLs and distinguish upstream SQLite facts from measured SedravaSQL behavior. For version-sensitive behavior cite the precise upstream version when an implementation version is selected.
