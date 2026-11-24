# Native B11 — последовательное чтение диапазона

Экспериментальное улучшение B10. Файловый формат SEDRIDX1 не меняется, API range сохраняет контракт [start,end), 1..256, лексикографический порядок, пространство имён и tombstones.

Два бинарных поиска в проверенном каталоге определяют границы; затем один явный seek для непустого диапазона и последовательные read_exact по смежным строкам. Пустой диапазон не позиционирует файловый дескриптор. Каждая строка проверяется по SHA-256, namespace, key и revision; сохранена проверка идентичности и приватности перед и после чтения. Точечный get использует тот же внутренний декодер с собственным seek. Каталожная сложность O(log N + K) вместо B10 O(log N + K log N). Полная проверка при open остаётся обязательной.

CLI index-range возвращает дополнительный счётчик row_seeks (явные вызовы позиционирования секции данных после open, не системные вызовы read и не измерение времени). Существующий row_bytes_read сохраняется. Результаты целиком возвращаются только после успешной проверки всех выбранных строк. Не является онлайн-индексом, benchmark или доказательством устойчивости к физическому обесточиванию.

Проверка: `SEDRAVASQL_NATIVE_BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview" python3 -B scripts/verify_native_index_b11.py` после сборки, Rust tests и регрессии B9/B10. Измеряется число явных seek, а не ускорение в разах.

VDS: 38 native Rust tests, 21 SQLite Rust tests, strict Clippy, B9/B10/B11 CLI PASS. B11 fixture creates 256 rows, reads all of them with row_seeks=1, checks limited/empty subranges and unchanged SHA-256 of log/snapshot/index. Additional Rust test interleaves ranges and point reads on one handle.
