# ADR-035: фоновое создание snapshots из committed actor state

Статус: принят для локального среза фазы 5, 2026-09-29.

## Контекст

Проверенное чтение snapshot и startup actor уже работают, но без создания
новых образов рабочий документ всегда остаётся на полном replay. Создание
snapshot не должно задерживать ACK и не может использовать неподтверждённую
projection.

## Решение

По умолчанию после каждого 1000-го пользовательского или системного события, считая
genesis v1, actor передаёт `Arc<Workbook>` в неблокирующий callback только
после успешного commit и candidate swap. У callback bounded channel ёмкостью
один; отдельный последовательный worker вызывает `publish_snapshot`.
Кодирование образа выполняется в `spawn_blocking`, затем LocalFilesystemBlobStore
и PostgreSQL проводят проверенный lifecycle ADR-033. Idempotent retry не
создаёт новую задачу. Неудача очереди/публикации не меняет committed head,
не превращается в ACK ошибки и не удаляет journal.
`PXEL_SNAPSHOT_INTERVAL` задаёт порог в пределах `1..100000`; значение 1
используется в локальном smoke, штатное значение — 1000.

## Последствия

Очередь может пропустить milestone при высокой нагрузке: следующий шанс
через 1000 событий. Это допустимо для ускоряющего cache, но требует
метрики, ручного recalculation/snapshot trigger и повторной политики до
production gate. Один процесс допускает одного writer worker; несколько
серверов могут создать разные snapshots одной версии, при этом preferred
pointer остаётся на максимальной версии. Таймауты/отмена blocking encoding,
GC и S3 contract требуют отдельных проверок. Максимальный binary snapshot
сейчас 32 MiB; превышение оставляет журнал читаемым, но не активирует образ.

## Проверка

PostgreSQL/actor тест проверяет enqueue версии 3 только после durable commit и
отсутствие второго enqueue на idempotent retry. Локальный server smoke с
порогом 1 подтвердил `ACTIVE` snapshot v2 после одной операции и чтение
документа после hard restart. Crash matrix, 10k-tail benchmark и S3
проверяются отдельно.
