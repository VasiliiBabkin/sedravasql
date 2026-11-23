# SedravaSQL claims register — 2026-11-23

| ID | Утверждение | Проверяемый источник |
|---|---|---|
| S-001 | Есть исполняемый Rust/SQLite прототип | `src/lib.rs`, `src/main.rs`, `Cargo.lock` |
| S-002 | CAS, idempotency, delete confirmation, bounded KV | `tests/transactions.rs`, `scripts/verify_local.py` |
| S-003 | Аудит/квитанции атомарны вместе с изменением | `Database::mutate`, четыре debug fault tests |
| S-004 | Конкурентные записи с одной ревизией сериализованы | Rust thread tests и 12-процессный тест |
| S-005 | Backup включает подтверждённый WAL | Rust backup tests и тест с открытым SQLite writer |
| S-006 | sqlite integrity_check и сверка связности audit/receipt | `Database::verify`, тесты |
| S-007 | Production, power-loss, arbitrary SQL, encryption | **Не проверено или не реализовано** |

Внешние основания: [SQLite transactions](https://www.sqlite.org/transactional.html),
[SQLite WAL](https://www.sqlite.org/wal.html),
[SQLite testing](https://www.sqlite.org/testing.html).
Заявления SQLite об её собственных испытаниях **не** являются
доказательством универсальной надёжности SedravaSQL.
