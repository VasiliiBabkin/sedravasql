# Native B10 — ограниченные диапазонные чтения

Экспериментальный офлайн-индекс SEDRIDX1, формат B8 не меняется.

`VerifiedIndexReader::range(namespace, start, end, limit)` возвращает записи одного пространства имён с ключами в полуоткрытом диапазоне [start, end), в лексикографическом порядке. Лимит 1..256; tombstones возвращаются с value=null. Пустой диапазон допустим при start < end, неверные границы отклоняются. Начальная позиция определяется бинарным поиском; каждая выбранная строка читается и проверяется через B9 get. Сложность каталожного поиска O(log N + K log N), без учёта открытия/дискового I/O. Первичное открытие по-прежнему проверяет весь файл. Метаданные, приватность и идентичность проверяются до и после диапазона, включая пустой результат. Не предназначено для конкурирующего писателя или недоверенного UID.

CLI: `sedravasql-native-preview index-range INDEX NAMESPACE START END LIMIT`.

VDS: 37 native Rust tests, 21 SQLite Rust tests, strict Clippy, B9 CLI and independent B10 CLI PASS. B10 CLI checks namespace isolation, half-open bounds, tombstones, order, limits, invalid arguments, zero-row byte count, and SHA preservation of all three source files. No physical power-loss or production claim.

Reproduce: source ~/.sedrava-dev/env.sh; sedravasql_target; cargo test --manifest-path native-engine/Cargo.toml --locked --offline --all-targets; cargo clippy --manifest-path native-engine/Cargo.toml --locked --offline --all-targets -- -D warnings; cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview; SEDRAVASQL_NATIVE_BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview" python3 -B scripts/verify_native_index_b10.py.
