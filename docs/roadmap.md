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
100×1000×3 прошёл. Полный gate фазы 3 ещё не закрыт: нужны сетевые/ACL и
структурные convergence сценарии и измерения latency. Outbound queue с
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
100×1000 сетевых сценариев и не закрывает phase 3 gate.
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

| Задача          | Причина и зависимость                                 | Критерий завершения                                                                        |
| --------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| AUTH-PROD       | Phase 0 dev auth не является production identity      | Выбран self-hosted provider, audited auth/CSRF/session integration до production           |
| CALC-CONTRACT   | Формулы не входят в phase 0                           | Подтверждены semantics и fixture corpus до фазы 2                                          |
| COLLAB-CONTRACT | Протокол команд пока отсутствует                      | Подтверждены rebase/revoke/barrier contracts до фазы 3                                     |
| S3-CONTRACT     | Snapshots появятся только в фазе 5                    | Один BlobStore suite проходит filesystem, Garage candidate и российский S3                 |
| XLSX-ISOLATION  | Недоверенный parser появится в фазе 6                 | Worker process sandbox, resource limits и malicious corpus проходят Linux/Windows adapters |
| SORT-RECTANGLE  | Full-row sort нарушает ожидания выделенного диапазона | Отдельный ADR, transformation fixtures и atomic rectangle tests перед добавлением          |

Ни одна запись этой roadmap сама по себе не означает реализованную или протестированную возможность.
