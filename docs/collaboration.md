# Collaboration: контракт последующих фаз

Foundation accepted for phase 0. Ниже проектный контракт для фаз 1–4.
Текущий код уже реализует bounded actor, CAS commit SetCellValue/SetCellFormula/ClearCell,
idempotent receipt, journal replay и отдельный WebSocket response для расчета.
Коалесцирующий авторизованный version hint уже уведомляет вторую вкладку и
вызывает HTTP refresh (ADR-014). Live-подписки canonical operations, replay/live
barrier, presence и отзыв подписок еще не реализованы; их семантический gate остается
отдельным. Подробные результаты текущего среза находятся в RUNBOOK.
Отзыв dev-auth cookie теперь обнаруживается перед следующей WebSocket-командой
или отправкой document payload и закрывает соединение; это не заменяет
упорядоченный `access_epoch` gate для изменения document ACL.

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

Writer проверяет текущий ticket непосредственно перед каждым новым payload send под gate; проверка при enqueue недостаточна. Bytes, уже переданные OS/socket до revoke, невозможно отозвать. ACK revoke означает, что дальнейшие sends под старым epoch не начнутся. Crash закрывает connections; новые подписки читают durable epochs. Будущий multi-process fanout требует acknowledged invalidation/fencing, а не только PostgreSQL NOTIFY.

## Лимиты начальной реализации

| Ресурс                    |                  Cap | Поведение                                                       |
| ------------------------- | -------------------: | --------------------------------------------------------------- |
| max_command_frame_bytes   |                1 MiB | Reject до decode                                                |
| max_batch_cells           |               10 000 | Все или ничего                                                  |
| max_decoded_command_bytes |                4 MiB | Проверка strings/collections и accounting, не только wire size  |
| max_persisted_event_bytes |                1 MiB | Включая envelope, transforms и preimages; reject до transaction |
| max_broadcast_patch_bytes |                1 MiB | Bounded message; packetization overhead входит в cap            |
| actor queue               |         256 commands | Backpressure/retryable busy                                     |
| client pending            |        32 operations | UI backpressure                                                 |
| outbound                  | 128 messages / 8 MiB | Disconnect slow consumer                                        |
| heartbeat                 |   15 s; timeout 45 s | Reconnect/resume                                                |
| calculation work          |  1 000 000 units/job | Typed CalculationLimitExceeded                                  |

Это проверяемые конфигурационные caps, не measured performance. В первой версии принимаются только batches, чье canonical event помещается в frame cap с overhead. Chunked batch delivery/HTTP event reference — будущее расширение; оно не меняет atomic persisted batch semantics. Декодер ограничивает элементы до их неограниченного выделения, а не только после получения большого AST.

## Матрица операций

Все строки — планируемые контракты. E = editor/owner текущего документа; R = read для source; P = guarded preimage restore новой компенсацией. Inverse никогда не уменьшает head. Guards проверяют последующие content/style/structural writes и чужие references; обычный undo разрешен только владельцу исходной операции. Превышение bytes preimage отклоняет весь command.

| Command                    | Preconditions / validation                                 | Rebase                                                   | Inverse                             | Dependency impact                    | Auth                         | Persisted representation                                     |
| -------------------------- | ---------------------------------------------------------- | -------------------------------------------------------- | ----------------------------------- | ------------------------------------ | ---------------------------- | ------------------------------------------------------------ |
| SetCellValue               | Живой target; тип/bytes/finite                             | Stable-ID content write; server order                    | P content                           | Target/downstream                    | E                            | key,value,preimage                                           |
| SetCellFormula             | Живой target; parse/AST/budget/base axes                   | Server bind на base + transform window                   | P content                           | Remove old/add new edges, downstream | E                            | source,normalized AST+schema,preimage                        |
| ClearCell                  | Живой target; content mask                                 | Stable ID, style независим                               | P content                           | Target/downstream                    | E                            | key,mask,preimage                                            |
| SetCellStyle               | Живой target; allowlisted fields                           | Порядок отдельно по property                             | P fields                            | Нет numeric impact                   | E                            | key,style delta,preimage                                     |
| SetRangeStyle              | Живой bounded range; cells/bytes                           | Измененная geometry → retry                              | P fields                            | Нет numeric impact                   | E                            | resolved range,delta,preimages                               |
| InsertRows / InsertColumns | Живой anchor/end; count/IDs/caps                           | Stable anchor; commit order                              | Guarded delete new IDs              | Axes/refs/ranges                     | E                            | IDs,resolved anchor,reference delta                          |
| ExtendRows                 | Последний extent; batch count/caps                         | Append к current end; новые unique IDs                   | Guarded delete appended IDs         | Axis/ranges                          | E                            | new IDs,prior extent                                         |
| DeleteRows / DeleteColumns | Все IDs живы; merges/limits                                | Удаленный ID → typed rejection                           | Restore old IDs + P                 | Axes/refs/ranges/downstream          | E                            | IDs,anchors,preimages,reference delta                        |
| ResizeRow / ResizeColumn   | Живой axis ID; размер в bounds                             | Property server order                                    | P size                              | Layout only                          | E                            | ID,size,prior                                                |
| RenameSheet                | Живой ID; bounded unique name                              | Name collision rejected                                  | P name                              | Resolver/formatter                   | E                            | ID,old/new name                                              |
| AddSheet                   | Limits/name/axis IDs valid                                 | Stable anchor; commit order                              | Guarded DeleteSheet                 | Workbook/resolver                    | E                            | ID,name,axes,anchor                                          |
| DeleteSheet                | Живой; остается один; preimage cap                         | Deleted target → rejection                               | RestoreSheet + P                    | Cross-sheet refs/downstream          | E                            | sheet preimage,reference delta                               |
| ReorderSheet               | Sheet/anchor alive; not self                               | Stable anchor                                            | Guarded previous order              | Order; no 3D refs                    | E                            | ID,anchor,previous order                                     |
| MergeCells                 | Rectangle; secondary cells blank; no overlaps              | Changed geometry/content → retry                         | Guarded Unmerge                     | Layout                               | E                            | merge ID,range,preimage                                      |
| UnmergeCells               | Merge ID/revision valid                                    | Recreated merge → retry                                  | Guarded Merge                       | Layout                               | E                            | merge ID,preimage                                            |
| CopyRange                  | Live bounded source/dest; supported formula refs           | Head-only retry initially; no silent current-source read | P destination                       | Dest/new refs/downstream             | E+R                          | frozen copied content,transformed AST,P                      |
| MoveRange / cut-paste      | Source/dest valid; no unsupported overlaps/merges          | Head-only retry initially                                | P both ranges/inbound refs          | Relocation map + downstream          | E+R                          | canonical content relocation,ref transforms,P                |
| SortRange                  | Deferred; atomic rectangle only; merges/arrays unsupported | Always retry stale keys/geometry                         | Guarded inverse content permutation | Range content/formula refs           | E                            | Final content permutation/ref delta, never re-sort on replay |
| SetFrozenPanes             | Sheet alive; bounds                                        | Property server order                                    | P boundaries                        | Layout                               | E                            | sheet,stable boundaries,prior                                |
| OperationBatch             | All children pass combined cells/bytes/cost                | Entire batch revalidated                                 | Guarded inverse reversed            | Union dirty set                      | All child permissions        | One envelope/children/preimages                              |
| Undo / Redo                | Own operation; all guards pass                             | Foreign dependent write → UndoConflict                   | New compensation                    | According to target operation        | E + ownership                | compensates_operation_id,canonical inverse                   |
| AdvanceCalculationEpoch    | System context validated; epoch monotonic                  | Ordered system event                                     | No ordinary undo                    | Volatile + downstream                | System; E may request recalc | Complete context + reason                                    |

Повтор того же operation_id возвращает receipt до повторного domain apply. Повтор delete с новым operation_id обращается к уже удаленной identity и получает typed rejection. History хранит обе версии concurrent same-property edits; самостоятельного rollback глобальной версии нет.

## Protocol compatibility

Wire major согласуется handshake; неизвестный major, enum/operation schema с обязательной семантикой или unrecognized oneof отвергается без partial apply. Optional additive fields могут быть совместимы, tags не переиспользуются. Proto3 сохранение unknown fields не означает разрешение выполнить неизвестную operation. Breaking checks сравнивают public schema с зафиксированным baseline; долговременная event schema имеет отдельные fixtures и read migrations. В phase 0 operation messages выше остаются документацией, а не пустыми незавершенными DTO.
