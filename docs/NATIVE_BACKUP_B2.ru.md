# SedravaSQL B2 — проверяемая офлайн-копия native-журнала

Дата: 23 ноября 2026. Статус: эксперимент, **не production**.

## Назначение

NativeLog (SQLite-free) теперь поддерживает две дополнительные команды:

- backup-plan SOURCE — проверяет полный исправный журнал SEDRLOG1, последовательность транзакций и SHA-256, возвращает source_sha256, source_bytes и sequence.
- backup-copy SOURCE NEW_DEST EXPECTED_SOURCE_SHA256 — повторно проверяет журнал и его хеш, создаёт **новый** приватный файл 0600, синхронизирует файл и каталог, проверяет полученную копию и состояние всех транзакций.

Это отличается от B1 recovery-copy: B1 допускает отрезание оборванного последнего кадра только в новой копии и по решению оператора. B2 **не принимает** оборванный или повреждённый исходник. Ни одна из команд не изменяет исходный журнал, не удаляет lock и не затрагивает SQLite API.

## Воспроизведение (тестовый источник, писатель остановлен)

    source "$HOME/.sedrava-dev/env.sh"
    sedravasql_target
    cd "$HOME/.sedrava-dev/sedravasql"
    cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview
    BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview"
    "$BIN" backup-plan ./private/data.slog
    "$BIN" backup-copy ./private/data.slog ./private/data-backup.slog FULL_SOURCE_SHA256_FROM_PLAN
    "$BIN" verify ./private/data-backup.slog

Оператор обязан убедиться, что нет текущего писателя, lock или
состязательной записи, и что назначение ещё не существует.

## Границы

- Только Unix preview, приватный каталог 0700, обычный исходный файл
  0600 с ровно одной жёсткой ссылкой. Symlink/hardlink/public mode
  и занятый журнал отклоняются.
- Максимум 64 MiB исходного журнала.
- Предварительное подтверждение полной SHA-256. Повторная проверка
  перед копированием, после копирования и проверка содержимого
  новой копии путём replay всех транзакций.
- Назначение создаётся только с create_new, без overwrite.
  При сбое удаляется только созданный файл при проверке его inode.
- Нет гарантий против злонамеренного писателя того же UID,
  игнорирующего lock; SHA-256 не аутентифицирует злонамеренную подмену.
- Нет online backup, snapshot, checkpoint, compaction, миграций,
  автоматического restore, тестов физического отключения питания,
  или механизма резервирования на другом носителе.

## Проверки

    cargo test --manifest-path native-engine/Cargo.toml --locked --offline --all-targets
    cargo clippy --manifest-path native-engine/Cargo.toml --locked --offline --all-targets -- -D warnings
    SEDRAVASQL_NATIVE_BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview" python3 scripts/verify_native_backup_b2.py

5 новых Rust-тестов и отдельный CLI тест проверяют два ключа,
8 коммитов, сохранение истории replay, точную побайтовую копию,
неверный SHA, устаревший план, lock, оборванный журнал, битый
checksum, symlink/hardlink, права и ограничение объёма. Проверки
B1 recovery и NativeLog batch также пройдены.

**Нельзя использовать для ценных данных.** Следующий этап:
версионированные snapshot/checkpoint, restore matrix, fault-injection
и внешняя политика хранения копий.
