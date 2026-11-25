# B35b — общий SQL backend contract

SqlStore объединяет TransactionalStore, sequence, lookup_receipt, ordered scans и cache вторичных индексов. Tables и SQL работают через этот интерфейс, NativeLog остаётся штатным backend. CowStore — явный экспериментальный backend на Unix. CLI: cow-init, cow-verify, cow-sql, cow-sql-bind, cow-sql-transaction, cow-sql-sequential, cow-explain. Формат выбирается явно, ошибочный тип отклоняется.

Два целевых теста PASS: parity SQL (DDL/DML, диапазоны, вторичный индекс и инвалидация, JOIN, GROUP BY и агрегаты, последовательные транзакции, restart replay и конфликт) и CLI roundtrip. Strict Clippy PASS. Ограничения B35a сохраняются: canonical frame history on COW, Replay и SQL row index в RAM, 32 MiB payload, полный обход перед commit. Не production, не direct disk row index. Основные ветки и выпуски не переключены.
