# B36 — сравнение трёх backend на VDS

Исходное ядро 40a0cdc214af177957f94d0173b6b5c5d481c2f2. Rust Release 1.100.0, Linux 6.8, ext4, 2 vCPU Intel Xeon Skylake, 7.8 GiB RAM. Сравнение через общий TransactionalStore: NativeLog, CowStore, SqliteBatchStore (SQLite bundled, WAL, synchronous FULL). Последний дополнительно сохраняет аудит; это не тест bare SQLite SQL, не сравнение одинаковых физических форматов или гарантий. Все операции выполняются одним процессом без сети. Системный кеш не сбрасывался, тест не изолирован от других процессов VDS. Нет power-loss испытания.

На каждый размер: 3 независимых прогона каждого backend, новый приватный временный каталог, порядок backend циклически меняется. 64/256 пакетов вставки по 4 строки, 32/128 пакетов обновления, payload 128 ASCII bytes/строка, 1024 точечных чтения, 128 повторов первоначальных запросов, 5 повторных открытий. Проверяются sequence, ревизии, старые квитанции, конфликт ключа и все финальные записи. Все 18 прогонов PASS. Время операции — wall-clock Instant; медиана из медиан трёх прогонов, p95 в summary — медиана per-run p95, не pooled p95. Throughput: число транзакций / сумма их измеренных задержек, медиана по прогонам. Размер закрытой базы — сумма размеров файлов в тестовом каталоге после drop, включая COW head; live size включает WAL/SHM/lock при наличии. Временные базы удалены автоматически.

## 1024 строки, медиана

| Backend | Insert 4 ms | Update 4 ms | Read us | Replay us | Reopen ms | Closed bytes | Insert tx/s |
|---|---:|---:|---:|---:|---:|---:|---:|
| NativeLog | 1.320 | 1.308 | 1.16 | 11.62 | 20.53 | 378524 | 657.06 |
| SQLite batch | 1.966 | 1.963 | 15.60 | 38.93 | 20.81 | 929792 | 453.49 |
| CowStore | 17.272 | 31.222 | 1.26 | 13.41 | 112.91 | 2441216 | 57.89 |

NativeLog/CowStore читают из Replay в RAM, SQLite выполняет запрос к своему индексу. Объём RSS и cold-cache не измерялись. Не экстраполировать на большие базы, concurrency или SQL parser.

## Диагностика COW

Отдельный raw CowTree тест: 384 транзакции с двумя новыми ключами (700/100 bytes), три повтора каждого режима, после reopen проверены все значения. Без SQL/receipt/Replay. Сравнение append-only commit и commit_reusing (включает full ownership verification и распределение свободных страниц), не изолированный профиль отдельных функций. При reuse меньше страниц, но резко растёт задержка. Append-only не является заменой проверок безопасного reuse; не переключать CowStore на него ради результата.

## Решение B37

B36 NO-GO для продвижения CowStore как более быстрого backend. Не изменять default NativeLog. B37a: отдельный формат прямого хранения строк (ordered namespace/key без хеширования порядка, сохраняющий ограничения и tombstone/revision). Долговременные полные результаты транзакций — отдельно, атомарная публикация row root/receipt root/free-space root; не терять историю ради производительности. B37b: доказуемый free-space ownership metadata, COW publication, re-open validation, повреждения metadata и process exit на всех границах; не переиспользовать страницы текущего root. B37c: адаптер SQL с ordered scan, parity и повтор B36 с большими масштабами/RSS. Сначала доказательство целостности, потом снижение полной валидации на hot path. Рабочий NativeLog и публичные релизы без изменений.

Воспроизведение: cargo run --release --locked --offline --example b36_backends; cargo run --release --locked --offline --example b36_cow_diagnostic; python3 scripts/summarize_b36.py RAW_JSONL. Исходные результаты и среда: evidence/benchmark/b36-backends-2026-11-25. Строгий Clippy обоих examples PASS.
