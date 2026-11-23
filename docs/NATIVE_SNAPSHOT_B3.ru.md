# SedravaSQL B3 — экспериментальный материализованный snapshot

Дата: 23 ноября 2026. Только исследовательский этап B3; не production.

## Возможности

Native CLI получил две отдельные команды:

- snapshot-create SOURCE NEW_DEST EXPECTED_SOURCE_SHA256: проверяет
  целостность незанятого журнала SEDRLOG1, полный SHA-256 из ранее
  утверждённого плана, затем создаёт новый файл снимка и проверяет его.
- snapshot-verify SNAPSHOT: проверяет отдельный файл по magic, версии,
  длине, контрольной сумме и внутренней структуре.

Формат: ASCII SEDRSNP1 (8 байт), длина JSON (4 байта big-endian),
JSON, SHA-256 предыдущих байтов (32 байта). Версия payload:
sedravasql.snapshot.b3-preview.v1 (нестабильная).

Содержит отсортированные записи (включая tombstones), отсортированные
квитанции исходных запросов с их fingerprint, последний sequence,
SHA-256 и размер журнала и SHA-256 его последней цепочки. Одинаковый
исходник даёт побайтово одинаковый snapshot без временных меток.

## Воспроизведение в приватной тестовой папке

    source "$HOME/.sedrava-dev/env.sh"
    sedravasql_target
    cd "$HOME/.sedrava-dev/sedravasql"
    cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview
    BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview"
    "$BIN" backup-plan ./private/test.slog
    "$BIN" snapshot-create ./private/test.slog ./private/test.snap FULL_APPROVED_SHA256
    "$BIN" snapshot-verify ./private/test.snap

Писатель должен быть остановлен. Источник и назначение должны
находиться в приватных каталогах; output должен отсутствовать.

## Пройденные испытания

5 Rust тестов и CLI test: 8 коммитов, 2 ключа, сохранение
материализованных записей и оригинальных квитанций, повторяемость,
сверка хешей, отказ при lock, повреждении, неверном операторском SHA,
попытке перезаписи, symlink/hardlink, публичной директории.
Старые B0/B1/B2 проверки продолжают проходить. Лимиты: 64 MiB
исходный журнал и 64 MiB snapshot. Не затрагивается SQLite API.

## Нереализованное (NO-GO)

- Snapshot НЕ открывается как writable native журнал и не участвует
  в открытии NativeLog. Нет команды восстановления из snapshot.
- Нет checkpoint, compaction, ускоренного старта, независимого
  дискового индекса, онлайн-snapshot и переключения рабочей базы.
- Отдельный snapshot-verify не подтверждает актуальность снимка
  относительно текущей версии журнала.
- SHA-256 не аутентифицирует злонамеренную полную подмену.
- Нет тестов аппаратной потери питания, независимого носителя,
  защиты от злонамеренного same-UID писателя и production-ready API.

Следующий возможный этап B4 — версионированный checkpoint и
операторский restore в новый журнал с отдельным failure-injection
и contract testing. Пока это только план.
