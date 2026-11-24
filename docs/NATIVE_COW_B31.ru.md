# B31 — overflow pages для COW-хранилища

Исследовательская ветка от B30 e59758058eaecf7864ebb46fb86c9a377f513a4e. Не SQL backend, не production.

## Формат и ограничения

Текст UTF-8 без NUL, 0..65536 bytes; до 256 bytes inline. Для более крупных значений leaf содержит специальный marker 65534, длину, ссылку на первый overflow page и SHA-256 полного значения. Overflow kind 3: count, next page ID, 1..4000 bytes. Ссылки строго убывают по ID, последний next=0. Канонический размер фрагмента min(4000, remaining); максимум 17 дополнительных страниц для 65536 bytes. Проверяются checksums, identity, page IDs, generation, длина, SHA-256 полного значения и UTF-8 после сборки. При open/compact проверяется уникальность достижимых страниц, в том числе запрет общего overflow chain у двух записей. Это обнаружение повреждения, не криптографическая аутентификация против владельца файлов.

SEDRHDR3 writes, read compatibility с SEDRHDR1/2 на проверенном inline формате. Старые B29/B30 readers v3 не читают. Формат обычных leaf/branch сохраняется. Страницы 4096 bytes; cache максимум 64 nodes, полные крупные значения в кеше не хранятся. Пакеты максимум 16 mutations, MAX_STAGE=640 nodes, MAX_PAGES=65536, MAX_ROWS=32768, прежние CAS и fsync/rename publication. Stage compaction обходит ссылки leaf→overflow и chain, перенумеровывает все достижимые staged links. Дисковые старые поколения ещё не переиспользуются.

B30 compact_copy пересоздаёт цепочки в независимой копии, сохраняет ревизии/generation/tombstones. Дополнительный лимит materialized payload 32 MiB на snapshot (не лимит общего RSS или размера базы); превышение отклоняется до создания destination data/head. Point/range читают значения по прежнему API; максимум 256 records на range. Большие значения требуют памяти для сборки строки и увеличивают I/O.

## Проверки

81 native Rust tests PASS (включая три служебных crash-child tests); strict Clippy PASS. В B29 обновлён тест отказа с 257 до 65537 bytes; в B30 проверка версии записываемого заголовка обновлена на v3. Новые шесть тестов: размеры 0/256/257/4000/4001/65536, Unicode через границу 4000 bytes, 16 максимальных значений в одной транзакции, атомарный отказ CAS/oversize/NUL, update inline↔overflow, delete/restore, compact/reopen, повреждение фрагмента с пересчитанной page checksum, self-link и duplicate ownership с пересчитанными checksums, совместимый v2 inline header (синтетический), три process exits после sync pages/sync head.tmp/rename. После подтверждения PID завершившегося child тест вручную очищает lock и неопубликованный staging head; восстанавливается старое или новое поколение целиком. Не физическое отключение питания.

## Измерения

Один release-прогон на shared 2-vCPU VDS, CPU 0, loadavg 2.65. 192 и 960 ключей, исходные размеры 64/257/4000/4001/16384/65536 bytes; 16-op batches. Затем первая четверть ключей обновлена/удалена; после compact и reopen проверен каждый ключ относительно BTreeMap oracle. Размер файла включает data и head. 80 point samples каждого неизменённого size class, 120 mixed range8 samples; 5 warmups. Нет сравнения с SQLite или B30 (B30 не поддерживает эти размеры).

| Ключей | Живых | Полезные байты | До compact | После compact | Compact ms | Reopen ms | Insert16 median ms |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 192 | 176 | 3215408 | 4927488 | 3780608 | 335.070 | 75.795 | 15.126082 |
| 960 | 880 | 16077040 | 24760320 | 18874368 | 2071.401 | 335.334 | 15.068338 |

| Размер значения, bytes | 192 keys, get median ms | 960 keys, get median ms |
|---:|---:|---:|
| 64 | 0.010133 | 0.039766 |
| 257 | 0.012301 | 0.092033 |
| 4000 | 0.054401 | 0.103535 |
| 4001 | 0.096894 | 0.146111 |
| 16384 | 0.539731 | 0.358005 |
| 65536 | 1.373082 | 1.335884 |

Лимит compact payload: ровно 33554432 bytes принят (512 значений по 65536 bytes, packed 35938304 bytes, 3513.40 ms); 33554433 bytes отвергнут, destination отсутствует. Это отдельный release boundary experiment.

Пока не реализованы повторное использование страниц, persistent receipts, SQL schema/transaction integration, автоматический switch compact pair, power-loss qualification. Значения ограничены UTF-8 текстом; BLOB не добавлен.

## Воспроизведение

cargo test --manifest-path native-engine/Cargo.toml --locked --all-targets; cargo clippy --manifest-path native-engine/Cargo.toml --locked --all-targets -- -D warnings; cargo build --manifest-path native-engine/Cargo.toml --locked --release --example b31_overflow; taskset -c 0 TARGET/release/examples/b31_overflow > overflow-bench.json. [Raw samples and hashes](../evidence/benchmark/b31-overflow-2026-11-24/).
