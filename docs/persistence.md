# Persistence и recovery

ADR-076 добавляет schema16 authored formula write: source, normalized bound AST,
exact preimage и authored_kind3. Старые schema2/11 bytes и hash tags неизменны;
новый semantic hash tag17 независим от protobuf bytes. Компенсации остаются
schema12/13. Migration расширяет principal constraint до16. Formula-bearing undo
повторяет проверку axis history внутри CAS transaction; отказ не меняет head и
не создаёт receipt. Кодек отвергает будущую binding version и при encode, и при
decode нового формата.
ADR-077 разрешает inverse own schema12/13 compensation без изменения её bytes
или hash. Транзакционный scan сокращает только собственные CellWrite inverse
пары по ID/version и отказывает при foreign/legacy/batch/copy barrier. Redo
сохраняет новую компенсацию обычным CAS/commit/receipt путём; история не удаляется.

Статус: migrations/readiness, LocalFilesystemBlobStore и durable document
operations реализованы локально. `SnapshotImage` и binary codec проверены.
ADR-033 добавил PostgreSQL snapshot metadata и внутренние publish/read/recover API:
локальный объект проходит повторное чтение перед `ACTIVE`, reader выбирает
предыдущее поколение при повреждении нового. Authenticated recovery API
дочитывает непрерывный проверенный хвост журнала после снимка и сверяет
конечную версию. ADR-034 подключил reader к actor startup и запускает полный
расчёт restored projection. ADR-035 добавил неблокирующий scheduler после
каждой 1000-й committed операции по умолчанию и отдельный sequential worker.
Локальный smoke с порогом 1 подтвердил ACTIVE v2 и чтение после restart.
ADR-037/038 добавили уборку incomplete и старых ACTIVE snapshots. ADR-039
подключил optional S3 adapter; локальный Moto contract прошёл, Garage и
российский S3 ещё не проверены. Ниже отдельно
названы действующие механизмы и будущие требования.

Controlled interruption test останавливает публикацию на шести границах
metadata/object/transaction и проверяет, что до COMMIT восстановление идёт
из журнала, после COMMIT — из ACTIVE snapshot. Power-loss и S3 crash
tests ещё не проведены; гонка с локальным GC проверена отдельно.

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

Filesystem contract suite проверяет binary round-trip, head size/checksum,
checksum mismatch без опубликованного object, missing key, rejected unsafe
key, atomic publication и delete. Отдельный S3 contract на Moto 5.2.3
проверил HTTP path-style, stream, checksum, conditional put, conflicts и
delete; server restart создал ACTIVE snapshots в S3. Этот результат не
означает совместимость Garage или российского S3. Их проверяет тот же
`just s3-contract` при доступном endpoint и credentials.

S3 MVP subset: PutObject/GetObject/HeadObject/DeleteObject, позднее multipart.
`If-None-Match: *` и сохранение пользовательских checksum metadata необходимы
для текущего контракта. Наличие S3-compatible label не гарантирует эти
семантики: [Garage API matrix](https://garagehq.deuxfleurs.fr/documentation/reference-manual/s3-compatibility/).
MinIO и Garage не входят в phase 0 Compose.

## Snapshot activation (фаза 5)

ADR-031 реализует внутренний образ и восстановление в domain crate.
Он сохраняет tombstones, bound formulas, calculation context и последнюю
версию изменения осей. ADR-032 добавляет отдельный бинарный codec. ADR-033
реализует metadata state machine и внутренний reader/recovery. ADR-034
подключает его к runtime actor при первом открытии документа.

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
8. GC отдельно удаляет неиспользуемые incomplete/rejected/retired objects после retention, проверяя metadata и fencing незавершённых publisher jobs. Старый ACTIVE нельзя удалять до верификации и восстановления более нового поколения. Объекты без metadata требуют отдельного безопасного inventory. Retention не означает удаление log живого документа.

Локальный GC (ADR-037/038) переводит старые incomplete records в `REJECTED`
и выводит старые `ACTIVE` поколения, только если остались два более новых
ACTIVE снимка разных версий. Preferred snapshot не выводится. Publisher
проверяет результат каждого перехода; worker ограниченными пачками повторяет
идемпотентное удаление объектов. `cleanup_attempted_at` обеспечивает обход
всех `REJECTED` записей без starvation. Обнаружение объектов без metadata,
S3 consistency и backup/PITR остаются отдельными задачами.

Crash после object upload, до metadata update оставляет незавершенный record/object: recovery reader его не использует, будущий reconciler/GC решит судьбу. Crash после activation transaction дает читателю ACTIVE record. Corrupt ACTIVE при загрузке сейчас пропускается без изменения metadata и выбирается предыдущий ACTIVE; автоматический аудируемый `REJECTED` переход будет отдельным срезом. Object existence alone никогда не активирует snapshot.

Изолированный тест ADR-040 запускает PostgreSQL 18 с `synchronous_commit=on`,
`fsync=on`, `full_page_writes=on`, активирует snapshot v3, подтверждает v4,
останавливает PostgreSQL в режиме `immediate` и запускает его снова. Runtime
recovery сообщает только opaque версии/длительность: выбранный snapshot v3 и
одно replayed событие; restored state hash совпадает с ACK v4. Этот тест
покрывает сбой процесса PostgreSQL с WAL recovery, но не потерю питания,
носителя, S3 объекта или backup/PITR restore.

ADR-041 добавляет локальный restore drill: backend остановлен после v4,
snapshot objects копируются как единый quiescent набор, PostgreSQL получает
`pg_basebackup -X stream` и `pg_verifybackup`. После этого исходный документ
доходит до v5, а новый кластер из backup восстанавливает ровно v4 через
snapshot v3 + один event. Это подтверждает, что проверенная пара DB+blob
копий пригодна для восстановления при выбранной последовательности действий.
Для online backup без остановки писателей потребуется точный порядок захвата
blobs и БД, retention/fencing, manifest и регулярный restore drill; отдельно
нужны production WAL archive и PITR.

Локальный PITR drill (ADR-042) включает `archive_mode`, создаёт base backup
после v3, затем подтверждает v4, записывает именованный restore point и
подтверждает v5. После принудительного переключения WAL восстановленный
кластер читает archive до маркера, делает promotion и содержит только v4.
Backend подтверждает hash v4 и snapshot v3 + одно событие хвоста. Команда
архивации в этом тесте — локальное копирование, а не production-хранилище:
автоматическое off-site retention, monitor archive lag, проверка утраты
сегмента и согласование S3 объектов с online backup ещё открыты.

## Replay и hashes

Reader авторизует historical read, выбирает подходящий ACTIVE snapshot <=requested version, проверяет envelope/checksum/hash и последовательно применяет log tail. Versions должны быть непрерывны. Unknown event/schema, gap и checksum mismatch не пропускаются. Replay применяет сохраненные canonical operations, не повторяет сортировку/генерацию IDs или random source.

Точный read-only HTTP-запрос `/api/documents/{id}/versions/{version}` использует
этот путь и возвращает только версии `1..head`; он не меняет head или actor.
Сервер ограничивает одновременные historical reads и timeout. Расчётный cache
не восстанавливается как часть исторической projection; результаты формул
для указанного прошлого context пока не предоставляются.

Cacheless content hash использует versioned domain encoding: fixed tags/endianness/lengths, ordered axes, sorted maps, normalized IEEE-754 bits. Runtime caches, pending operations, presence и protobuf field order исключены. Context identity/epoch передаются отдельно; result hash включает engine/profile/context/input_version и нормализованные результаты. Comparison проводится только при одинаковых версиях и завершенном calculation generation.

[Protobuf serialization is not canonical](https://protobuf.dev/programming-guides/serialization-not-canonical/) — причина отдельной encoding, а не разрешение нестабильного domain hash.

## Retention и эксплуатация

По умолчанию durable log живого документа не сокращается. Удаление tenant/document и backups — отдельная авторизованная retention policy. Future archival обязана сохранять обещанную historical/rebase доступность либо явно сужать ее границу. Snapshot — ускорение восстановления, не replacement log.

Local COMMIT не обещает пережить полную потерю диска. Production gate потребует backup/PITR, restore drill, replication/RPO/RTO policy и secret management. Snapshot workers и future XLSX process не получают больше доступа, чем требуется задаче. Operation payload, preimages, formulas и blob content не попадают в обычные логи.
