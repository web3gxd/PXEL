# ADR-041: локальный restore drill для PostgreSQL и snapshot объектов

Статус: принят для изолированного среза фазы 5, 2026-09-29.

## Контекст

Snapshot ускоряет replay, но PostgreSQL metadata и объект должны попасть в
восстановление согласованной парой. Успешный `pg_basebackup` сам по себе не
проверяет BlobStore; успешный checksum snapshot не проверяет восстановление
журнала и ACL в новом PostgreSQL.

## Решение

`just backup-restore-postgres` запускает отдельный PostgreSQL 18 с durable
настройками, публикует ACTIVE snapshot v3 и подтверждает v4. Backend
останавливается, чтобы snapshot worker не менял объектное хранилище. Тест
копирует локальный blob-каталог и выполняет физический `pg_basebackup` с
потоковой передачей WAL. Затем `pg_verifybackup` проверяет manifest и WAL.
Исходная книга после backup доходит до v5; из резервной пары поднимается новый
кластер и backend. Он должен восстановить ровно v4, совпасть с hash v4 из ACK,
прочитать историческую v3 и сообщить snapshot v3 + один event хвоста.

Этот порядок специально quiescent. Он не объявляется алгоритмом online
согласования S3 и БД. Production backup требует отдельного manifest с
версиями/ключами объектов, fencing/retention для активных writers, регулярного
restore drill и защищенного хранения копий. WAL archiving/PITR и off-site
копирование проверяются отдельно.

## Проверка

Локальный тест на PostgreSQL 18.6 проходит оба шага: проверку backup manifest
и реальный запуск нового кластера. Рецепт использует приватные каталоги под
`.local`, не трогает рабочую БД. Дополнительные гарантии инструмента и пределы
`pg_verifybackup` описаны в [документации PostgreSQL 18](https://www.postgresql.org/docs/18/app-pgverifybackup.html).
