# B30 — безопасная компактная копия COW-хранилища

Исследовательская ветка, база B29 d0462132f88d967f3da1e295f10fb0a1d8d93184. Не SQL backend, не production.

## Контракт

compact_copy(destination) сохраняет все физические ключи, значения, tombstones, CAS revisions и generation; новый identity. Проверяет исходный manifest, длину, identity и достижимые страницы без доверия к прогретому кешу. Сортированные записи упаковываются в листья по 8, внутренние группы балансируются до 25 children; новые page IDs выдаются снизу вверх. Память ограничена MAX_ROWS=32768, VALUE_MAX=256 bytes и лимитами узлов. Старые исторические страницы и промежуточные версии в копию не попадают.

Сохраняет исходные data/head bytes, удерживая exclusive lock. Не перезаписывает существующую цель или её sidecars; пересечение семейств имён source/destination (включая отсутствующий .head.tmp) запрещено, проверяются canonical parents. Trusted private directories only. Новые страницы sync_all, staging head sync_all, rename, directory fsync. Под destination lock повторно читаются реальные страницы и manifest и сравниваются все записи. Незавершённый результат требует ручной проверки; исходный файл не удаляется. Переключение потребителя на новый путь — отдельная операция оператора. Никакой in-place reuse, atomic replacement пары файлов или автоматического vacuum в B30.

B30 читает SEDRHDR1 и SEDRHDR2; пишет SEDRHDR2. В v2 generation больше не ограничен количеством страниц, поскольку компактная копия сохраняет историю ревизий при меньшем файле. Старый B29 reader не читает v2. Бинарный формат узлов не менялся.

## Испытания

75 Rust tests PASS (включая два служебных child tests), strict Clippy PASS. Новые тесты: packed snapshot с generation > pages, tombstone/revision, запись после reopen, source unchanged, no overwrite, overlapping file families, empty copy, synthetic exact v1 header, corruption after cache warmup. Три child-process exits после sync pages, sync staging head, rename head: исходный файл побайтно неизменен; до публикации цель отвергается, после публикации открывается и содержит все записи. Stale locks удаляются в тесте только после подтверждения PID завершившегося процесса. Это проверка process crash, не физического отключения питания.

## Масштабирование на VDS

Один release-прогон, CPU 0, общий 2-vCPU VDS, loadavg 3.86 перед запуском. Пять последовательных фаз по 4096 уникальных ключей, затем 4096 изменений существующих ключей (820 tombstones, 3276 значений размером 256 bytes). Каждая фаза 256 atomic 16-op batches. Перед и после уплотнения 240 точечных и 240 восьмистрочных диапазонных чтений (5 warmups). После каждой копии отдельный reopen и полная сверка каждого ключа с BTreeMap oracle. Только disposable fixtures удаляются после проверки. Размер включает data и 4096-byte head.

| Фаза | Физ. ключей | Живых | Generation | До, байт | После, байт | Уменьшение | Compact, мс | Reopen, мс |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 4096 | 4096 | 256 | 36401152 | 2195456 | 93.97% | 248.68 | 32.80 |
| 2 | 8192 | 8192 | 512 | 39378944 | 4382720 | 88.87% | 427.09 | 76.20 |
| 3 | 12288 | 12288 | 768 | 41373696 | 6569984 | 84.12% | 573.85 | 76.65 |
| 4 | 16384 | 16384 | 1024 | 43560960 | 8753152 | 79.91% | 685.98 | 103.61 |
| 5 | 20480 | 20480 | 1280 | 45744128 | 10940416 | 76.08% | 933.18 | 139.31 |
| 6 | 20480 | 19660 | 1536 | 50786304 | 10940416 | 78.46% | 861.13 | 141.96 |

До/после — размеры в каждой отдельной фазе с предыдущей компактной копией, не кумулятивный размер без vacuum. 20480 keys tested, не доказательство надёжности на MAX_ROWS или произвольном workload. Большие значения >256 bytes, overflow pages, persistent receipts, in-place reclamation и SQL migration отсутствуют. Общая нагрузка и один прогон не позволяют универсальные latency claims.

## Воспроизведение

cargo test --manifest-path native-engine/Cargo.toml --locked --all-targets; cargo clippy --manifest-path native-engine/Cargo.toml --locked --all-targets -- -D warnings; cargo build --manifest-path native-engine/Cargo.toml --locked --release --example b30_compact; taskset -c 0 TARGET/release/examples/b30_compact > compact-bench.json. [Первичные измерения и хеши](../evidence/benchmark/b30-compact-2026-11-24/).

Следующий этап: overflow values/packing и независимый crash-safe reclamation protocol; только затем receipts и SQL backend.
