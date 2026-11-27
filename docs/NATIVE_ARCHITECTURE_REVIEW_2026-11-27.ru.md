# Техническая ревизия B32–B36 · 27 ноября 2026

## Решение

NativeLog (B28) остаётся основным SQLite-free SQL backend. Не объединять весь research/B36 с main. Сохранить исследовательские ветки, измерения, форматы и тесты без переписывания истории. В первую очередь переносить backend-independent SQL contract; кандидат начинается от main 98a0ba58caadd2dec3d53f64ae9347db0881c3e3, меняет только lib/sql/table и документацию. Интерфейс SqlStore остаётся crate-private, публичные execute/execute_bound/execute_transaction/execute_sequential/explain и Tables::new продолжают принимать NativeLog; внутренний кеш не становится публичным API. Новый on-disk формат и новые runtime-функции не добавлены. До CI не merge.

## Проверенные исходные точки

| Этап | Git revision | Результат / граница |
|---|---|---|
| B32 | af407e5 | COW reuse с полным uncached ownership traversal; высокая стоимость на hot path |
| B33 | 3779d49 | root + generation receipt атомарно; результат SQL не сохраняется |
| B33b | 7c0c2cb | paged receipt history, линейный поиск, заявленный предел 32768, фактическое испытание 240 |
| B34a | c6f7ef8 | независимый read-only образ NativeLog, 32 MiB serialized budget; не writable CowStore |
| B34b | dd39f24 | восстановление канонического NativeLog в новый файл; неканонические frames отклоняются |
| B35a | 9a77e79 | CowStore сохраняет canonical frames в COW; все logical rows/receipts восстанавливаются в RAM |
| B35b | 40a0cdc | общий SQL contract, explicit CLI; лимиты логического журнала плюс COW budgets |
| B35c (Sedrava) | be97e73 | opt-in --backend cow; 63 HTTP запросов на каждом backend, в исследовательской ветке |
| B36 | 956899c | 18 сравнительных и 6 диагностических release-прогонов |

На GitHub и Forgejo CI-copy подтверждены B32–B36 по точным SHA. Основные ветки 98a0ba5 (SQL), 30c8b3a (язык); общий предок research 363ccbc. Все исследовательские ветки сохранены. Предыдущие публичные alpha-релизы не менялись.

## Причины NO-GO для CowStore как default

* B36 / 1024 logical rows: median 4-row insert 17.272 ms против NativeLog 1.320 и SQLite batch 1.966; update 31.222 против 1.308 и 1.963; closed bytes 2441216 / 378524 / 929792. Read medians RAM-based, не доказательство disk-index performance.
* CowStore дублирует journal history на страницах, не заменяет RAM Replay дисковым row index. B33b receipts не являются механизмом full NativeCommit в B35: B35 восстанавливает их из frames.
* Диагностический B36 raw COW: verified reuse 594 pages / 8091 ms total против append-only 1685 pages / 1822 ms, 384 transactions. Это сравнение режимов, не изолированный профиль hash/IO. Не отключать проверку ownership ради результата.
* B34 read-only image, B35 writable frame store и B33 generation receipts — разные contracts. Нельзя объявлять автоматическую миграцию в B35 или заменять B35 B34 образом.
* Процессные fault tests не являются power-loss qualification; stale locks/stage требуют операторской процедуры. COW data/head должны копироваться согласованно. Нет доказательства работы с большими БД/несколькими писателями.
* NativeLog тоже preview: предел 64 MiB и 65536 транзакций, single writer, RAM state. Он принят как ограниченная текущая архитектура, не как production database.

## План без повторения работы

1. B37-contract: выборочно интегрировать внутренний SQL storage contract после CI. NativeLog — единственная штатная реализация; COW не включать в основной релиз.
2. Sedrava A22: единое демонстрационное веб-приложение с реально работающим сервером, статическим UI и SQL CRUD (сохранение после перезапуска, typed params, role auth, атомарные операции, негативные проверки). Повторно использовать A17–A21, не создавать второй SQL parser или новые storage semantics. Начать с loopback, не публиковать сервер с пользовательскими данными без дополнительной квалификации.
3. B37 research: прямой упорядоченный row index с namespace/key, revisions, tombstones и полными историческими receipt results; совместное атомарное root publication; доказуемая карта ownership/free space; сначала corruption/reopen/fault tests, затем измерения. Сопоставить не только SQLite, но и NativeLog на одинаковом контракте, разных объёмах и RSS. Не переносить hot-path optimizations без доказательства. Сохранять B32–B36 как oracle и базу регрессии.
4. B38: только после выбора нового формата — проверяемая миграция NativeLog в writable backend с сохранением результатов и rollback.
5. Forgejo: GitHub остаётся источником публичных веток/релизов; mirror pull отдельно, writable CI-copy отдельно. Каждая CI ветка строится от конкретного SHA, workflow-only commit отдельно, checkout SHA проверяется. Изолированный runner 1 vCPU / 2 GiB / concurrency=1, использовать последовательно; не запускать GitHub Actions для текущего research. Артефакты выпускать только после тестов и checksum; не публиковать автоматически.
6. Релиз — после регрессии обоих проектов, end-to-end демонстрации и честного описания ограничений. Самостоятельность относится к NativeLog/SedravaSQL SQL runtime, а не ко всем зависимостям (SQLite adapter пока сохраняется).

## Воспроизведение и gates

Сохранить точные SHA входа и workflow-only commit; CI: cargo build --locked; cargo test --locked --all-targets; cargo test --manifest-path native-engine/Cargo.toml --locked --all-targets; strict Clippy. Проверить B36 research отдельно, включая прежние crash tests. До завершения CI статус кандидата CONDITIONAL / не merge. Источник B36: docs/NATIVE_B36_BENCHMARK.ru.md и evidence/benchmark/b36-backends-2026-11-25 в ветке research/b36-backend-benchmark. Измерения 25 ноября не повторять под нагрузкой другого runner и не сравнивать напрямую host vs container.
