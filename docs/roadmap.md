# Roadmap и gates

Статус: пользователь разрешил продолжать все оставшиеся стадии, 2026-09-28.
Локальные проверки фазы 0 пройдены; реализация продолжается последовательными
срезами по ADR-008. Linux/Compose/remote CI остаются открытым deployment gate.
Каждая новая фаза проверяет собственные acceptance criteria; разрешение продолжать
не означает, что будущие функции или совместимость уже подтверждены.

Локальный функциональный gate фазы 2 по описанному ниже базовому профилю
пройден 2026-09-29: общий native/browser WASM corpus 99/99, 1000 книг/2000
patches с одинаковыми normalized hashes; инкрементальность, cycle и parser
limits покрыты Rust tests. Полная цепочка 10 000 bound formulas измерена
Criterion: 41.822–42.905 ms (optimized, Windows/Ultra 9 185H, 10 samples).
Это не p95 UI и не Excel golden parity. AND/OR/NOT, volatile-функции, широкий
совместимый профиль и Linux gate остаются отдельными работами.

Фаза 3 идёт отдельными срезами: bounded PostgreSQL replay/barrier (ADR-015),
durable ACL access epoch/send gate (ADR-016) и ephemeral presence с удалённым
выделением (ADR-017) локально проверены. Domain-only seeded convergence
100×1000×3 прошёл; harness дополнен вставкой/удалением стабильных строк и
столбцов и frozen CopyRange значений. Локальный сетевой scalar/LWW harness
100×1000 прошёл за 1 310 127 ms с двумя
авторами, live/replay подписчиком, повторами и reconnect. Отдельный hard-restart
сценарий с двумя авторами прошёл. Добавлен живой сценарий формула + ACL revoke:
viewer видит серверный расчёт после правки, затем его подписка закрывается и
последующий расчёт запрещён. После исправления временного HTTP 429 при загрузке
projection общий `just smoke` проверил integration 43/43, recovery 2/2 и
Chromium 48/48. Полный gate фазы 3 ещё открыт: нужны масштабный совмещённый
ACL/формульный convergence stress, cross-host stress и измерения latency. Outbound queue с
лимитом кадров/байтов реализована в ADR-018; cross-host slow-reader stress и
очередные метрики ещё открыты. Локальный paused-socket stress выполнен:
три прогона по 512×30,005-byte commits завершились head 513, медленный socket
закрылся, писатель не остановился. Третий прогон обнаружил send timeout;
queue overflow в сети и cross-host case ещё не подтверждены;
[условия](benchmarks/2026-09-29-slow-reader-local.md).
Дополнительный сетевой convergence test прошёл 5 фиксированных сценариев через
настоящий WebSocket/PostgreSQL: конкурентные писатели, retry, `ExtendRows`,
disconnect/reconnect и поздний replay. Версии пришли без пропусков, финальный
event state hash совпал с HTTP canonical projection. Это меньше целевых
100×1000 сетевых сценариев; позднее они проверены отдельным scalar harness.
Ещё 5 сетевых сценариев смешали конкурентные записи, вставку/удаление строк и
столбцов, frozen CopyRange, idempotent retry и reconnect: поздний replay после
CopyRange был непрерывным, а replay через CopyRange честно потребовал projection
resync на barrier. Тогда полный PostgreSQL/WebSocket integration suite прошёл 40/40;
после добавления сетевого scalar harness общий smoke прошёл 42/42.
Эти пять структурных сценариев не входят в масштабный scalar сетевой прогон.
Живой integration test одновременной записи в одну ячейку подтвердил версии
2/3, серверный порядок и значение победителя после replay. Это не заменяет
случайные multi-user сценарии с разными задержками и отказами.
Browser E2E при DPR 1/2 проверил, что отключенная вкладка после удаленной
правки вновь подключается и показывает canonical значение. Восстановление
может использовать HTTP projection refresh; тест не доказывает исключительно
operation replay. Он прошел в двух полных suites 18/18 после ограничения
параллелизма Playwright до четырех workers из-за dev rate limit 64/s.
Первый ограниченный срез фазы 4 реализован отдельно (ADR-019): `ExtendRows`
добавляет до 1,000 stable row IDs за одну durable operation, автоматически
срабатывает у нижней границы Canvas и восстанавливается после restart.
Два клиента, idempotent retry, replay и stale-formula rejection проверены.
Второй срез фазы 4 (ADR-020) добавляет `InsertRows` перед stable row anchor:
значение остаётся на прежнем ID, параллельные вставки следуют server order,
retry и replay идемпотентны. В историческом schema-5 срезе книга с формулами
получала typed `UnsupportedStructuralDependency`; этот отказ сохранён для
старых событий и заменён в новых командах следующим срезом.
Третий срез фазы 4 (ADR-021) вводит отдельное persisted schema 6 для вставки
строк с формулами: stable targets сохраняются, relative row offsets
пересчитываются до CAS, текущий A1-текст строится из bound AST, а старые
schema-5 события сохраняют прежнюю семантику. Локальные PostgreSQL/WebSocket,
browser DPR 1/2 и hard restart recovery прошли. Criterion для candidate с
10 000 формулами: 12.078–13.443 ms, без БД/сети; [условия](benchmarks/2026-09-29-formula-insertion-local.md).
Четвёртый срез фазы 4 (ADR-022) добавляет `DeleteRows` со schema 7:
удалённый stable target становится `#REF!`, пустые ячейки не создаются,
удалённые row IDs не используются повторно. PostgreSQL/WebSocket integration
и hard restart recovery прошли. Criterion дал 14.974–17.812 ms для удаления
target строки при 10 000 формулах в domain candidate, без БД/сети;
[условия](benchmarks/2026-09-29-row-deletion-local.md). Browser результаты
находятся в RUNBOOK. Пятый срез фазы 4 (ADR-023) добавляет `InsertColumns`
со schema 8: stable targets и source текста сохраняются, относительные
column offsets и текущий A1-текст меняются до CAS. PostgreSQL/WebSocket,
hard restart и browser DPR 1/2 проверены. Criterion: 11.853–14.140 ms
для domain candidate с 10 000 формулами;
[условия](benchmarks/2026-09-29-column-insertion-local.md). Фаза 4 всё ещё
открыта; следующий срез — удаление столбцов.
Шестой срез фазы 4 (ADR-024) добавляет `DeleteColumns` schema 9: ссылки
на удалённый stable target дают `#REF!`, частичные ranges сужаются,
удалённый ID не выдаётся повторно. PostgreSQL/WebSocket и hard restart
проверены. Criterion: 12.241–13.377 ms для domain candidate с 10 000
формулами; [условия](benchmarks/2026-09-29-column-deletion-local.md).
Copy/paste, undo, multi-sheet compatibility и snapshots ещё открыты.
Седьмой срез фазы 4 (ADR-025) добавляет атомарную TSV-вставку обычных значений:
до 1,000 ячеек и 512 KiB текста сохраняются одной schema-10 операцией и одной
версией. PostgreSQL/WebSocket commit, retry, atomic rejection и пересчёт
зависимой формулы проверены; браузер DPR 1/2 подтвердил reload. Criterion
для 1,000 numeric writes в domain candidate: 567.59–631.31 µs
([условия](benchmarks/2026-09-29-value-batch-local.md)). Копирование формул,
rich clipboard, cut/move и undo остаются отдельными срезами.
Восьмой срез фазы 4 (ADR-026) добавляет copy/paste одной формулы в пределах
документа: текущий A1-текст преобразуется общим Rust-кодом в WASM Worker,
сервер заново bind-ит полученный текст через `SetCellFormula`. Native tests и
browser DPR 1/2 проверили mixed `$` modes, диапазоны, `#REF!`, commit и reload.
Локальный Worker benchmark: p50 12.50 ms, p95 16.10 ms для 50 fresh-Worker
копирований после warm-up; это не latency commit и не официальный gate
([условия](benchmarks/2026-09-29-formula-copy-worker-local.md)). Multi-cell
formula copy, cut/move, undo и snapshots ещё открыты.
Девятый срез фазы 4 (ADR-027) вводит прямоугольное выделение Shift+стрелками
и pointer drag в Canvas, stable-ID remap anchor/focus при структурных изменениях,
TSV-копирование до 1,000 обычных значений и вставку в левый верхний угол
через существующий атомарный schema-10 batch. Native server event не меняется;
browser DPR 1/2 подтвердил один commit, reload и отсутствие DOM-ячеек.
Смешанные диапазоны с формулами требуют следующего server-side `CopyRange` с
замороженным source; локальный TSV-copy не выдается за такую операцию.
Десятый узкий срез фазы 4 ([ADR-028](adr/028-guarded-single-cell-undo.md)):
guarded undo только новых одиночных value/clear writes своего автора. Schema 11
хранит preimage, schema 12 — новую компенсирующую версию; поздняя запись в ту же
stable ячейку отклоняет undo даже при совпадающем значении. PostgreSQL tests,
hard restart/retry и browser DPR 1/2 прошли. Локальный одинарный замер с
хвостом 1000 событий: 22.461 ms, не p95
([условия](benchmarks/2026-09-29-undo-tail-local.md)). Старые события,
формульный preimage, batch и структура не отменяются этим срезом.
Одиннадцатый срез ([ADR-029](adr/029-formula-preimage-undo.md)) восстанавливает
формульный preimage одиночной value/clear записи, если после неё не менялись
оси. Formula AST сохраняется в schema-13 компенсации; canonical patch сообщает
`SetCellFormula`, а сервер пересчитывает результат. После структурного изменения
`UndoCell` возвращает `NotUndoable`, не связывая старый source с новыми осями.
Domain/codec, PostgreSQL/WebSocket и browser DPR 1/2 проверки прошли. General
formula undo, batch/structural undo и redo ещё открыты.
Двенадцатый срез ([ADR-030](adr/030-authoritative-copy-range.md)) добавляет
server-side `CopyRange` смешанного прямоугольника до 1,000 ячеек: source
фиксируется на точной версии документа, формулы связываются сервером в точках
назначения, а значения и пустоты сохраняются одной schema-14 операцией.
Устаревший source отклоняется; текущий wire v1 передаёт head/version/hash,
после чего клиент обновляет projection по HTTP. Cut/move, стили и batch undo
по-прежнему открыты. Результаты полного gate записываются в RUNBOOK.
Первый срез фазы 5 ([ADR-031](adr/031-snapshot-image-and-tail-replay.md))
добавляет внутренний `SnapshotImage` и проверенное восстановление sparse
projection: bound formulas, retired stable IDs, CalculationContext и
`last_axis_change_version` сохраняются; применение хвоста даёт тот же
domain hash, что непрерывный replay. Следующие срезы добавили codec,
локальную activation и optional S3 adapter по ADR-039.
Второй срез фазы 5 ([ADR-032](adr/032-versioned-binary-snapshot-codec.md))
добавляет отдельный protobuf `SnapshotEnvelope` schema 1 и строгий decoder
с лимитом 32 MiB; формула, context, tombstones, hash и tail replay проходят
codec test. На том срезе runtime ещё читал полный журнал.

Третий срез фазы 5 ([ADR-033](adr/033-snapshot-metadata-activation.md))
добавляет `document_snapshots`, локальную публикацию `UPLOADING → STORED →
VERIFIED → ACTIVE` и проверенный reader с fallback к предыдущему `ACTIVE`.
PostgreSQL/BlobStore тест проверяет повреждение нового объекта; внутренний
recovery API в этом случае применяет только хвост, а без валидных снимков
читает весь журнал. Четвёртый срез ([ADR-034](adr/034-actor-bootstrap-from-snapshot.md))
подключил проверенное восстановление к startup actor; при восстановлении
запускается полный пересчёт cache. Пятый срез ([ADR-035](adr/035-bounded-snapshot-scheduler.md))
добавил bounded неблокирующий scheduler после каждых 1000 committed событий
по умолчанию; локальный smoke с порогом 1 подтвердил ACTIVE snapshot v2;
при пропуске milestone следующий шанс через 1000 событий. Domain-only
benchmark snapshot + 10 000 tail events дал 5.7527–8.6369 ms против
17.024–20.579 ms для полного replay 20 001 events; условия —
[отчёт](benchmarks/2026-09-29-snapshot-replay-domain-local.md). Отдельный
release harness с реальными PostgreSQL, checksum/codec и LocalFilesystemBlobStore
дал p95 71.356–87.611 ms для snapshot + 10 000 tail events в двух локальных
прогонах; full replay p95 131.171–139.006 ms. Условия и ограничения —
[отчёт](benchmarks/2026-09-29-snapshot-replay-persisted-local.md). Полная crash
matrix и S3 contract для Garage/российского провайдера остаются открытыми.
Controlled interruption на шести границах publication уже проверил
metadata/BlobStore/activation rollback; power-loss не проверен.
Уборка incomplete/rejected snapshots ([ADR-037](adr/037-incomplete-snapshot-cleanup.md))
проверена на PostgreSQL/LocalFilesystemBlobStore, включая гонку с publisher.
Ограниченный GC старых ACTIVE поколений ([ADR-038](adr/038-active-snapshot-retention.md))
оставляет два последних снимка разных версий и полный журнал. Historical
read версии без снимка проверен через replay; после потери newest восстановление
идёт из предпоследнего. Optional S3 adapter ([ADR-039](adr/039-s3-snapshot-blob-adapter.md))
прошёл локальный HTTP contract на Moto 5.2.3 (8/8); server hard-restart test
с порогом snapshot 1 создал девять новых ACTIVE S3 снимков. Garage 2.4.0,
российский S3, inventory orphan objects и power-loss остаются открытыми.
Отдельный crash recovery test ([ADR-040](adr/040-postgres-immediate-stop-recovery.md))
прошёл на изолированном PostgreSQL 18: после `immediate` stop и WAL recovery
actor поднял ACTIVE snapshot v3 и одно committed событие хвоста v4; hash
совпал с ACK. Это не имитация потери питания или диска.
Локальный restore drill ([ADR-041](adr/041-quiescent-backup-restore.md)) прошёл:
`pg_basebackup` + `pg_verifybackup` и snapshot objects восстановили v4 в новом
кластере, хотя источник после backup уже достиг v5; actor взял snapshot v3 +
один event. Online-consistent backup, production WAL archive/PITR и off-site
копии открыты.
Локальный PITR drill ([ADR-042](adr/042-named-point-in-time-recovery.md)) прошёл:
WAL archive и named restore point после v4 восстановили документ до v4,
исключив committed v5; snapshot v3 + один event совпали с ACK v4. Это не
подтверждает production archive, online согласованность blobs, RPO/RTO или
off-site retention.
Read-only historical endpoint восстанавливает точную версию из подходящего
snapshot и хвоста журнала ([ADR-036](adr/036-historical-version-read.md));
PostgreSQL и HTTP tests проверили v1/v2, fallback, ACL и неизменность head.
Исторические calculated results и UI истории ещё открыты.
Локальный release baseline на одной машине (5×1000 samples, PostgreSQL
durability on) дал худший p95 7.86 ms до ACK и 11.11 ms до второго клиента;
условия и пределы находятся в [отчёте](benchmarks/2026-09-29-phase3-local.md).
Это пока не официальный latency gate на закреплённом runner.
Canvas document render измерен отдельно в реальном Chromium на production
bundle: 20 reloads дали p95 64.4 ms от navigation и 18.8 ms после document GET,
107 DOM nodes для пустой книги 1,000×26. Это локальный предварительный baseline,
не gate для 100,000 заполненных ячеек; условия в
[отчёте](benchmarks/2026-09-29-render-local.md).
Синтетическая frontend projection на 100,000 ячеек (10,000×100 осей,
14.58 MB JSON) измерена отдельно: два runs по 20 reloads, худший p95
442.8 ms после document GET, максимум 107 DOM nodes. Persisted large-book
path этим не проверен; [отчёт](benchmarks/2026-09-29-render-sparse-local.md).
Создание domain-книги с 100,000 стабильных row IDs отдельно измерено Criterion:
два локальных runs дали интервалы 6.77–7.90 ms; память и snapshots этим не
измерялись. Criterion показал разницу со старым baseline, но причина без
контролируемого A/B неизвестна; [условия](benchmarks/2026-09-29-axis-allocation-local.md).
Вставка одной строки перед первой из 99,999 stable row IDs измерена отдельным
Criterion harness: 6.6972–7.2624 ms на локальном optimized domain candidate,
без БД, расчета и UI; [условия](benchmarks/2026-09-29-row-insertion-local.md).
Четвёртый срез фазы 6 ([ADR-046](adr/046-bounded-xlsx-worker-ir.md))
передаёт scalar IR из дочернего XLSX-процесса в ограниченном version-2 frame
до 64 MiB. Родитель повторно валидирует schema, координаты, порядок,
числа и лимиты до использования модели. CLI preview по-прежнему выводит
только счётчики; документ не публикуется. OS sandbox, compatibility report,
загрузка в книгу и export/reimport остаются открытыми.
Пятый срез ([ADR-047](adr/047-scalar-xlsx-export.md)) добавил запись
scalar IR в XLSX-пакет с обратным чтением двухлистного синтетического
fixture нашим reader. Это библиотечный round-trip, не пользовательский
экспорт и не Excel-verified interoperability; преобразование domain → IR,
аудит и выдача файла были открыты на момент ADR-047.
Шестой срез ([ADR-048](adr/048-owner-audited-document-xlsx-export.md))
добавляет owner-only HTTP-выдачу scalar XLSX из подтверждённой версии книги.
Append-only запись аудита с версией, размером и checksum фиксируется до
отправки ответа; viewer не может скачать файл. Проверены PostgreSQL storage
test и живой HTTP/WebSocket integration. Это ещё не import/edit/export/reimport:
стили, даты, merges, Excel golden fixtures и document import остаются открыты.
Седьмой узкий срез добавляет кнопку скачивания подтверждённого scalar XLSX в
интерфейсе. Она недоступна, пока есть неподтверждённые операции, и сообщает
viewer об отсутствии права на экспорт. Браузерный download проверяется при
DPR 1/2; импорт и полный phase-6 gate остаются открытыми.
Восьмой срез ([ADR-049](adr/049-atomic-local-xlsx-document-import.md))
публикует один scalar-лист через локальную dev-auth CLI. Parent валидирует
worker IR, сервер связывает формулы, а PostgreSQL одной транзакцией сохраняет
создание документа, owner permission, версии 1..N и все события. Живой
integration test проверяет импорт, HTTP replay и owner export; двухлистовой
fixture отвергается целиком. Публичная загрузка, OS sandbox, malware scan,
стили, compatibility report и полный import/edit/export/reimport gate открыты.
Локальный optimistic Canvas draw submission при удержанных WebSocket sends
измерен в Chromium: пять runs по 100 правок дали худший p95 14.9 ms при
неизменной серверной версии. Это узкий preliminary target, не измерение GPU
presentation или большой книги; см. [отчёт](benchmarks/2026-09-29-optimistic-local.md).

| Фаза | Минимальный рабочий срез                                                                                                                            | Gate                                                                                                                                                             |
| ---- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0    | Workspace, quality tooling, PostgreSQL migrations, Axum health/readiness, dev-auth boundary, protobuf handshake, пустой Canvas, local BlobStore, CI | Команды старта и quality checks работают; браузер получает Welcome и рисует сетку; BlobStore contract проходит; действительные ограничения окружения перечислены |
| 1    | Создать документ → SetCellValue через actor → durable CAS event → projection → reload/restart                                                       | Значение/IDs восстановлены из БД; duplicate не создает вторую версию; ACL и crash до/после commit протестированы                                                 |
| 2    | Formula text → parser/binder → incremental engine → server calculation + WASM preview                                                               | Fixtures и parity проходят; независимые формулы не пересчитываются; cycle дает typed value; chain 10k измерена                                                   |
| 3    | Два клиента, pending queue, subscriptions, reconnect/barrier, presence, revoke                                                                      | После canonical replay hashes совпадают; stale calc не применяется; slow consumer не блокирует документ; ACL epoch закрывает старые подписки                     |
| 4    | Structural insert/delete, reference transforms, copy/paste/batch, guarded compensating undo                                                         | Вставки детерминированы; удаленные IDs дают typed errors; undo не затирает чужую правку; cut/move проходят отдельный compatibility gate                          |
| 5    | Versioned binary snapshots, S3 adapter, activation state machine, recovery/history                                                                  | Snapshot+tail равен full replay; corrupted snapshot дает fallback; crash matrix и BlobStore S3 contracts проходят                                                |
| 6    | Изолированный XLSX reader → IR → document; export → reimport                                                                                        | Поддерживаемые значения/formulas/styles сохраняются; unsupported features явно отражены в report; hostile files ограничены                                       |

SortRange не входит в ранний MVP. Будущий отдельный срез реализует atomic permutation содержимого только выбранного прямоугольника; SortSheet рассматривается отдельно. Ранее предложенная сортировка полных строк под именем SortRange отменена.

## Объем фазы 0

1. Проверить доступность candidate versions, licenses и runtime; закрепить реальный toolchain и lock-файлы, сохранив отчет об отклонениях.
2. Создать только используемые domain/application/wire/storage/server crates, frontend workspace и quality commands.
3. Поднять PostgreSQL и выполнить migrations. Health проверяет процесс; readiness проверяет действительные зависимости и schema.
4. Проверить binary Hello/Welcome/Rejected между Rust и TS. Неизвестные versions/payloads отвергаются полностью. Origin/size/auth boundary работают до upgrade/обработки сообщения.
5. Показать пустую viewport Canvas-сетку. Не реализовывать editing, document commits, formulas или collaboration под видом skeleton.
6. Реализовать LocalFilesystemBlobStore с checksums, ограниченными keys, атомарной публикацией и tests. Не запускать S3 service.
7. Выполнить CI-equivalent commands и browser tests; перечислить реальные прошедшие/не запущенные checks в RUNBOOK.

Одна команда `just infra-up` должна поднимать PostgreSQL и применять migrations. Одна команда `just dev` должна запускать backend/frontend после подготовки dependencies. Linux-first команда допускает Docker/WSL на Windows; если host prerequisites недоступны, это blocker конкретного gate, а не повод объявить его пройденным.

## Измеримые gates первых фаз

**Phase 0:** clean migration succeeds; health 200; readiness 200 при доступной БД и 503 при потере БД с восстановлением после ее возвращения; invalid origin/version/frame rejected; Playwright Welcome + Canvas при DPR 1/2, keyboard focus и ограниченное число DOM nodes; production build без dev-auth; format, lint, typecheck, unit, build, protobuf generation/drift checks. First usable grid measured отдельно, initial target p95 <1 s после готовности данных. Shared CI не падает на этом latency threshold.

**Phase 1:** create/edit/reload/restart; 100 одинаковых retries → одна запись/версия; payload collision → IdempotencyConflict; два конкурентных CAS writers не пропускают versions; candidate failure до transaction не пишет event; lost ACK восстанавливается receipt+replay; viewer/чужой tenant не меняют документ. 100k sparse cells не создают 100k DOM nodes. Commit p95 <50 ms измеряется только на описанном durable reference стенде.

**Phase 2:** lexer/parser/coercion fixtures и snapshots; 100k bounded arbitrary parser inputs без panic; canonicalization idempotence; A1+B1, SUM/AVERAGE/MIN/MAX/IF; старые dependency edges удаляются; одна измененная формула из 10k независимых не считает остальные; chain 10k не переполняет stack; bounded range lookup измерен; native/browser общие fixtures и 1000 seeded workbooks дают одинаковый normalized result hash. Excel golden fixtures имеют provenance, неподтвержденные ожидания не называются Excel-verified.

**Phase 3:** 100 seeded сценариев по 1000 commands с duplicates/delays/disconnect/restart, hashes после replay совпадают; simultaneous same-property edits следуют server order; pending p95 <16 ms и доставка второму клиенту p95 <100 ms на фиксированном стенде; presence не входит в durable log; revoke прерывает новые payload sends; replay/live race покрыта тестом.

## Отложенные задачи с критериями завершения

| Задача           | Причина и зависимость                                  | Критерий завершения                                                                        |
| ---------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| AUTH-PROD        | Phase 0 dev auth не является production identity       | Выбран self-hosted provider, audited auth/CSRF/session integration до production           |
| CALC-CONTRACT    | Формулы не входят в phase 0                            | Подтверждены semantics и fixture corpus до фазы 2                                          |
| COLLAB-CONTRACT  | Протокол команд пока отсутствует                       | Подтверждены rebase/revoke/barrier contracts до фазы 3                                     |
| S3-CONTRACT      | Snapshots появятся только в фазе 5                     | Один BlobStore suite проходит filesystem, Garage candidate и российский S3                 |
| XLSX-ISOLATION   | Bounded package inspector появился без process sandbox | Worker process sandbox, resource limits и malicious corpus проходят Linux/Windows adapters |
| SORT-RECTANGLE   | Full-row sort нарушает ожидания выделенного диапазона  | Отдельный ADR, transformation fixtures и atomic rectangle tests перед добавлением          |
| HISTORY-UI       | Фаза 5 пока даёт только точное read-only HTTP-чтение   | Список версий и безопасный просмотр diff доступны в UI без изменения head                  |
| HISTORY-HTTP-ACL | HTTP body send не ограждён WebSocket delivery gate     | Тест гонки revoke/read доказывает отсутствие payload после завершённого revoke             |

Ни одна запись этой roadmap сама по себе не означает реализованную или протестированную возможность.
