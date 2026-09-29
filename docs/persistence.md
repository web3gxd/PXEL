# Persistence и recovery

Статус: migrations/readiness, LocalFilesystemBlobStore и durable document
operations реализованы локально. `SnapshotImage` и binary codec проверены.
ADR-033 добавил PostgreSQL snapshot metadata и внутренние publish/read API:
локальный объект проходит повторное чтение перед `ACTIVE`, reader выбирает
предыдущее поколение при повреждении нового. Actor startup, scheduler,
tail replay wiring, GC и S3 contract ещё не подключены. Ниже отдельно
названы действующие механизмы и будущие требования.

## PostgreSQL authority

Метаданные tenant/users/memberships/documents/ACL и append-only operation log живут в PostgreSQL. Workbook не сохраняется единым JSON. Tables появляются с use cases; import/export jobs и snapshot metadata не создают видимость работающих функций до соответствующей фазы.

Document ACL grant/revoke хранится отдельно от authored workbook log:
`documents.access_epoch` и `document_permissions` меняются в одной
транзакции с append-only `audit_events` schema 1. Повтор `change_id` дает
оригинальный receipt; новый intent с тем же ID отклоняется. ACL изменение не
расходует document version и не попадает в calculation dependencies.

Event envelope имеет payload schema version, document_id, operation_id, committed_version, authenticated principal, command fingerprint, canonical event bytes/checksum и prepared state hash. Уникальны `(document_id,committed_version)` и `(document_id,operation_id)`. UUIDv7 генерируется с глобальной уникальностью; partition-friendly DB constraint scoped к документу. Один batch получает одну версию.

Persistence schema отделена от wire schema. Byte checksum защищает точные сохраненные protobuf bytes; он не является каноническим хэшем состояния. Старые события не переписываются ради нового schema version: upcast при чтении или новый snapshot. Неизвестная schema останавливает replay контролируемой ошибкой.

## CAS transaction

Candidate и его hash готовятся до transaction. Transaction проверяет auth epochs/ACL, receipt, затем выполняет условное изменение head:

```sql
UPDATE documents
SET head_version = head_version + 1
WHERE document_id = $1 AND head_version = $2
RETURNING head_version;
```

Вернувшаяся версия используется для INSERT immutable event и idempotency record в той же transaction; failure любого statement откатывает head. DB constraint исключает вторую запись этой версии. Проверяется переполнение счетчика до SQL. SQLx использует checked queries и `.sqlx` metadata; CI `prepare --check` на реальной мигрированной БД дополняет offline build.

CAS zero rows, serialization failure или deadlock: rollback → reload tail/head/ACL → re-run command validation/rebase/binding/candidate → новая transaction. Нельзя повторить только UPDATE/INSERT, сохранив решение от старого состояния. Retry bounded; exhaustion → typed retryable conflict. Для повторного operation_id конкурентные транзакции сходятся к сохраненному receipt, а payload mismatch дает collision.

COMMIT подтверждается только при `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`. Эти настройки включаются в environment/benchmark evidence. Transaction timeout не доказывает rollback, когда ответ COMMIT потерян. Actor проверяет operation_id через новый connection и при необходимости приостанавливает документ, прежде чем выбирать дальнейшую head. Client повторяет исходный ID.

После commit projection публикуется prepared state swap. Crash между COMMIT и swap/ACK не теряет событие. Hash mismatch/internal invariant error запрещает новые команды и переводит документ в quarantined. Восстановление должно завершиться совпадением expected hashes прежде, чем снять карантин.

Источник по complete retry: [PostgreSQL serialization failure handling](https://www.postgresql.org/docs/18/mvcc-serialization-failure-handling.html).

## BlobStore

Application BlobStore port предоставляет put/get/head/delete с typed BlobKey, checksum и metadata. LocalFilesystemBlobStore — единственный adapter фазы 0. Boundaries описаны через bounded data/stream abstractions по текущему use case; отсутствие blob consumers не оправдывает неограниченный upload endpoint.

Local keys не являются произвольными путями: допускаются только проверенные относительные opaque names, запрещены traversal/absolute paths/drive prefixes/symlink escape. Root принадлежит сервису, создаваемые файлы private. Put проверяет checksum, пишет temporary object в том же filesystem, flush/sync и атомарно публикует; читатель видит полный old/new object, не partial file. Metadata/head соответствует полному объекту. Delete idempotent. Crash durability конкретного filesystem/ОС не приравнивается без тестов к подтвержденной durable snapshot activation.

Contract suite проверяет binary round-trip, head size/checksum, checksum mismatch без опубликованного object, missing key, rejected unsafe key, atomic publication и delete. Phase 0 исполняет suite локально; S3 compatibility пока не заявляется. Phase 5 тот же suite проверяет S3BlobStore против Garage 2.4.0 candidate и выбранного российского S3.

S3 MVP subset: PutObject/GetObject/HeadObject/DeleteObject, позднее multipart. Наличие S3-compatible label не гарантирует контракт: [Garage API matrix](https://garagehq.deuxfleurs.fr/documentation/reference-manual/s3-compatibility/). MinIO и Garage не входят в phase 0 Compose. Никаких bucket/credentials без реального потребителя.

## Snapshot activation (фаза 5)

ADR-031 реализует внутренний образ и восстановление в domain crate.
Он сохраняет tombstones, bound formulas, calculation context и последнюю
версию изменения осей. ADR-032 добавляет отдельный бинарный codec. ADR-033
реализует metadata state machine и внутренний reader; runtime actor пока не
выбирает снимок при запуске.

Snapshot имеет immutable document version, schema version, engine/profile/context, domain encoding version, length, byte checksum и content hash. Scheduler берет согласованный immutable candidate на V, не читает изменяемую книгу во время записи.

State machine:

```text
UPLOADING -> STORED -> VERIFIED -> ACTIVE
     \         \          \-> REJECTED
      +---------+-----------> REJECTED
```

1. Создать record UPLOADING с unique immutable object key и intended document version.
2. Загрузить объект в staging namespace по этому ключу. Rename в S3 не требуется: после activation тот же key становится опубликованным через metadata, а объект не изменяется.
3. Проверить storage size/checksum, сохранить STORED. Ошибку явно классифицировать, partial object не публиковать.
4. Скачать/stream-verify bytes, декодировать с caps/schema checks, восстановить projection и сверить content hash. Простой HEAD не заменяет чтение/проверку.
5. Перевести в VERIFIED с validation metadata.
6. Короткая DB transaction переводит record в ACTIVE и выбирает preferred snapshot pointer. Более старый поздно завершившийся job не понижает preferred version. Неизвестный/неверный engine/profile context не активируется молча.
7. Предыдущие ACTIVE generations остаются доступными для fallback. ACTIVE означает «прошел activation», а preferred pointer указывает рекомендуемое новейшее поколение. Это не единственная строка ACTIVE на документ.
8. GC отдельно удаляет неиспользуемые orphan/incomplete/rejected/retired objects после retention, проверяя metadata и leases незавершенных jobs. Старый ACTIVE нельзя удалять до верификации и восстановления более нового поколения. Retention не означает удаление log живого документа.

Crash после object upload, до metadata update оставляет незавершенный record/object: recovery reader его не использует, будущий reconciler/GC решит судьбу. Crash после activation transaction дает читателю ACTIVE record. Corrupt ACTIVE при загрузке сейчас пропускается без изменения metadata и выбирается предыдущий ACTIVE; автоматический аудируемый `REJECTED` переход будет отдельным срезом. Object existence alone никогда не активирует snapshot.

## Replay и hashes

Reader авторизует historical read, выбирает подходящий ACTIVE snapshot <=requested version, проверяет envelope/checksum/hash и последовательно применяет log tail. Versions должны быть непрерывны. Unknown event/schema, gap и checksum mismatch не пропускаются. Replay применяет сохраненные canonical operations, не повторяет сортировку/генерацию IDs или random source.

Cacheless content hash использует versioned domain encoding: fixed tags/endianness/lengths, ordered axes, sorted maps, normalized IEEE-754 bits. Runtime caches, pending operations, presence и protobuf field order исключены. Context identity/epoch передаются отдельно; result hash включает engine/profile/context/input_version и нормализованные результаты. Comparison проводится только при одинаковых версиях и завершенном calculation generation.

[Protobuf serialization is not canonical](https://protobuf.dev/programming-guides/serialization-not-canonical/) — причина отдельной encoding, а не разрешение нестабильного domain hash.

## Retention и эксплуатация

По умолчанию durable log живого документа не сокращается. Удаление tenant/document и backups — отдельная авторизованная retention policy. Future archival обязана сохранять обещанную historical/rebase доступность либо явно сужать ее границу. Snapshot — ускорение восстановления, не replacement log.

Local COMMIT не обещает пережить полную потерю диска. Production gate потребует backup/PITR, restore drill, replication/RPO/RTO policy и secret management. Snapshot workers и future XLSX process не получают больше доступа, чем требуется задаче. Operation payload, preimages, formulas и blob content не попадают в обычные логи.
