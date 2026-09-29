# ADR-037: уборка незавершённых snapshots

Статус: принят для локального среза фазы 5, 2026-09-29.

## Контекст

Падение publisher до activation оставляет `UPLOADING`, `STORED` или `VERIFIED`
metadata и иногда объект. Такие записи reader игнорирует, но объект занимает
место. Publisher может ещё работать, когда уборка выбирает старую запись;
одно только удаление объекта без fencing допускает позднюю запись или
активацию по уже удалённому ключу.

## Решение

Раз в час фоновый worker выбирает не более 100 записей, чей `created_at`
старше `PXEL_SNAPSHOT_CLEANUP_HOURS` (по умолчанию 24 часа). PostgreSQL
переводит незавершённые записи в `REJECTED` через `FOR UPDATE SKIP LOCKED`.
Publisher проверяет `rows_affected = 1` на переходах в `STORED`, `VERIFIED`
и `ACTIVE`; проигравший уборке publisher не меняет preferred pointer и
удаляет поздно записанный объект там, где это возможно.

`REJECTED` metadata сохраняется. GC повторно вызывает идемпотентный
`BlobStore::delete` для её object key. `cleanup_attempted_at` сортирует
кандидатов так, чтобы ограниченная пачка обходила все записи и поздний
объект снова попадал в уборку. Эта дата означает попытку, а не доказательство
того, что объект больше никогда не появится. Ошибка удаления оставляет
metadata доступной для следующей попытки. `ACTIVE` snapshots и operation
log этим worker никогда не изменяются.

## Последствия и границы

Уборка не удаляет metadata и не выводит старые `ACTIVE` поколения: они нужны
для fallback и исторического чтения. Это не полный lifecycle/retention GC.
Нет списка объектов без metadata; штатный publisher всегда создаёт metadata
до `put`. Для S3 adapter нужен тот же контракт повторного delete и отдельная
проверка eventual consistency. Перед удалением старых `ACTIVE` поколений
потребуются политика retention и проверка пригодности новых образов.

## Проверка

SQLx-тест на PostgreSQL/LocalFilesystemBlobStore проверяет age cutoff,
bounded batches, `REJECTED` и `ACTIVE`, повторное удаление позднего объекта
и publisher, задержанный перед `put`: после уборки он получает typed
`PublicationRetired`, объект отсутствует, preferred snapshot не меняется.
