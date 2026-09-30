# Collaboration: контракт последующих фаз

Foundation accepted for phase 0. Ниже проектный контракт для фаз 1–4.
Текущий код уже реализует bounded actor, CAS commit SetCellValue/SetCellFormula/ClearCell,
idempotent receipt, journal replay и отдельный WebSocket response для расчета.
Авторизованная подписка теперь воспроизводит до 128 canonical user events из
PostgreSQL до `ReplayComplete`, а затем догоняет live head из того же журнала
(ADR-015). Системное событие или больший gap требует HTTP projection resync.
Документный grant/revoke теперь использует durable `access_epoch` и
process-local payload send gate (ADR-016). Два разных dev-пользователя,
закрытие подписки и restart recovery проверены. Ephemeral presence и удалённое
выделение реализованы отдельно от журнала (ADR-017); два пользователя и
браузерный disconnect проверены. Outbound queue теперь ограничивает каждый
сокет по кадрам и байтам (ADR-018). Membership revoke, distributed fencing и
полный семантический gate фазы 3, включая сетевой stress и измерение latency,
остаются открытыми.
Подробные результаты находятся в RUNBOOK.

Первый структурный срез `ExtendRows` принят по ADR-019: actor добавляет
ограниченный batch стабильных row IDs только в конец оси, сериализует
конкурентные запросы через CAS, а WebSocket-клиент обновляет авторизованную
projection после структурного события. `last_axis_change_version` выводится
при replay. ADR-057 добавляет historical axis binding: формула связывается
по осям base version и трансформируется по позднейшим insert/delete. Cache
ограничен 10 000 версиями / 16 MiB accounted bytes и budget работы;
недоступная история требует typed `RebaseWindowExceeded` и resync.

`InsertRows` (ADR-020/021) использует stable row anchor, а не позицию клиента.
Два append-before одного anchor сохраняются в server commit order. Новое
schema-6 событие до CAS пересчитывает относительные row deltas всех формул,
а текущий A1-текст выводится из stable targets. Историческое schema-5 событие
сохраняет formula-free guard. Новый formula command со старой base axis version
может связываться через cache ADR-057; после рестарта история до recovered
head восстанавливается по запросу через ADR-058; недоступная или слишком
дорогая history требует resync. Это не заявленная полная
Excel-совместимость структуры.

`DeleteRows` (ADR-022) записывается отдельным schema-7 event. Все удаляемые
stable IDs должны быть живыми и уникальными; последняя строка остаётся.
Удалённые IDs сохраняются как tombstones и не могут быть повторно выделены.
Скалярная ссылка на удалённый target становится `#REF!`; частично уцелевший
диапазон сужается к живой границе. Stale edit по удалённому ID отклоняется,
оригинальный idempotent retry возвращает receipt. Replay проверяет тот же
domain hash. Матрица ниже задаёт целевой контракт полного набора операций;
inverse/undo для `DeleteRows` в текущем срезе ещё нет.

`InsertColumns` (ADR-023) использует stable column anchor и отдельное
schema-8 событие. Конкурентные вставки перед одним anchor следуют server
commit order; target IDs формул не меняются. Relative column offsets и
отображаемый A1-текст пересчитываются в candidate до CAS. Stale new formula
после изменения оси использует bounded history ADR-057 или требует resync,
если history недоступна. Inverse этой операции
остаётся планом из матрицы.

`DeleteColumns` (ADR-024) является отдельным schema-9 event. Все IDs должны
быть живыми и уникальными, последняя колонка остаётся; retired IDs не
используются повторно. Scalar references на удалённую колонку дают `#REF!`,
partial ranges сужаются до живых границ. Stale write получает typed
rejection, исходный idempotent retry — receipt. Replay проверяет domain hash.
Структурные inverse ещё не реализованы.

Guarded undo по ADR-028/076 реализован для новых одиночных value/clear writes
(schema 11) и formula writes (schema 16) с persisted preimage. `UndoCell` — отдельное намерение; schema-12
компенсация создаёт новую версию. Actor проверяет candidate, а журнал под
блокировкой документа сканирует до 10 000 последующих событий перед CAS.
Повтор команды возвращает receipt. Старые
события, batch и структурные операции этим срезом не
отменяются. ADR-029 разрешает восстановить формульный preimage одиночной
value/clear записи отдельной persisted schema 13 только если оси не менялись с версии отменяемого события;
после изменения осей возвращается `NotUndoable`.
ADR-076 применяет этот guard к formula result и preimage и повторяет его внутри
транзакции по общему `Operation::changes_axes`. Formula compensation с исторической
binding version использует projection resync: сохранённый canonical текст может
отличаться от текущего A1 после структурного изменения до отменяемой записи.
ADR-077 добавляет inverse собственной полной compensation как redo. Собственные
CellWrite пары сокращаются только по точному target ID/version вершины стека;
весь эффективный хвост должен быть пуст. Чужие, legacy, batch и copy записи того
же stable address остаются конфликтом даже после отмены. UI держит100
подтверждённых undo/redo IDs, новый edit очищает redo, unsupported own operation
очищает обе истории; reload/switch начинает новую локальную историю. Браузер
повторно проверяет наличие pending после receipt refresh, чтобы пересекающиеся
ACK не продвинули историю дважды. ADR-079 расширяет историю на новые интерактивные
RangeWrite: TSV и смешанные копии сохраняют точные preimages, а inverse отменяет
весь footprint либо отклоняется целиком. Scalar и range пары сокращаются по
ID/version отдельно для каждого адреса; чужие и старые schema10/14 записи остаются
барьерами. Скан ограничен 10000 версиями,64MiB payload и100000 matching-address
updates; превышение возвращает UndoWindowExceeded без нового commit. Структурный
undo и durable history discovery остаются отдельными незавершёнными срезами.

## Command, validation, event, projection

Command — намерение клиента: protocol_version, operation_id (UUIDv7), document_id, actor_id, base_version, client_sequence, client_instance_id, typed payload. Server AuthContext является authority; actor_id проверяется на совпадение, не дает права. ValidatedOperation — проверенное и нормализованное действие. CommittedEvent — неизменяемый durable факт с document version и schema version. Projection — восстанавливаемое состояние, не транспортный DTO.

В document actor последовательная bounded очередь. Application OperationJournal port появляется с первым use case; PostgreSQL adapter не зависит от collab. InProcessSessionDirectory создает actor single-flight. CAS все равно обязателен: локальная последовательность не защищает от старого process/writer.

## Commit algorithm

1. Decode проверяет wire version, recognized payload, frame/decoded limits. Проверяются tenant/document authorization и клиентские лимиты.
2. Найти durable operation_id только после authorization. Совпадающий canonical command fingerprint возвращает receipt; несовпадение — IdempotencyConflict. Canonical command fingerprint определяется отдельным versioned encoding, не произвольным protobuf byte order.
3. Загрузить необходимые пропущенные events; проверить base_version, выполнить разрешенный rebase, проверить вновь актуальные permissions/epochs. Future base отклоняется.
4. Построить canonical operation, preimages/inverse, projection delta. Применить к private candidate; проверить domain invariants и получить `confirmed_state_hash_without_calculated_cache`. Никакое observable actor state до commit не меняется.
5. Начать короткую transaction. Получить ACL locks/epochs в общем порядке с control lane; повторить authorization/idempotency check. CAS head на ожидаемую версию; INSERT event/idempotency/prepared hash; все constraints и event bytes caps проверяются.
6. COMMIT с durability settings из persistence.md. CAS miss, serialization failure, deadlock: rollback, reload, полная повторная preparation/validation/binding/authorization, новая transaction. Retry ограничен (стартовый cap 3), exhaustion — RetryableConflict с resync instructions.
7. После успешного COMMIT заменить projection на подготовленный candidate одной infallible публикацией. Publish canonical event и ACK. Установка не должна повторно выполнять domain apply, выделять неизвестно большой буфер или требовать запросов к БД.
8. Crash после COMMIT восстанавливается replay. Невозможная внутренняя ошибка запрещает новые writes: quarantined + закрытие подписок + восстановление с hash check. При неопределенном исходе COMMIT actor сначала разрешает исход по operation_id; не сообщает rejected.

Один batch — одна candidate validation, одна transaction, одна version, ноль частичных применений. Невалидные команды не входят в основной журнал. Error values формул не являются rejected transport commands.

Idempotent retry receipt содержит только `operation_id`, `committed_version`, `canonical_event_hash`, `current_head_version`. Клиент получает данные обычным авторизованным replay. Старый ACK/calculation patch не возвращается. Отозванное право не восстанавливается существованием receipt.

## Operation и calculation сообщения

`OperationCommitted`: document_id, committed_version, operation_id, canonical_operation, transformed, confirmed_state_hash_without_calculated_cache. Changed authored cells находятся в canonical operation/delta. ACK не обязан содержать recalculation patch.

`CalculationCommitted`: document_id, input_version, calculation_epoch, formula_engine_version, compatibility_profile, context_id, patch_sequence, is_final, bounded patch, calculated_state_hash. Result hash окончательно сравнивается при is_final; промежуточный chunk может иметь отдельный checksum, но не заявляет полноту всех results.

Клиент применяет event только к следующей confirmed version, duplicate игнорирует, gap вызывает replay. Calculation chunks буферизуются/применяются только в соответствующую calculation generation. Нельзя считать chunk для input_version 10 актуальным при confirmed_version 11. Смена version/context/epoch отменяет или логически supersede generation; sequence gaps не скрываются. Сервер может не считать промежуточные версии и отправить только результат последней. WASM preview помечен отдельно и не повышается в confirmed result самопроизвольно.

Actor не держит DB transaction во время calculation. Bounded CPU executor получает immutable input/version/context, поддерживает cooperative deterministic budget и cancellation. Mutating commands продолжают поступать между расчетами; stale worker completion отвергается по generation key.

## Окно преобразований и формулы на старом base

Начальное окно — последние 10 000 **всех document versions**, максимум 16 MiB структурных записей на active document, что наступит раньше. Lower bound может двигаться быстрее при крупных axis events. Сервер объявляет lower bound/capability в session state; это не гарантия вечного rebase.

В actor хранится axis-only checkpoint на lower bound: sheet order/names, row/column order, stable IDs, нужные tombstones. Следом идут структурные events до head. Для base строится bounded временная axis projection replay от checkpoint; не хранится полная книга на каждую версию. При сдвиге окна checkpoint продвигается теми же структурными events. После restart checkpoint/window восстанавливаются из durable snapshot/log прежде, чем объявляется способность rebase. Если history unavailable, conservative lower bound=head и старые формулы получают resync error.

SetCellFormula: server parse source → bind A1/quoted sheet names относительно base axes → transform bound refs через structural tail → normalize относительно current formula origin → validate candidate. AST клиента можно использовать только как hint для проверки, его IDs не являются authority. RenameSheet входит в structural history resolver, хотя не двигает оси.

`RebaseWindowExceeded` с `resync_required=true` возвращается, когда необходимый base context вне окна. Stable-ID property writes без чтения base могут rebase и при более старом base, если команда не имеет дополнительных preconditions; future base все равно ошибка. Read-dependent Move/Copy/Sort/merge не читают current content вместо обещанного base молча: head-only retry до наличия bounded historical read support.

В MVP log append-only и не compacts/deletes. Snapshot activation не отменяет rebase window. Будущая archival/compaction обязана либо сохранить axis checkpoint+tail и заявленный lower bound, либо атомарно повысить lower bound и отвергать более старые commands; обещанный base нельзя реконструировать приблизительно.

## Replay/live barrier

1. Actor принимает Subscribe(last_seen_version) в последовательной очереди и авторизует его.
2. Фиксирует barrier_version V и **сначала** регистрирует subscriber для events >V.
3. Читает из БД `(last_seen_version,V]` в порядке версии; live >V идет в отдельный bounded buffer, также ограниченный bytes.
4. Отправляет replay, затем `ReplayComplete(V)`.
5. Отправляет buffered live >V в порядке версий и переходит в live. Новые события добавляются после уже накопленных, без второй сортировки по arrival time.

Клиент меняет режим после ReplayComplete, но сохраняет общую sequential-version проверку. При слишком большом replay actor фиксирует checkpoint на V, регистрирует buffer до отправки checkpoint и затем шлет >V. До phase 5 это transient projection checkpoint, не durable snapshot. Buffer overflow/timeout прекращают подписку и требуют resubscribe; slow client не блокирует actor. Presence может coalesce/drop и не входит в replay.

## Access epoch и контроль отправки

Document access_epoch и membership access_epoch образуют server-issued authorization ticket вместе с principal/tenant. Каждая команда, replay/history и export проверяют права. Проверка ACL при commit не заменяет проверку подписок.

ACL change идет через строго упорядоченную authorization control lane. Общий порядок блокировок: membership control gate → document gate → DB membership/document rows; ordinary commit пользуется тем же порядком. Membership revoke охватывает все активные document gates этого principal в детерминированном порядке. Обход этого пути прямым административным SQL не является поддерживаемым API.

Перед ACL transaction control lane закрывает payload send gate затронутых подписок. Очередной write не может начать отправку под старым ticket. Уже начатая отправка имеет bounded timeout; она линейризуется до revocation. Затем transaction меняет ACL, увеличивает epochs, пишет durable authorization control event/outbox. После COMMIT старые queued payloads удаляются, подписка закрывается или требует reauthorization, gate старого epoch не открывается. Rollback может возобновить прежний gate только после проверки epoch.

Реализованный MVP-срез относится к document ACL: `POST /api/documents/{id}/permissions`
принимает `action=grant|revoke`, UUIDv7 `change_id` и target user ID. Grant
поддерживает роли editor/viewer и при необходимости добавляет существующего
пользователя в workspace. Изменение role/epoch и schema-1 append-only audit
атомарны в PostgreSQL. Отдельного distributed outbox пока нет; он нужен до
multi-process fanout. Повтор исходного intent возвращает прежний receipt,
коллизия ID отклоняется. Текущий in-process gate держит read lock на время
финальной проверки epoch и отправки каждого document frame, write lock — на
время ACL transaction. Это не распределенный control lane для нескольких
server processes и не реализация membership revoke.

Writer проверяет текущий ticket непосредственно перед каждым новым payload send под gate; проверка при enqueue недостаточна. Bytes, уже переданные OS/socket до revoke, невозможно отозвать. ACK revoke означает, что дальнейшие sends под старым epoch не начнутся. Crash закрывает connections; новые подписки читают durable epochs. Будущий multi-process fanout требует acknowledged invalidation/fencing, а не только PostgreSQL NOTIFY.

## Лимиты начальной реализации

| Ресурс                    |                            Cap | Поведение                                                            |
| ------------------------- | -----------------------------: | -------------------------------------------------------------------- |
| max_command_frame_bytes   |                          1 MiB | Reject до decode                                                     |
| max_batch_cells           | 1000 source; 10000 move writes | Все или ничего; TSV/copy до1000 writes                               |
| max_decoded_command_bytes |          единый cap ещё открыт | Frame cap и semantic limits действуют; общего decoded accounting нет |
| max_persisted_event_bytes |                          4 MiB | Включая envelope, transforms и preimages; reject до COMMIT           |
| max_broadcast_patch_bytes |                          1 MiB | Bounded message; packetization overhead входит в cap                 |
| actor queue               |                    64 commands | Backpressure/retryable busy                                          |
| client pending            |                 128 operations | UI backpressure; move только при idle                                |
| outbound                  |           160 messages / 2 MiB | Disconnect slow consumer; socket send timeout2s                      |
| heartbeat                 |             15 s; timeout 45 s | Reconnect/resume                                                     |
| calculation work          |            1 000 000 units/job | Typed CalculationLimitExceeded                                       |

Это текущие caps по коду, не measured performance; общий decoded-memory accounting
остаётся открытым hardening gate. Wire patch либо head hint должен помещаться в
frame cap с overhead; persisted preimages не передаются как wire patch. Range
inverse и MoveRange используют authorized HTTP projection refresh. Chunked batch
delivery/HTTP event reference — будущее расширение; оно не меняет atomic semantics.
Protobuf input ограничен1MiB до decode; дальнейшие semantic limits проверяются
до candidate. Нельзя выдавать это за отдельный общий4MiB decoded-memory budget.

ADR-081: MoveRange требует exact head, фиксирует все source/destination preimages
и затронутые inbound formulas до CAS; stale/partial-range refusal не создаёт version.
После content move delayed formula binding со старого base получает code17;
новый actor восстанавливает эту границу по bounded history даже без axis changes.
Transient admission failures сохраняют ID/bytes для receipt retry. Move пока
history barrier; undo historical formula через последующий move запрещён.

## Матрица операций

Таблица объединяет реализованные и планируемые контракты; фактический статус каждого среза указан в roadmap и ADR. E = editor/owner текущего документа; R = read для source; P = guarded preimage restore новой компенсацией. Inverse никогда не уменьшает head. Guards проверяют последующие content/style/structural writes и чужие references; обычный undo разрешен только владельцу исходной операции. Превышение bytes preimage отклоняет весь command.

| Command                    | Preconditions / validation                                             | Rebase                                               | Inverse                               | Dependency impact                    | Auth                         | Persisted representation                                           |
| -------------------------- | ---------------------------------------------------------------------- | ---------------------------------------------------- | ------------------------------------- | ------------------------------------ | ---------------------------- | ------------------------------------------------------------------ |
| SetCellValue               | Живой target; тип/bytes/finite                                         | Stable-ID content write; server order                | P content                             | Target/downstream                    | E                            | key,value,preimage                                                 |
| SetCellValues              | 1–1000 distinct live targets; ≤512 KiB text; all-or-nothing            | Stable-ID writes; one candidate/CAS version          | Guarded P for all targets atomically  | Union of targets/downstream          | E                            | schema17 RangeWrite with exact preimages; legacy schema10 retained |
| SetCellFormula             | Живой target; parse/AST/budget/base axes                               | Server bind на base + transform window               | P content                             | Remove old/add new edges, downstream | E                            | source,normalized AST+schema,preimage                              |
| ClearCell                  | Живой target; content mask                                             | Stable ID, style независим                           | P content                             | Target/downstream                    | E                            | key,mask,preimage                                                  |
| SetCellStyle               | Живой target; allowlisted fields                                       | Порядок отдельно по property                         | P fields                              | Нет numeric impact                   | E                            | key,style delta,preimage                                           |
| SetRangeStyle              | Живой bounded range; cells/bytes                                       | Измененная geometry → retry                          | P fields                              | Нет numeric impact                   | E                            | resolved range,delta,preimages                                     |
| InsertRows / InsertColumns | Живой anchor/end; count/IDs/caps                                       | Stable anchor; commit order                          | Guarded delete new IDs                | Axes/refs/ranges                     | E                            | IDs,resolved anchor,reference delta                                |
| ExtendRows                 | Последний extent; batch count/caps                                     | Append к current end; новые unique IDs               | Guarded delete appended IDs           | Axis/ranges                          | E                            | new IDs,prior extent                                               |
| DeleteRows / DeleteColumns | Все IDs живы; merges/limits                                            | Удаленный ID → typed rejection                       | Restore old IDs + P                   | Axes/refs/ranges/downstream          | E                            | IDs,anchors,preimages,reference delta                              |
| ResizeRow / ResizeColumn   | Живой axis ID; размер в bounds                                         | Property server order                                | P size                                | Layout only                          | E                            | ID,size,prior                                                      |
| RenameSheet                | Живой ID; bounded unique name                                          | Name collision rejected                              | P name                                | Resolver/formatter                   | E                            | ID,old/new name                                                    |
| AddSheet                   | ≤64 sheets; unique name and stable IDs; bounded axes                   | Append in CAS order; name collision rejects          | Guarded DeleteSheet (planned)         | Workbook/resolver                    | E                            | schema-15 ID,name,ordered axes                                     |
| DeleteSheet                | Живой; остается один; preimage cap                                     | Deleted target → rejection                           | RestoreSheet + P                      | Cross-sheet refs/downstream          | E                            | sheet preimage,reference delta                                     |
| ReorderSheet               | Sheet/anchor alive; not self                                           | Stable anchor                                        | Guarded previous order                | Order; no 3D refs                    | E                            | ID,anchor,previous order                                           |
| MergeCells                 | Rectangle; secondary cells blank; no overlaps                          | Changed geometry/content → retry                     | Guarded Unmerge                       | Layout                               | E                            | merge ID,range,preimage                                            |
| UnmergeCells               | Merge ID/revision valid                                                | Recreated merge → retry                              | Guarded Merge                         | Layout                               | E                            | merge ID,preimage                                                  |
| CopyRange                  | Same-sheet live source/dest; 1–1,000 cells; exact head                 | Stale source rejected; overlap reads original source | Guarded destination preimages         | Dest/new refs/downstream             | E                            | Schema17 frozen writes+preimages; legacy schema14 retained         |
| MoveRange / cut-paste      | Same-sheet live rectangle; area<=1000; writes<=10000; no partial range | Exact head; receipt before source check              | Guarded own MoveCompensation schema20 | Relocation map + inbound/downstream  | E                            | Schema19 geometry+guarded source/dest/inbound writes               |
| SortRange                  | Deferred; atomic rectangle only; merges/arrays unsupported             | Always retry stale keys/geometry                     | Guarded inverse content permutation   | Range content/formula refs           | E                            | Final content permutation/ref delta, never re-sort on replay       |
| SetFrozenPanes             | Sheet alive; bounds                                                    | Property server order                                | P boundaries                          | Layout                               | E                            | sheet,stable boundaries,prior                                      |
| OperationBatch             | All children pass combined cells/bytes/cost                            | Entire batch revalidated                             | Guarded inverse reversed              | Union dirty set                      | All child permissions        | One envelope/children/preimages                                    |
| Undo / Redo                | Own operation; all guards pass                                         | Foreign dependent write → UndoConflict               | New compensation                      | According to target operation        | E + ownership                | compensates_operation_id,canonical inverse                         |
| AdvanceCalculationEpoch    | System context validated; epoch monotonic                              | Ordered system event                                 | No ordinary undo                      | Volatile + downstream                | System; E may request recalc | Complete context + reason                                          |

Повтор того же operation_id возвращает receipt до повторного domain apply. Повтор delete с новым operation_id обращается к уже удаленной identity и получает typed rejection. History хранит обе версии concurrent same-property edits; самостоятельного rollback глобальной версии нет.

## Protocol compatibility

Wire major согласуется handshake; неизвестный major, enum/operation schema с обязательной семантикой или unrecognized oneof отвергается без partial apply. Optional additive fields могут быть совместимы, tags не переиспользуются. Proto3 сохранение unknown fields не означает разрешение выполнить неизвестную operation. Breaking checks сравнивают public schema с зафиксированным baseline; долговременная event schema имеет отдельные fixtures и read migrations. В phase 0 operation messages выше остаются документацией, а не пустыми незавершенными DTO.

ADR083 enables own MoveRange/MoveCompensation UndoCell targets. The complete
ordered inverse and original geometry persist in schema20. Under ACL/document
lock, the journal binds the internal confirmed projection to verified durable
head doc/version/hash, verifies target family/geometry/preimages and checks
bounded tail references before CAS/COMMIT. Own exact inverse pairs neutralize;
foreign/legacy equal-value writes and new effective references remain barriers.
Compensation updates the relocation floor on recovery too. Delivery uses existing
authorized projection resync; moves enter acknowledged100-entry local history.
Cross-sheet/partial-range/style/merge moves, axis/style compensation, discovered
history remain open. ADR084 adds geometry-only shared Rust Worker preview, kept
in a private pending overlay until server confirmation. Worker failure never
blocks durable command submission. A newer head, terminal rejection, refresh or
ordinary edit discards the private moved projection; retryable uncertainty keeps
the original operation bytes/ID. See ADR083/084 and RUNBOOK.
