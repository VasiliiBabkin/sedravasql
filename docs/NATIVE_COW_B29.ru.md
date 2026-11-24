# B29 — бинарное дисковое copy-on-write B+Tree (исследование)

Базовый коммит: 363ccbc9542ed8e3ecf41e3619f78823ffeb97d4. Отдельная исследовательская ветка, не основной SQL backend. Rust, Linux/Unix, single-writer, доверенный приватный каталог.

## Формат и транзакции

Страницы 4096 байт, прямой offset page_id * 4096, бинарные узлы/строки, SHA-256 каждой страницы, identity 128 bit, page_id и generation. Динамическое увеличение файла, до 65536 страниц (256 MiB данных), до 32768 физических ключей, глубина до 8. Ключи ASCII valid_id до 128 байт, UTF-8 value до 256 байт, tombstone с ревизией; leaf до 8, branch до 24 separator keys. FIFO decoded page cache до 64 узлов. B+Tree без leaf links: диапазонный обход через границы дочерних узлов.

Пакет до 16 различных ключей, CAS revisions; validate and stage (до 256 новых узлов), удаление недостижимых промежуточных staged pages и перенумерация ссылок в порядке исходных page IDs. Ошибка admission не изменяет файл. Append pages -> sync_all -> новый .head.tmp (checksummed manifest) -> sync_all -> rename в .head -> fsync parent directory -> подтверждение. Ошибка I/O после начала append блокирует handle. No durable request receipts.

Open: проверка private/single-link, exclusive lock, identity, manifest, длины и всех достижимых страниц текущего корня (не всех исторических страниц), порядок ключей, границы, глубина и число строк. Неполный неопубликованный хвост обрезается только после проверки committed tree. Повреждённые committed pages не откатываются молча к старому корню. .head.tmp и stale .lock требуют ручного разбора; тест удаляет только lock подтверждённо завершившегося дочернего процесса и его staging-файл. Два файла (data и .head) должны резервироваться согласованно под exclusive access. Нет защиты от подмены доверенным владельцем, включая откат валидной старой копии manifest.

## Проверки

71 native Rust tests PASS (включая служебный crash child); strict Clippy PASS. Новый тест 2048 permuted keys, 128 atomic batches, splits, exact BTreeMap oracle, CAS conflict and rejection unchanged, UTF-8 update, tombstone, reopen/cache. Коррупция текущей страницы/manifest, foreign manifest, hardlink, truncated committed file отклоняются; неопубликованный хвост обрезается. Три реальных child-process exits (86) после sync pages, после sync staging head, после rename head: старое/старое/новое поколение, оба ключа атомарны; последующая запись работает. Это crash процесса с debug fault sync, не физический power loss, не симуляция всех torn writes.

## Сравнение B25 и B29

Release builds, 2-vCPU shared VDS, CPU 0, loadavg 2.73 до исходной серии и 2.96 до финальной. B25 1024 fixed slots, B29 dynamic. 128/512 keys: B25, COW single, COW batch16, batch16, single, B25; 2048: только COW single/batch16 (B25 не вмещает). 240 point и 240 eight-row range timed queries на запуск после 5 warmups; точные результаты сверены. Build включает создание файла и вставки; reopen включает проверку. Размер COW = data + head; B25 data file, без transient WAL/lock. Commit latency per batch (не per row). Общая VDS нагрузка может влиять на fsync и времена, не сравнение с SQLite.

| Ключей | Вариант | Build, мс | Open, мс | Байты | Get median, мс | Range8 median, мс | Commit median, мс |
|---:|---|---:|---:|---:|---:|---:|---:|
| 128 | b25 | 805.08 | 56.74 | 4202496 | 0.319165 | 1.003526 | 5.281038 |
| 128 | cow-single | 566.95 | 3.17 | 1114112 | 0.012265 | 0.015081 | 3.781984 |
| 128 | cow-batch16 | 72.34 | 2.84 | 385024 | 0.009320 | 0.012585 | 8.533290 |
| 512 | b25 | 3317.72 | 171.05 | 4202496 | 0.646543 | 2.006113 | 5.506828 |
| 512 | cow-single | 2658.11 | 14.03 | 6246400 | 0.013352 | 0.135195 | 4.402531 |
| 512 | cow-batch16 | 340.52 | 10.09 | 2813952 | 0.019883 | 0.137213 | 8.916743 |
| 2048 | cow-single | 13419.04 | 69.30 | 28618752 | 0.072629 | 0.192022 | 5.107940 |
| 2048 | cow-batch16 | 1723.99 | 36.34 | 16269312 | 0.065727 | 0.145987 | 12.392863 |

## Staging compaction

| Ключей | Batch16 до, байт | Batch16 после, байт | Уменьшение |
|---:|---:|---:|---:|
| 128 | 1114112 | 385024 | 65.44% |
| 512 | 6246400 | 2813952 | 54.95% |
| 2048 | 28618752 | 16269312 | 43.15% |

Размеры детерминированы данным workload. Стадия compact уменьшает запись промежуточных узлов внутри одного batch, не освобождает страницы прежних committed generations. B29 существенно быстрее B25 на tested point/range workload; стоимость роста и открытие остаются ограничениями. 512 single COW занимает больше диска, чем B25; 2048 keys batch16 всё ещё ~16.3 MB для коротких значений. Нужны reclamation/vacuum, overflow values, более плотная упаковка и строгая привязка SQL receipt/schema/transaction model до интеграции. Не обещаем производственную надёжность, ACID при power loss, concurrency или превосходство над SQLite.

## Воспроизведение

cargo test --manifest-path native-engine/Cargo.toml --locked --all-targets; cargo clippy --manifest-path native-engine/Cargo.toml --locked --all-targets -- -D warnings; cargo build --manifest-path native-engine/Cargo.toml --locked --release --example b29_cow; taskset -c 0 TARGET/release/examples/b29_cow. Для исходной серии замените cow_tree.rs на сохранённый evidence source в отдельном временном checkout. [Первичные измерения, исходная версия и manifest](../evidence/benchmark/b29-cow-2026-11-24/).
