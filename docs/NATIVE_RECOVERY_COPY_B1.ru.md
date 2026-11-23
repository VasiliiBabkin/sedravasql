# SedravaSQL — экспериментальное восстановление native-журнала B1

Дата: 23 ноября 2026. Статус: **копирование проверенного префикса**, не автоматическое восстановление и не production.

## Назначение

NativeLog — отдельный SQLite-free движок с журналом SEDRLOG1 и одним писателем. При аварийном завершении посреди последней записи обычное открытие отказывает. Инструмент B1 исследует обрыв без изменения исходного журнала.

Две явные команды экспериментального CLI:

- recovery-plan SOURCE — только чтение; проверяет формат, полные кадры и цепочку SHA-256; возвращает SHA-256 оригинала, длину проверенного префикса, размер оборванного хвоста и число подтверждённых транзакций.
- recovery-copy SOURCE NEW_DEST EXACT_SOURCE_SHA256 — снова проверяет оригинал и полную хеш-сумму из ранее полученного плана, создаёт новый приватный файл только с подтверждёнными транзакциями, синхронизирует его и заново проверяет восстановленный журнал.

**Ни одна команда не удаляет lock-файл и не обрезает журнал на месте.** Оператор должен убедиться, что писатель действительно завершился, до ручного удаления stale lock. Удалять lock работающего процесса запрещено.

## Пример на тестовом файле

Сначала активировать окружение VDS:

    source "$HOME/.sedrava-dev/env.sh"
    sedravasql_target
    cd "$HOME/.sedrava-dev/sedravasql"
    cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview

После независимого подтверждения, что нет писателя и lock:

    "$CARGO_TARGET_DIR/debug/sedravasql-native-preview" recovery-plan ./private/source.slog

Затем использовать полный реальный SHA-256 из плана:

    "$CARGO_TARGET_DIR/debug/sedravasql-native-preview" recovery-copy ./private/source.slog ./private/recovered.slog FULL_SOURCE_SHA256_FROM_PLAN
    "$CARGO_TARGET_DIR/debug/sedravasql-native-preview" verify ./private/recovered.slog

Каталог private требуется 0700, выходной файл должен отсутствовать. Это демонстрация API, а не разрешение восстанавливать боевую базу.

## Границы безопасности

- Копируется только подтверждённая часть до **неполного последнего** кадра. Если полностью присутствующий кадр имеет неправильную контрольную сумму, структуру, последовательность или операцию — отказ без копирования.
- Занятый lock, чужой формат, существующее назначение, symlink/hardlink, публичные права или несовпадение SHA — отказ.
- Исходный журнал сохраняется неизменным. Неподтверждённый последний кадр не переносится.
- Экспериментальное ограничение исходного файла — 64 MiB.
- Контроль SHA-256 не аутентифицирован и не защищает от злонамеренной полной подмены. Некорректный писатель, игнорирующий lock, также не предотвращается.
- Нет автоматического recovery, безопасного использования в production, тестов диска/контроллера, физического power-loss, политики онлайн-резервирования и отката.

## Проверки

    cargo test --manifest-path native-engine/Cargo.toml --locked --offline --all-targets
    cargo test --locked --offline --all-targets
    cargo clippy --manifest-path native-engine/Cargo.toml --locked --offline --all-targets -- -D warnings
    SEDRAVASQL_NATIVE_BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview" python3 scripts/verify_native_recovery.py

4 дополнительных Rust-теста проверяют обрыв длины, payload и checksum, отказы для битой полной записи, запрет overwrite/lock, точные SHA и сохранение старых квитанций.

CLI проверен в самостоятельных дочерних процессах: авария после записи длины даёт возможность отдельного recovery-copy, а авария после sync_all даёт обычный replay без восстановления.

**NO-GO: инструмент только для тестовых данных.** Связанный контракт: BATCH_CONTRACT_A1.ru.md.
