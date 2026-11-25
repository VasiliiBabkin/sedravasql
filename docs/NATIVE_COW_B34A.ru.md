# B34a — независимый образ миграции NativeLog → COW

Исследование от B33b 7c0c2cb. Новый образ предназначен для сохранения и сверки, не для исполнения SQL. Исходный SEDRLOG1 остаётся рабочим источником; SQLite не мигрируется.

`migrate_copy` захватывает эксклюзивный NativeLog lock, проверяет журнал с диска, сохраняет все записи Replay (включая каталоги SQL, индексов, FK, SQL receipt rows и tombstones) с namespace/key/value/revision; все исторические NativeCommit с request key и оригинальным fingerprint, результатом и sequence. Хеш пары namespace/key разделяет адреса; префиксы entry/receipt различны. JSON-объекты разбиваются по UTF-8 границам на фрагменты ≤48 KiB, проверяемые по длине, SHA-256 и ревизии. Ключи сортируются, повторы отклоняются. Источник проверяется повторно перед публикацией. Независимая COW-пара data/head создаётся без перезаписи; новый packed snapshot проверяется целиком до возврата. Manifest хранит исходный sequence, terminal chain hash, counts; для пустого источника физическое поколение 1 (исходный sequence 0). Лимиты: 32 MiB суммарного сериализованного payload, 32768 физических записей, MAX_PAGES; большие журналы могут быть отклонены до записи назначения. В случае I/O или аварии неполное назначение требует операторского разбора. Удаление источника и автоматическое переключение отсутствуют.

`MigrationImage` — read-only interface: entry, receipt, replay (проверяет оригинальный fingerprint и возвращает NativeCommit без повторной операции), matches_source (сравнивает полный manifest и каждый физический record с новой проверенной копией исходного Replay). Хешированная адресация не обеспечивает SQL range scans; нужен новый backend contract. Квитанции исторического NativeLog хранятся как объекты миграции, не смешиваются с B33b generation-only receipts. Проверки обнаруживают повреждение, не аутентифицируют недоверенного владельца файлов.

Команды (Linux, приватный каталог 0700):
    sedravasql-native-preview migration-copy SOURCE NEW_IMAGE
    sedravasql-native-preview migration-info IMAGE
    sedravasql-native-preview migration-read IMAGE NAMESPACE KEY
    sedravasql-native-preview migration-receipt IMAGE REQUEST_KEY
    sedravasql-native-preview migration-match-log IMAGE SOURCE

4 целевых теста PASS: полные 64 KiB Unicode и JSON-escaped значения в исторических multi-part receipts, namespace separation, tombstones, revisions, SQL catalog и SQL rows, replay/conflict, сверка с оригиналом и отклонение изменившегося источника, пустая база, существующее назначение, пересечение файловых семейств, повреждённый журнал и пять CLI-команд. Strict Clippy PASS. Полная регрессия ведётся отдельно. [Логи](../evidence/benchmark/b34a-migration-2026-11-25/).

Далее: B34b/B35 — ordered address mapping, writable adapter, атомарное сохранение исходного SQL receipt contract и conformance, не переключать основной backend до проверки.
