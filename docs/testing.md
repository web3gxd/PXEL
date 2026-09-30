# Проверки и доказательства

ADR086 adds application history reduction, a paged real-PostgreSQL discovery
suite, metadata-only HTTP/ACL tests, browser reload/multi-tab tests and a
two-hard-kill recovery test. Version-window and payload-budget boundaries are
tested with actual persisted events. Frontend regressions cover stale responses,
uncertain receipts and unseen own edits hidden by coalesced foreign/system heads
or local ACKs. System-event provenance must be verified through the live actor,
not solely by constructing a HeadUpdate in an adapter unit test. Final stage
acceptance is limited to this locally verified history slice; RUNBOOK records
the commands, intermediate failures and final regression scope.

ADR085 coverage: domain axis_edit/lifecycle and storage-codec axis_tests exercise
exact restoration, malformed envelopes, snapshot3/legacy readers and activation
floors. Real PostgreSQL journal axis_tests cover own/foreign history, restored
range interiors, missing/forged context, stale Copy, size and touch budgets.
Browser axis-history.spec.ts covers four explicit axis actions at DPR1/2 with
foreign independent writes and Undo/Redo/reload. Recovery axis-history-restart
uses two genuine process kills on a dedicated port and compares content,
calculation hashes/results, stale rejection and late receipts. It exercises log
replay; it does not by itself prove snapshot recovery. Actual outcomes are in
RUNBOOK; ignored release harnesses are not automatically passed.

Этот документ задает проверки. Что действительно запускалось и с каким результатом, фиксируется в RUNBOOK; описание test suite не является свидетельством его прохождения.

## Phase 0

| Уровень       | Проверка                                                                                                                                           |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Static/build  | rustfmt, clippy, Rust tests/build; TS strict typecheck, ESLint, Prettier, Vitest, Vite build                                                       |
| Protocol      | Buf lint/generation drift; Rust/TS Hello/Welcome round-trip; unsupported protocol, missing payload, malformed/oversized frame controlled rejection |
| PostgreSQL    | Real migration clean start; checked SQL metadata verification; readiness success/failure/recovery, не mocked healthy response                      |
| BlobStore     | Binary put/get/head/delete, checksum mismatch, bad key/path escape, missing object, complete publication                                           |
| Browser       | Actual frontend/backend, WS Welcome, Canvas nonempty rendering at DPR 1/2, resize/scroll, bounded DOM footprint, keyboard focus, no console errors |
| Auth boundary | Development auth feature/runtime isolation; production build excludes dev auth; origin allowlist tested                                            |
| Entrypoints   | `just infra-up`, `just dev`, CI-equivalent quality command с настоящими prerequisite services                                                      |

Health 200 обозначает процесс, readiness 200 — готовность обязательных dependencies/schema. Проверять readiness 503 при недоступной БД и 200 после восстановления. Exact commands определяются фактическим skeleton и README; нельзя объявлять ненаписанную команду выполненной.

Phase 0 не проверяет сохранение ячейки, document actor, формулы, snapshots, XLSX или две редактирующие сессии. Эти функции не реализуются ради преждевременного green test. Native/WASM toolchain smoke проверяет доступность target; настоящее engine parity появляется с engine.

## Следующие уровни

Unit: lexer/parser/AST normalization/coercion/functions/reference transforms, stable identity, operation validation, ACL. Property tests: parser no panic; AST canonicalization idempotent; serialize/decode preserves operation; duplicate apply receipt idempotent; independent property writes commute; snapshot+tail equals full replay; native/WASM convergence; structural refs valid or REF.

Convergence virtual clients: seeded delays, duplicate/out-of-order delivery, reconnect, concurrent edits/inserts, deletion during edit, permissions revoke и restart. Sort сценарии добавляются только вместе с отдельно accepted rectangular sort. После полного canonical replay версии/cacheless hashes совпадают; result hashes сравниваются только для одинаковых completed context/version.

Текущий domain harness запускает в `just check` smoke 5 seeds × 300
канонических событий; `just convergence` явно запускает полный 100 seeds ×
1000 commands × 3 replicas. Клиенты получают повторы, задержки и сообщения
не по порядку, затем догоняют журнал; серверная projection несколько раз
строится заново из журнала. Это проверяет чистое применение существующих
SetCellValue/ClearCell, вставку и удаление stable-ID строк/столбцов, frozen
`CopyRange` обычных значений/пустот и cacheless hash. Для каждого seed
проверяется, что структурные и copy ветви действительно исполнены. Тест не
моделирует реальные WebSocket/БД, ACL, формулы в CopyRange или calculation patches; они требуют
отдельных интеграционных сценариев по мере появления соответствующего кода.

Отдельный настоящий WebSocket/PostgreSQL test выполняет пять смешанных
сценариев с двумя писателями, структурными вставками/удалениями, frozen
`CopyRange`, retry и reconnect. После непредставимого в wire v1 CopyRange
`ReplayComplete` требует projection resync; подключение после него получает
непрерывные структурные events и финальный hash. Это функциональное покрытие,
не 100×1000 случайных сетевых сценариев и не latency benchmark.

`pnpm test:network-convergence` запускает отдельный seeded сетевой harness на
реальном сервере/PostgreSQL: по умолчанию 3 документа × 100 команд. Два
WebSocket автора отправляют пары операций с воспроизводимой задержкой 0–2 мс;
часть пар пишет одно свойство одной ячейки. Harness проверяет серверный порядок,
идемпотентные receipts, live последовательность всех версий, переподключение
автора, 20 пропущенных подписчиком версий, replay без дыр и финальный hash
против авторизованной HTTP projection. Масштаб задают `PXEL_NETWORK_SEEDS`
(1–100) и `PXEL_NETWORK_COMMANDS` (чётное 2–1000); полный профиль — 100×1000.
Этот сценарий пока не перезапускает сервер между командами, не меняет ACL и
не включает формулы. Отдельный `tests/recovery/convergence-restart.test.mjs`
жёстко останавливает сервер после 50 конкурирующих записей двух клиентов,
проверяет полный replay и hash, идемпотентный retry, затем делает ещё две
конкурентные записи. Он входит в `pnpm test:recovery`, но не моделирует
случайные задержки и ACL revoke в том же restart-сценарии. Wall-clock тестов
не является p95 commit/delivery.

Локальный полный профиль 100×1000 прошёл 2026-09-29 за 1 310 127 ms на
Windows 11 Pro 10.0.26100 / Intel Core Ultra 9 185H / 15.61 GiB RAM,
PostgreSQL 18.6 на 127.0.0.1:55432. `synchronous_commit`, `fsync` и
`full_page_writes` были `on`; backend работал в debug build, клиент и БД
на одной машине. Это длительность всего correctness run, не распределение
latency отдельной операции и не cross-host production показатель.

Recovery matrix: до transaction; после CAS/INSERT до второго statement; после COMMIT до publish/ACK; candidate failure; two writers CAS; unknown commit outcome; snapshot UPLOADING/STORED/VERIFIED/ACTIVE boundaries; object upload до metadata; replay interruption/gap/corrupt event. Невалидный candidate не сохраняется, committed event не теряется.

Публикация snapshot имеет тест шести управляемых остановок на настоящих
PostgreSQL и LocalFilesystemBlobStore: после metadata INSERT, blob PUT,
STORED, VERIFIED, UPDATE внутри activation transaction и activation COMMIT.
Тест проверяет metadata, preferred pointer и восстановление из журнала или
ACTIVE snapshot. Это проверки границ исполнения, не симуляция потери питания;
orphan cleanup и реальный процессный crash harness остаются отдельной работой.

Исторический read использует подходящий snapshot `<=V` и только хвост до V.
SQLx-тест проверяет отсутствие snapshot, два ACTIVE поколения, удалённый
объект, точный hash, будущую версию, ACL и неизменность head. HTTP test
проверяет маршрут и JSON projection на настоящем сервере.

Уборка incomplete snapshots проверяет возрастной порог и bounded batch на
реальных PostgreSQL/LocalFilesystemBlobStore. Один test задерживает publisher
перед object put, запускает GC, затем продолжает publisher: он должен
получить `PublicationRetired`, не изменить preferred pointer и удалить
поздний объект. Повторный sweep должен убрать объект, созданный уже после
предыдущей попытки; ACTIVE snapshot остаётся доступным.

Formula differential corpus один для native/browser/WASM. Excel expected results имеют workbook/version/OS/locale/provenance и verified flag. Отсутствие Excel runtime dependency не запрещает ручную верификацию fixtures. Unknown/unsupported syntax coverage не заменяется фиктивным numerical fallback.

Fuzz targets появляются с parsers: formulas, protobuf decode, snapshot decode, references/ranges, ZIP/package/XML. Stable proptest — основной повседневный gate. Если libFuzzer требует nightly, отдельный pinned testing job и ADR/обоснование, без nightly product runtime.

## Performance harness

Сначала harness и correctness dataset, потом optimization. Записывать CPU/RAM/OS/disk, native vs Docker/WSL, container limits, DB/browser versions, release profile, worker counts, DPR/viewport, dataset size/seed, topology и warm/cold status. Никогда не подставлять вымышленные показатели.

Для commit targets обязательны `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`, warm pool и настоящая PostgreSQL. Разделить actor queue wait, validation, DB checkout, transaction duration, commit wait, broadcast enqueue и end-to-end ACK. Client/DB topology указывается явно. Измерение driver commit latency не объявляется точным временем физического fsync без отдельной instrumentation.

Начальный протокол latency: 100 warm-up, >=1000 measured samples, пять runs, p50/p95/p99. Render — 20 page loads, cold/warm assets отдельно. Official thresholds на закрепленном self-hosted стенде; shared CI запускает functional benchmark/regression smoke без жестких p95 gates.

`just bench-collab` запускается отдельно от CI на уже поднятом release-сервере
и реальной PostgreSQL: 5 новых документов, по 100 warm-up + 1000 measured
sequential edits. Он проверяет версии ACK/второго клиента и печатает p50/p95/p99
в JSON, не сохраняя значения ячеек. Для атрибуции сборки установите
`PXEL_BENCH_SERVER_PROFILE=release`. Последний проверенный локальный результат
и ограничения: [2026-09-29-phase3-local.md](benchmarks/2026-09-29-phase3-local.md).

`just bench-render` измеряет первый Canvas-кадр с загруженной projection через
browser Performance API в Chromium на production bundle. Последний локальный
замер: 20 reloads, p95 64.4 ms от navigation и 18.8 ms после ответа document
GET, 107 DOM nodes для пустой книги 1,000×26. Условия и ограничения:
[2026-09-29-render-local.md](benchmarks/2026-09-29-render-local.md).

`just bench-render-sparse` отдельно подменяет только document GET синтетической
projection на 100,000 заполненных ячеек. Два запуска по 20 reloads дали
худший p95 442.8 ms после ответа API и максимум 107 DOM nodes; это
frontend-only benchmark, не проверка durable large-document path.
[Условия](benchmarks/2026-09-29-render-sparse-local.md).

`just bench-optimistic` удерживает отправку WebSocket после подписки и измеряет
в реальном Chromium время от Enter до Canvas `fillText` для нового значения.
Пять запусков по 20 warm-up + 100 measured edits дали худший p95 14.9 ms;
серверная версия оставалась 1. Это submission draw call, не GPU presentation.
[Методика и ограничения](benchmarks/2026-09-29-optimistic-local.md).

`just stress-slow-reader` ставит TCP чтение подписчика на паузу и делает
512 durable commits по 30,005 bytes через другого клиента. Три локальных
прогона сохранили все 512 версий, а медленный socket оказался закрыт после
возобновления чтения. В третьем прогоне структурированный event показал
`reason=timeout`; переполнение очереди этим тестом не доказано.
[Отчёт](benchmarks/2026-09-29-slow-reader-local.md).

`just bench-recovery-storage` запускает отдельно от CI игнорируемый release
SQLx-test: 20 000 durable commits, verified snapshot на v10001, 10 000
событий хвоста и сравнение с полным authenticated journal replay. Fixture
создаётся до измерения; каждый sample проверяет domain hash. Два локальных
прогона и границы интерпретации: [отчёт](benchmarks/2026-09-29-snapshot-replay-persisted-local.md).

| Цель                                      | Target / measurement                                     |
| ----------------------------------------- | -------------------------------------------------------- |
| First usable grid после готовности данных | p95 <1 s                                                 |
| Optimistic local update                   | p95 <16 ms                                               |
| Simple durable commit                     | p95 <50 ms на описанном стенде                           |
| Second local client visibility            | p95 <100 ms                                              |
| 10 000 dependency chain                   | Измерить время/allocations, threshold после baseline     |
| 100 000 sparse cells                      | Нет 100 000 DOM nodes; record memory/open/render         |
| Snapshot + 10 000 tail events             | Отдельный benchmark и equality с full replay             |
| JS/WASM boundary                          | Encode/decode/crossing отдельно от engine                |
| Axis ID allocation                        | Memory и snapshot size для 1k/100k rows                  |
| Reverse range index                       | Lookup/dirty propagation с множеством overlapping ranges |

## Правила отчета

`just formula-parity` также запускает отдельный Move corpus через native runner
и реальный browser Worker:209книг/418переносов, полный output/hash/renderedtext.
`just bench-move-preview` измеряет shared planner+private candidate+incremental
calculation на100000initialoccupiedcells и1000-cell Move. Он требует готового
WASM (`node scripts/wasm.mjs`), проверяет реальные лимиты и отдельно считает
encode/call/decode/Workerroundtrip. [Условия и результаты](benchmarks/2026-09-30-move-preview-local.md).
Developer percentiles не заменяют fixed-runner
acceptance; серверный commit и GPU presentation этим harness не измеряются.

Failure не удаляет тест и не ослабляет assertion без причины. Root cause исправляется либо явно записывается blocker с воспроизведением. Skipped и not-run отличаются от passed. Нельзя назвать CI green, если прогон был только локальным; локальный CI-equivalent run так и называется. Combined toolchain becomes tested baseline только после соответствующих реальных builds/integration gates, а не metadata registry lookup.
