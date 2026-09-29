# Проверки и доказательства

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
SetCellValue/ClearCell и cacheless hash. Тест не моделирует реальные
WebSocket/БД, ACL, структурные команды или calculation patches; они требуют
отдельных интеграционных сценариев по мере появления соответствующего кода.

Recovery matrix: до transaction; после CAS/INSERT до второго statement; после COMMIT до publish/ACK; candidate failure; two writers CAS; unknown commit outcome; snapshot UPLOADING/STORED/VERIFIED/ACTIVE boundaries; object upload до metadata; replay interruption/gap/corrupt event. Невалидный candidate не сохраняется, committed event не теряется.

Formula differential corpus один для native/browser/WASM. Excel expected results имеют workbook/version/OS/locale/provenance и verified flag. Отсутствие Excel runtime dependency не запрещает ручную верификацию fixtures. Unknown/unsupported syntax coverage не заменяется фиктивным numerical fallback.

Fuzz targets появляются с parsers: formulas, protobuf decode, snapshot decode, references/ranges, ZIP/package/XML. Stable proptest — основной повседневный gate. Если libFuzzer требует nightly, отдельный pinned testing job и ADR/обоснование, без nightly product runtime.

## Performance harness

Сначала harness и correctness dataset, потом optimization. Записывать CPU/RAM/OS/disk, native vs Docker/WSL, container limits, DB/browser versions, release profile, worker counts, DPR/viewport, dataset size/seed, topology и warm/cold status. Никогда не подставлять вымышленные показатели.

Для commit targets обязательны `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`, warm pool и настоящая PostgreSQL. Разделить actor queue wait, validation, DB checkout, transaction duration, commit wait, broadcast enqueue и end-to-end ACK. Client/DB topology указывается явно. Измерение driver commit latency не объявляется точным временем физического fsync без отдельной instrumentation.

Начальный протокол latency: 100 warm-up, >=1000 measured samples, пять runs, p50/p95/p99. Render — 20 page loads, cold/warm assets отдельно. Official thresholds на закрепленном self-hosted стенде; shared CI запускает functional benchmark/regression smoke без жестких p95 gates.

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

Failure не удаляет тест и не ослабляет assertion без причины. Root cause исправляется либо явно записывается blocker с воспроизведением. Skipped и not-run отличаются от passed. Нельзя назвать CI green, если прогон был только локальным; локальный CI-equivalent run так и называется. Combined toolchain becomes tested baseline только после соответствующих реальных builds/integration gates, а не metadata registry lookup.
