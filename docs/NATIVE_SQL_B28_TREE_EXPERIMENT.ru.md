# B28-A — экспериментальный B+Tree SQL-индекс (исследование, не main)

Исходная база B27: 075597a707cde19cb6ddc31360b2c98fd9c3c295. Прототип подключает B25 через явный SQL read-only API TreeSqlReader и CLI sql-tree-build LOG TABLE NEW_TREE / sql-tree LOG TABLE TREE SQL [PARAMS_JSON]. Журнал остаётся единственным источником строк, транзакций и квитанций. Дерево — расходуемый индекс адресов и ревизий, не SQL storage backend.

## Контракт

- 512 физических ключей (включая tombstones), 1024 pager slots, ASCII IDs; preflight превышения до создания файла. Не перезаписывает существующий путь. При ошибке построения возможен неполный файл, который не следует использовать.
- Открытие проверяет журнал, структуру дерева и точное совпадение числа, ключей и ревизий, с именем таблицы в marker. Это проверка эквивалентности индексного набора, не доказательство provenance: другая база с тем же набором ключей и ревизий безопасна для key-only чтения, поскольку значения читаются из текущего журнала.
- Эксклюзивный borrow NativeLog на срок сессии; не допускается запись из этого handle. Проверка каждой возвращаемой строки, exclusive continuation, SELECT with DISTINCT/ORDER BY/LIMIT and half-open PK >= lower AND PK < upper, дополнительные AND predicates; прочие запросы отклоняются. Обычный SQL не изменён, кроме выделения общей проекции результата.
- Не поддерживает SQL-записи, JOIN, агрегаты через дерево, dynamic growth, многопользовательский режим. Tree::open может выполнить прежнее WAL recovery. Доверенная private directory; внешние изменения файлов при открытом handle не поддерживаются.

## Измерения

Одинаковые SELECT и точные эталонные результаты. По 240 измерений на ширину и способ в каждом прогоне, 32/128/512 ключей, ABBA, CPU 0. Время build/attach отдельно, не в медиане SELECT. Первый прогон с конкурирующей сборкой (loadavg 6.51), повторный после завершения сборки (2.50); VDS shared 2 vCPU. Не сравнение с SQLite и не прогноз для больших баз.

| Прогон | Ключей | Ширина | Scan, мс | Tree, мс | Tree/scan |
|---|---:|---:|---:|---:|---:|
| range-bench.json | 32 | 1 | 0.097176 | 0.452684 | 4.658 |
| range-bench.json | 32 | 8 | 0.102657 | 2.022590 | 19.702 |
| range-bench.json | 32 | 32 | 0.107233 | 6.013568 | 56.080 |
| range-bench.json | 128 | 1 | 0.336256 | 0.782106 | 2.326 |
| range-bench.json | 128 | 8 | 0.340415 | 1.875354 | 5.509 |
| range-bench.json | 128 | 32 | 0.293516 | 5.429397 | 18.498 |
| range-bench.json | 512 | 1 | 1.196233 | 1.976212 | 1.652 |
| range-bench.json | 512 | 8 | 1.166520 | 4.913894 | 4.212 |
| range-bench.json | 512 | 32 | 1.234230 | 11.312269 | 9.165 |
| range-bench-repeat.json | 32 | 1 | 0.100403 | 0.451875 | 4.501 |
| range-bench-repeat.json | 32 | 8 | 0.103740 | 1.214365 | 11.706 |
| range-bench-repeat.json | 32 | 32 | 0.098058 | 4.657067 | 47.493 |
| range-bench-repeat.json | 128 | 1 | 0.326087 | 0.938968 | 2.880 |
| range-bench-repeat.json | 128 | 8 | 0.307615 | 2.578040 | 8.381 |
| range-bench-repeat.json | 128 | 32 | 0.291131 | 6.816472 | 23.414 |
| range-bench-repeat.json | 512 | 1 | 1.249340 | 1.181145 | 0.945 |
| range-bench-repeat.json | 512 | 8 | 1.336135 | 2.989231 | 2.237 |
| range-bench-repeat.json | 512 | 32 | 1.405574 | 10.749904 | 7.648 |

Дерево занимает 4 202 496 байт независимо от числа ключей (фиксированные слоты); 512-строчный журнал — 341 850 байт в эксперименте. Во втором прогоне 512 keys: build 4968.06 ms, attach 945.97 / 729.73 ms. Узкий диапазон 1 строки в повторе почти равен scan; 8 и 32 строки ощутимо медленнее. Диск-индекс не принят как основной SQL path. Причины для дальнейшего профилирования: JSON nodes/rows, checksums, fixed-page probing, repeated reads and cache behavior; не утверждаем, что любой B+Tree медленнее.

## Проверки и воспроизведение

69 native tests PASS (66 previous + 3 B28-A), strict Clippy PASS. Multi-page range, tombstones, bound params, invalid/unsupported SQL, wrong/incomplete/corrupt/stale index, capacity, no overwrite, journal unchanged. Benchmark: cargo build --manifest-path native-engine/Cargo.toml --locked --release --example b28_ranges; taskset -c 0 TARGET/release/examples/b28_ranges. [Raw samples and hashes](../evidence/benchmark/b28-disk-tree-2026-11-24/).
