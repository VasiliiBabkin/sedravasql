# SedravaSQL — B4 controlled native restore from snapshot

Date: 2026-11-23. Research-only, manual offline restore. NO production claim.

## Что реализовано

Новая команда экспериментального Unix CLI:

    sedravasql-native-preview restore-copy SNAPSHOT NEW_DEST APPROVED_SNAPSHOT_SHA256

Вход — корректный, закрытый SEDRSNP1 (B3). Оператор обязан
предварительно выполнить snapshot-verify, проверить 64-значную SHA-256
и отдельно подтвердить назначение. Программа не удаляет
существующие файлы, не изменяет snapshot, журналы или блокировки.

В B3 snapshot записаны materialized entries, tombstones, последовательность
и все квитанции request-key с результатами. B4 реконструирует
исторические операции по их сохранённым результатам, восстанавливает
порядок транзакций, CAS-ревизии, idempotency fingerprint,
каждый кадр и SHA-256 цепочку оригинального SEDRLOG1.

**Ключевой факт:** перед созданием нового журнала сверяются полная
SHA-256 исходного журнала, размер, последняя chain SHA-256, итоговое
состояние и сохранённые результаты всех квитанций. Если snapshot
не содержит достаточных данных или не совпадает хотя бы один факт,
отказ без создания назначения. Формат SEDRSNP1 не менялся.

## Проверка (только отдельные учебные файлы)

    source "$HOME/.sedrava-dev/env.sh"
    sedravasql_target
    cd "$HOME/.sedrava-dev/sedravasql"
    cargo build --manifest-path native-engine/Cargo.toml --locked --offline --bin sedravasql-native-preview
    BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview"

    "$BIN" snapshot-verify ./private/test.snap
    # Сверьте и сохраните snapshot_sha256 из полученного отчёта.
    "$BIN" restore-copy ./private/test.snap ./private/restored-new.slog FULL_SNAPSHOT_SHA256
    "$BIN" verify ./private/restored-new.slog

В новый файл записывается **тот же исторический журнал** SEDRLOG1,
а не сокращённый checkpoint. В тестах его байты совпадают с
оригинальным журналом, в том числе после DELETE и replay старого
request-key. После открытия нового файла следующие транзакции
могут продолжаться с существующих ревизий.

## Защитные границы

- Только экспериментальный Unix, приватный каталог 0700, исходный
  snapshot — обычный файл 0600 с единственным hardlink.
- Нельзя восстановить поверх исходного, существующего destination,
  symlink или файла чужого формата. create_new + синхронизация
  файла и каталога; ошибка удаляет только созданный файл
  при совпадении inode.
- Лимит snapshot и восстанавливаемого журнала: 64 MiB.
- Пакеты реконструируются только при корректном порядке seq,
  полном соответствии результатам, batch fingerprint, chain SHA
  и итоговой исходной SHA. Порча и недостоверные квитанции
  отвергаются до открытия назначения.
- Нет поддержки злонамеренной гонки процесса того же Unix UID,
  сговора с пересчётом всех SHA или аппаратных гарантий диска.
  SHA-256 — контроль целостности, не авторизация доверенного владельца.
- Snapshot должен быть доверенным и созданным из корректного
  B3-процесса; сам B4 не проверяет удалённый первоисточник.
- **Нет** автоматического выбора свежего snapshot, изменения
  боевой базы, live restore, атомарного переключения работающего
  писателя, асинхронного checkpoint, compaction, уменьшения журнала,
  отдельного B+tree индекса, кроссплатформенных гарантий и
  проверки реального отключения питания.

## Воспроизводимые тесты

    cargo test --manifest-path native-engine/Cargo.toml --locked --offline --all-targets
    cargo clippy --manifest-path native-engine/Cargo.toml --locked --offline --all-targets -- -D warnings
    SEDRAVASQL_NATIVE_BIN="$CARGO_TARGET_DIR/debug/sedravasql-native-preview" python3 -B scripts/verify_native_restore_b4.py

5 новых Rust-тестов: пустой журнал, 10 транзакций с двумя ключами и
tombstone, точное SHA совпадение, сохранение replay/ключей, CAS после
восстановления, запрет overwrite/symlink/hardlink/public output,
фальшивые квитанции с **пересчитанной** внешней SHA, отказ при
нулевой ревизии, ненастоящей source SHA и повреждении snapshot.
Отдельный CLI тест: 8 транзакций/2 ключа, побайтовое равенство,
повторные запросы и новая транзакция после восстановления.
Предыдущие B1/B2/B3 и SQLite тесты остаются обязательными.

## Следующий этап B5

Надёжная crash/fault-injection матрица с остановками на разных
точках записи восстановления, версия и политика хранения снимков,
строгие bounds на историю и индекс хранения. Никакой production
рекомендации до проверки power-loss и модели угроз.
