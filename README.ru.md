# SedravaSQL — 0.1.0-alpha.1

**Экспериментальная встраиваемая база данных на Rust и SQLite.**

[English](README.md) · [Подробный контракт](docs/IMPLEMENTED_V0_1.ru.md)

> Исполняемый прототип реализован, проверен на Ubuntu Linux x86_64.
> **Не используйте для важных или производственных данных.**
> Это ограниченный интерфейс ключ/значение, **не** полноценный
> SQL-сервер или замена PostgreSQL.

## Сборка и проверка

Требуются Rust 1.85+, Cargo и C-компилятор
(для сборки SQLite в составе проекта).

```sh
git clone https://github.com/VasiliiBabkin/sedravasql.git
cd sedravasql
cargo build --locked
cargo test --locked --all-targets
python3 scripts/verify_local.py
```

Сам бинарник не требует Python. Независимый Python-скрипт проверяет
его как отдельный процесс в изолированных временных каталогах.
Аварийные тесты работают только в debug, не в release.

## Минимальный пример

```sh
mkdir -m 700 -p /tmp/sedravasql-demo
DB=/tmp/sedravasql-demo/data.sqlite
BIN=./target/debug/sedravasql
"$BIN" init "$DB"
"$BIN" plan-put "$DB" demo greeting "Привет"
"$BIN" put "$DB" demo greeting "Привет" \
  --expect 0 --request-key example-request-0001
"$BIN" get "$DB" demo greeting
"$BIN" audit "$DB" 10
"$BIN" verify "$DB"
"$BIN" backup "$DB" /tmp/sedravasql-demo/backup.sqlite
```

Второй идентичный вызов `put` с тем же ключом возвращает
`"replayed": true`, а устаревшая ревизия для нового запроса
отклоняется. Удаление требует `plan-delete`, потом `delete`
с `--confirm`, ревизией и новым ключом запроса.

## Статус возможностей

| Возможность | Статус |
| --- | --- |
| SQLite, WAL, synchronous=FULL | Реализовано |
| Типизированный API ключ/значение, параметризованный CRUD | Реализовано |
| Preview + проверка ревизии (CAS) + подтверждение удаления | Реализовано |
| Идемпотентные квитанции + аудит в одной транзакции | Проверено на VDS |
| Конкурентные операции нескольких процессов | Проверено на VDS |
| Резервная копия через SQLite Backup API | Проверено на VDS |
| Неизвестные схемы и версии | Отклоняются, автоматической миграции нет |
| Произвольные SQL-запросы, свои таблицы, SQL-сервер | Не реализовано |
| Проверенное восстановление после потери питания | Не подтверждено |
| Шифрование, ACL, репликация и production | Не реализовано |

Архитектура и [дорожная карта](docs/ROADMAP.md) ·
[проверка фактов](docs/CLAIMS.md) ·
[отчёт об испытаниях](docs/VERIFICATION.md).

Проект независим от языка программирования Sedrava.
Лицензия: [Apache-2.0](LICENSE).

**Посвящается памяти Дмитрия Грабыльникова.**
[Посвящение](DEDICATION.md).
