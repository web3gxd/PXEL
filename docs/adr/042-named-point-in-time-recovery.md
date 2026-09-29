# ADR-042: локальный PITR до именованной точки между операциями

Статус: принят для локального recovery среза фазы 5, 2026-09-29.

## Контекст

ADR-041 проверил восстановление из согласованной копии при остановленном
backend, но не проверил replay WAL после base backup до выбранной версии.
Для recovery важна способность исключить более позднюю подтвержденную
операцию, если оператор выбирает более раннюю точку.

## Решение

`just pitr-restore-postgres` поднимает отдельный PostgreSQL 18 с WAL archive.
После ACTIVE snapshot v3 backend останавливается, объект копируется, а
`pg_basebackup` и `pg_verifybackup` создают и проверяют base backup. Backend
снова запускается: команда v4 получает ACK, `pg_create_restore_point`
записывает маркер, затем v5 тоже получает ACK. `pg_switch_wal` делает сегмент
с маркером доступным в archive.

В новом кластере `recovery.signal`, `restore_command`,
`recovery_target_name = 'pxel_v4'` и `recovery_target_action = 'promote'`
останавливают replay у маркера. Harness дожидается выхода из read-only
recovery перед запуском backend. Проверяются head v4, hash v4 из ACK,
содержимое, историческая v3 и runtime загрузка snapshot v3 + одного
события. Committed v5 в восстановленный head не входит.

Архивные команды `copy`/`cp` здесь нужны только для приватного локального
стенда. Они не являются production archiver: нет защищенного off-site
хранилища, мониторинга задержки архива, политики retention и проверки
восстановления после потери отдельных сегментов. Online backup snapshot
объектов при активных writers и RPO/RTO остаются отдельными gates.

PostgreSQL описывает `recovery.signal`, `restore_command` и named restore
point в [PITR guide](https://www.postgresql.org/docs/18/continuous-archiving.html).
