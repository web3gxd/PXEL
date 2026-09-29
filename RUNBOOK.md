# PXEL — RUNBOOK

## 2026-09-29 — замер создания оси на 100,000 стабильных строк

Перед замером проверены ADR-002, существующий `workbook` Criterion harness и
другие benchmark reports. Без изменения production-кода выполнены два runs
`cargo bench --locked -p spreadsheet-domain --features test-support --bench
workbook axis_allocation -- --sample-size 10`. Первый проход с idle dev server
дал 7.1798–7.9022 ms для 100,000 строк и 30.625–33.905 µs для 1,000 строк;
после остановки сервера второй дал 6.7701–7.5430 ms и 32.594–34.945 µs.
Это 95% Criterion estimates, не UI p95. Criterion отметил отличие от более
старого локального baseline, но два текущих runs статистически не различались
для 100,000 строк (`p=0.10`). Причина старой разницы не установлена; код оси
не оптимизировался. Условия и границы записаны в
[отчёте](docs/benchmarks/2026-09-29-axis-allocation-local.md). Memory footprint,
persisted snapshot и controlled A/B не измерялись.

---

## 2026-09-29 — сетевой convergence сценарий со структурной операцией

Перед срезом прочитаны AGENTS, roadmap, ADR-019 и текущий WebSocket test
harness. Цель — проверить одну смешанную последовательность через живые
WebSocket/PostgreSQL, а не только domain projection. Инварианты: монотонные
версии, идемпотентный retry, устойчивость stable IDs при `ExtendRows`,
reconnect и совпадение hash последнего canonical event с HTTP projection.

В `tests/integration/document.test.mjs` добавлены пять сценариев с фиксированным
xorshift seed: по два конкурентных начальных писателя, 12 дальнейших команд,
один retry, append двух строк, разрыв одного соединения, поздняя подписка и
проверка полного ordered replay. Задержка 30 ms между командами удерживает
тест ниже локального dev rate limit; это параметр теста, не production latency.
Focused run прошёл 1/1 за 3.93 s, полный `pnpm test:integration` прошёл 30/30
за 9.78 s. Это не 100×1000 сетевых сценариев, не permission-revocation race,
не restart между командами и не cross-host stress; phase 3 gate остаётся открыт.
Презентация и roadmap обновлены без объявления фазы завершённой.

---

## 2026-09-29 — первый структурный срез: автоматическое расширение строк

Дополнительный regression test на существующую формулу `=A1` подтвердил, что
добавление строки в конце сохраняет bound target и рассчитанное значение 17;
полный повтор `pnpm test:integration` прошёл 29/29.

Перед этапом прочитаны AGENTS, roadmap, ADR-002, collaboration contract и
код domain/actor/journal/Canvas. Цель — провести одну настоящую структурную
операцию через durable commit, replay, два клиента и браузер, не меняя смысл
существующих A1-ссылок. Инварианты: stable IDs, CAS/идемпотентность, отсутствие
пустых Cell records, confirmed state отдельно от pending и строгий отказ от
устаревшего formula binding после изменения оси. Риск — stale formula base и
дорогая перезагрузка projection; оба ограничения зафиксированы в ADR-019.

Добавлен `ExtendRows`: 1–1,000 новых UUIDv7 row IDs за команду, максимум
100,000 строк; actor сериализует concurrent append, PostgreSQL хранит event
schema 4, protobuf wire v1 расширен аддитивно. Новая миграция разрешает schema
4 для user events. Domain hash включает порядок ID, replay восстанавливает
`last_axis_change_version`. Старая formula base получает typed
`RebaseWindowExceeded`. Canvas автоматически отправляет команду у последних
20 строк; после подтверждения и удалённого структурного hint браузер берет
авторизованную projection. Это не optimistic axis preview.

Изменены domain/formula/collab/storage-codec/storage/server, оба protobuf
контракта и generated TS, frontend store/Canvas, migration, integration,
recovery и browser tests; обновлены ADR-019, roadmap, collaboration contract и
презентация. Первый `just check` остановился на Rust formatting; `cargo fmt`
исправил форматирование без изменения поведения. Первая проверка integration
до миграции и расширения диапазона schema в journal отклоняла event; оба
несоответствия исправлены, тест сохранен. В итоговой проверке `just check`
прошёл exit 0: Rust fmt/Clippy/tests/build, wasm32, proto drift, WASM build,
Prettier, ESLint, TypeScript, 55 Vitest и Vite build. Живой
`pnpm test:integration` прошёл 28/28, `pnpm test:recovery` прошёл 1/1 после
остановки локального сервера, `pnpm test:e2e` прошёл 20/20 при DPR 1/2.
Первый повтор recovery не дошёл до теста: Windows запретил линкеру заменить
исполняемый файл работающего dev-сервера. Неверная root-команда
`pnpm presentation:check` была заменена на
`pnpm --filter @pxel/web presentation:check`; проверка прошла 10 слайдов на
desktop/mobile. Сетевой
cross-host, официальный release latency threshold и Linux/Compose CI в этом
срезе не запускались. Полный gate фазы 4 не закрыт: arbitrary insert/delete,
ссылочные преобразования, batch copy/paste и guarded undo впереди.

---

## 2026-09-29 — Canvas с 100,000 синтетических ячеек

Перед срезом прочитаны AGENTS, roadmap, testing, существующий Canvas
benchmark и код DocumentStore/renderer. Цель — измерить реальную загрузку
production frontend с большим разреженным набором без создания DOM-узла на
ячейку. Инвариант: этот frontend fixture не выдаётся за persisted canonical
document; state hash синтетического ответа не соответствует содержимому.

Добавлены `tests/performance/render-sparse.mjs`, `just bench-render-sparse`
и [отчёт](docs/benchmarks/2026-09-29-render-sparse-local.md). Browser route
подменяет только document GET ответом на 14.58 MB: 10,000 row IDs, 100 column
IDs, 100,000 numeric cells. Production bundle, Canvas и frontend state
реальные. Два запуска по 1 first load + 20 reloads прошли в Chromium 153:
худший p95 442.8 ms после responseEnd, navigation p95 820.7 ms, максимум
107 DOM nodes. Один p99 navigation достиг 1010 ms; он явно записан.
Persisted 100k operations, memory, cross-host и frame presentation ещё не
измерены. Roadmap и презентация обновлены.
`pnpm exec prettier --write` применён к harness/report/slides;
`presentation:check` прошёл 10 слайдов на 1440×900 и 390×844,
`pnpm format:check` и `git diff --check` прошли. Полный `just check` после
добавления только benchmark/docs не повторялся; предыдущий полный прогон
успешен.

---

## 2026-09-29 — причина отключения медленного WebSocket клиента

Перед срезом перечитаны AGENTS, roadmap, ADR-018 и outbound writer. Цель —
различить socket error и send timeout без document content в логах.
`apps/server/src/outbound.rs` теперь пишет только `outbound_send_failure`
и константный `reason=timeout|socket_error`; ADR-018 и отчёт stress обновлены.
`cargo fmt --all`, server Clippy (`--all-targets --all-features -D warnings`)
и `cargo build --release --locked -p pxel-server --features dev-auth` прошли.
Третий `just stress-slow-reader` прошёл 512/512 commits, head 513, socket
закрылся; сервер выдал `outbound_send_failure reason=timeout` во время
нагрузки, затем `socket_error` после cleanup. Переполнение byte queue в
реальном socket этим не доказано. На обложке презентации статус прямо назван
живым инженерным отчётом: фаза 3 в работе, фазы 4–6 впереди.
После изменений `pnpm test:integration` прошёл 27/27 против release server;
`presentation:check` прошёл 10 слайдов на desktop/mobile; `just check`
завершился exit 0 (fmt, оба Clippy, Rust tests/build, wasm32, protobuf
drift, WASM build, Prettier, ESLint, TS, 53 Vitest, Vite build).

---

## 2026-09-29 — медленный подписчик не блокирует документ

Перед срезом прочитаны AGENTS, roadmap, ADR-018, текущие WebSocket outbound
queue, heartbeat и integration helpers. Цель — проверить реальным TCP pause,
что медленный получатель не останавливает durable writer. Инварианты: каждое
ACK writer имеет следующую версию, PostgreSQL head достигает ожидаемого
значения, медленный socket не удерживает document actor.

Добавлены `tests/performance/slow-reader.test.mjs`, `just stress-slow-reader`
и [отчёт](docs/benchmarks/2026-09-29-slow-reader-local.md). Два запуска против
release сервера и реальной PostgreSQL прошли: 512/512 последовательных
коммитов по 30,005 bytes, head 513, paused socket закрыт после resume,
19.57/19.17 s на Windows/Ultra 9 185H. Команды разделены 22 ms, чтобы не
сработал глобальный dev limiter 64/s. Логи не различают queue overflow и
send timeout; заявляется лишь подтверждённое поведение. Cross-host,
queue-depth telemetry и multiple-slow-client stress не проверены.
После форматирования `presentation:check` прошёл 10 слайдов на desktop/mobile,
`pnpm format:check` и `git diff --check` прошли. Полный `just check` в этом
документационно-тестовом срезе повторно не запускался; последний полный
прогон в предыдущем срезе был успешным.

---

## 2026-09-29 — браузерное восстановление после разрыва связи

Перед срезом прочитаны AGENTS, roadmap, collaboration contract, `useSession`
и текущие Playwright tests. Цель — проверить сходимость двух реальных вкладок,
когда одна пропускает правку и затем переподключается. Инвариант: отключенная
вкладка не получает live event до восстановления; после него видит canonical
значение. Тест не утверждает, что догон происходит только через operation
replay: клиент может выполнить авторизованный projection refresh.

В `apps/web/e2e/document.spec.ts` добавлен сценарий для DPR 1/2 с отдельным
контекстом и общей сессией. Test-only WebSocket proxy закрывает открытые
сокеты до перехода context в offline; после удаленной правки context
возвращается online и проверяет значение. Первые попытки выявили две причины
нестабильности harness: Chromium offline до socket.close не гарантировал
ожидаемый `onclose`, а 11 параллельных workers вызывали HTTP 429 от
глобального dev session/WS limiter 64/s. Последовательность разрыва
исправлена; `playwright.config.ts` ограничен четырьмя workers. Для диагностики
старый WASM E2E теперь указывает HTTP status при неуспешном чтении.

После исправления три focused runs прошли 2/2 при DPR 1/2; два полных
`pnpm test:e2e` прошли 18/18. До исправления были реальные красные прогоны,
тесты не удалялись и assertions не ослаблялись. Сетевой slow-reader stress и
массовые randomized browser scenarios пока не выполнены.
`presentation:check` прошёл 10 слайдов на 1440×900 и 390×844. `just check`
завершился exit 0: Rust fmt/Clippy/tests/build, wasm32, protobuf drift,
WASM build, Prettier, ESLint, TS, 53 Vitest и Vite build.

---

## 2026-09-29 — конкурентная запись одного свойства

Перед срезом прочитаны AGENTS, roadmap, collaboration contract и действующие
integration helpers. Цель — проверить server-order semantics при двух
одновременных записях в одну ячейку из двух WebSocket-сессий на одинаковой
base_version. Инварианты: обе принятые команды имеют последовательные
версии; значение в canonical projection и replay определяется событием с
последней версией, а не временем ответа клиентам.

Добавлен живой `tests/integration/document.test.mjs` сценарий: два клиента
параллельно отправляют разные значения, проверяются версии 2/3, состояние
HTTP projection и late-subscriber replay. `pnpm test:integration` против
release backend и реальной PostgreSQL прошёл 27/27. Структурных операций,
сетевых задержек и server restart этот конкретный сценарий не покрывает.

---

## 2026-09-29 — optimistic Canvas latency без серверного подтверждения

Перед срезом прочитаны AGENTS, roadmap, testing, текущие DocumentStore,
Canvas renderer и E2E сценарии. Цель — проверить локальную задержку именно
pending overlay до подтверждения сервером. Инварианты: команда не достигает
сервера во время измерения; подтвержденная версия остаётся неизменной;
тайминг берется из browser clock между Enter и Canvas draw submission.

Добавлен `tests/performance/optimistic.mjs` и `just bench-optimistic`. Harness
создаёт документ, дожидается подписки, удерживает WebSocket sends и делает
20 warm-up + 100 measured edits через formula bar. Первая реализация harness
ошибочно обрезала UUIDv7 и получила HTTP 400 `invalid_identity`; генератор
исправлен. После этого один probe run и пять независимых серий прошли. В пяти
сериях p95: 8.8/9.1/8.8/14.9/8.6 ms; серверная версия 1 проверена после
каждой серии. Результат и пределы в
[отчёте](docs/benchmarks/2026-09-29-optimistic-local.md), отражены в testing,
roadmap и презентации. Это `fillText` submission, не момент презентации кадра
GPU; большая книга и фиксированный runner ещё открыты.
`pnpm exec prettier --write` применён к harness/report; presentation check
прошёл 10 слайдов на desktop/mobile. `just check` завершился exit 0: Rust
fmt/Clippy/tests/build, wasm32 check, protobuf drift, WASM build, Prettier,
ESLint, TS, 53 Vitest, Vite build. E2E браузера в этом срезе отдельно не
запускался; предыдущий срез проверил 16/16 против той же production bundle.

---

## 2026-09-29 — замер первого Canvas-кадра с загруженной книгой

Перед срезом прочитаны AGENTS, roadmap, testing, текущий Canvas renderer и
phase-3 benchmark. Цель — измерить первый полезный кадр после загрузки
projection, сохранив презентацию как проверяемый отчёт. Инвариант: маркер
ставится только после успешного `drawGrid` для конкретного document ID;
измерение не должно подменять проверку большой sparse книги.

В `CanvasGrid.tsx` добавлен `pxel:document-grid-ready`, в
`tests/performance/render.mjs` — браузерный harness; `just bench-render`
запускает 1 первый load и 20 reloads против production Vite bundle и release
backend. Условия и реальные числа записаны в
[отчёте](docs/benchmarks/2026-09-29-render-local.md), отражены в testing,
roadmap и презентации. В последнем прогоне Chromium 153 на Windows/Ultra 9
185H дал p95 64.4 ms от navigation и 18.8 ms после document GET, максимум
107 DOM nodes для пустой книги 1,000×26. `pnpm build` прошёл; два запуска
`just bench-render` прошли, второй — после добавления viewport/DPR metadata.
Первый `just check` нашёл только форматирование benchmark Markdown; после
`prettier --write` повторный `just check` завершился exit 0 (Rust fmt,
Clippy, workspace tests/build, wasm32, protobuf drift, WASM build, Prettier,
ESLint, TS, 53 Vitest и Vite). `pnpm test:e2e` против release backend/production
bundle прошёл 16/16 при DPR 1/2; presentation check прошёл 10 слайдов на
1440×900 и 390×844. Chrome DevTools MCP не ответил в отведённое время;
проверка выполнена через настоящий Playwright Chromium.
Большая sparse книга, optimistic input p95 и fixed runner пока не измерены.

---

## 2026-09-29 — локальный benchmark ACK и доставки второму клиенту

Перед срезом прочитаны AGENTS, roadmap, testing, ADR-018 и текущий WS/actor
код. Цель — получить измеряемый end-to-end baseline фазы 3 на durable
PostgreSQL, не выдавая локальную цифру за SLA. Инварианты: каждый sample
подтверждает новую версию и тот же canonical head у второго клиента;
разогрев и измерения разделены; никакие значения ячеек не пишутся в отчёт.

Добавлены `tests/performance/collaboration.test.mjs`, `just bench-collab`,
[полный отчёт](docs/benchmarks/2026-09-29-phase3-local.md), пояснение в
testing/README и слайд с фактическим p95. Harness создаёт новый документ
на каждый из 5 прогонов, делает 100 warm-up и 1000 measured последовательных
перезаписей одной числовой ячейки, меряет время до `OperationCommitted` и до
`DocumentHeadChanged` у второго WebSocket-клиента. Сервер запущен через
`cargo run --release --locked -p pxel-server --features dev-auth`; Node и
сервер на loopback, PostgreSQL 18.6 нативно на `127.0.0.1:55432`.
Реальные `SHOW` подтвердили `synchronous_commit=on`, `fsync=on`,
`full_page_writes=on`. Host: Windows 10.0.26100, Intel Ultra 9 185H,
22 logical CPUs, 15.61 GiB RAM, Samsung MZAL81T0HDLB-00BL2, Node 24.18.0.

`just bench-collab` прошёл 1/1 за 33.55 s. p95 ACK по пяти runs:
4.61/5.73/5.55/5.58/7.86 ms; p95 второй клиент:
6.48/8.29/7.88/7.88/11.11 ms. Хуже всего 7.86 и 11.11 ms соответственно;
локальные targets <50/<100 ms на этом узком dataset выполнены. После
добавления безопасной топологии в JSON отчёт короткий smoke 1×10/2 warm-up
прошёл 1/1; он не заменяет полный замер выше. `just check` после изменений
завершился exit 0 (fmt, Clippy, Rust tests/build, wasm32, protobuf drift,
Prettier, ESLint, TS, 53 Vitest, Vite build), presentation check прошёл
10 слайдов на 1440×900 и 390×844. Это не
разделяет actor queue wait, DB checkout, transaction и fsync и не проверяет
большую книгу, распределённую сеть или concurrent writers. Optimistic UI p95
и first grid render p95 ещё не измерены; phase 3 gate остаётся открытым.

---

## 2026-09-29 — byte-bounded WebSocket writer (фаза 3, срез)

Перед срезом прочитаны AGENTS, roadmap, ADR-015/016/017, server WebSocket,
collaboration и текущие интеграционные тесты. Цель — ограничить исходящие
кадры медленного клиента по количеству и сумме байт без блокировки document
actor. Инварианты: queued payload не считается отправленным; перед физической
отправкой проверяются ACL, session и исходный `access_epoch` под delivery gate;
отзыв прав после COMMIT запрещает дальнейшую передачу старой подписке.

После Welcome WebSocket разделён на reader и writer task. У каждого сокета
bounded `mpsc` на 160 кадров и semaphore budget 2 MiB payload bytes.
Admission неблокирующий: overflow возвращает false, connection закрывается,
а tracing получает только `outbound_queue_overflow`/тип лимита без содержимого.
Writer выполняет финальную авторизацию и держит read gate до завершения
физической отправки с таймаутом 2 s. Writer failure сигналит reader;
завершение соединения ограничено 2 s и затем aborts writer. Replay cap 128
events/1 MiB умещается в очереди. Добавлен `futures-util` из уже закреплённого
workspace, ADR-018, обновлены ADR-016, collaboration, roadmap, README и
презентация.

Проверено: unit tests отдельно переполняют byte budget и frame count; после
рефакторинга `cargo clippy -p pxel-server --all-targets --all-features` прошёл,
живой `pnpm test:integration` — 26/26 (replay, ACL revoke, calculation,
presence), `pnpm test:e2e` — 16/16 при DPR 1/2, `pnpm test:recovery` — 1/1.
Security review нашёл возможный путь отправки committed/receipt без ACL gate,
если предварительная проверка не дала ticket, а command обработался успешно;
теперь без ticket передаются только rejected/protocol error, иначе сокет
закрывается для replay. После исправления повторный live integration — 26/26.
Первый полный `just check` остановился на Prettier для обновлённого слайда;
после форматирования повторный `just check` завершился exit 0: fmt, оба
Clippy режима, workspace Rust tests/build, wasm32, protobuf drift,
Prettier, ESLint, TS, 53 Vitest и Vite build. Presentation check после
форматирования прошёл 10 слайдов на 1440×900 и 390×844.
Kernel-level slow-reader test, queue depth histogram, p95 latency и
cross-process fanout в этом срезе не проверены; фаза 3 остаётся открытой.

---

## 2026-09-29 — seeded domain convergence harness (фаза 3, срез)

Перед срезом прочитаны AGENTS, roadmap, ADR-014/015/016/017, testing и
текущая domain/replay модель. Цель — воспроизводимо проверить, что три
виртуальных клиента после повторов, пропусков, доставки не по порядку и
reconnect получают одинаковый cacheless domain hash из канонического
журнала. Инварианты: серверный порядок единственный; duplicate не меняет
версию; клиент не применяет версию с пропуском; восстановление authoritative
projection из log дает тот же hash.

Добавлен `crates/spreadsheet-domain/tests/convergence.rs`: 100 фиксированных
seed × 1000 SetCellValue/ClearCell commands × 3 replicas, полное восстановление
серверной projection через каждые 250 событий и финальный replay клиентов.
Порядок доставки, повторы и интервалы отключения порождаются детерминированным
PRNG. В обычном `cargo test`/`just check` работает короткий smoke 5×300 с
одним восстановлением; полный тест запускается `just convergence`. Обновлены
`justfile`, testing, roadmap и презентация. Это проверка модели, а не
WebSocket/PostgreSQL end-to-end и не performance benchmark.

Проверено: первый прямой полный прогон 100×1000 прошёл за 60.084 s, затем
после выделения smoke/full `cargo test --locked -p spreadsheet-domain --test
convergence -- --nocapture` прошёл smoke за 586 ms, а `just convergence`
повторно прошёл full за 60.073 s. Условия: Windows 10.0.26100, debug test
profile, локальный Intel Ultra 9 185H; это wall time теста, не latency p95.
Первый `just check` остановился на новом test-only Clippy
`manual_is_multiple_of`; выражения заменены на `is_multiple_of` без изменения
сценариев. Повторный `just check` завершился exit 0: fmt, оба Clippy режима,
workspace Rust tests/build, wasm32 check, protobuf drift, Prettier, ESLint,
TypeScript, 53 Vitest и production Vite build. Проверка презентации прошла
10 слайдов на 1440×900 и 390×844. Сетевые
задержки, ACL revoke, структурные операции, formula patches и реальные
server process restarts этим harness не покрыты; для них остаются отдельные
сценарии. Фаза 3 не закрыта.

---

## 2026-09-29 — ephemeral presence и удалённое выделение (фаза 3, срез)

Перед срезом прочитаны AGENTS, roadmap, ADR-014/015/016, collaboration,
wire/session, server subscription и web store/Canvas. Цель — показать
удалённое выделение двум пользователям, не превращая движение курсора в
каноническую операцию. Инварианты: document version и state hash не меняются;
отправка требует текущего ACL/epoch под send gate; сообщения ограничены;
disconnect удаляет presence; данные документа не попадают в логи.

Wire v1 расширен `SetPresence` и `PresenceSnapshot` с полными stable ID
anchor/focus. Сервер проверяет форму, sheet consistency, подписку, session и
ACL epoch; process-local directory держит максимум 64 peers на документ,
coalescing watch, лимит 20 обновлений/с на сокет и не более 10 рассылок/с
на документ с отложенной доставкой финального положения. Snapshot уходит через
тот же ACL delivery gate. Клиент проверяет ID относительно текущих осей,
рисует видимые удалённые выделения на Canvas и показывает число участников.
При disconnect, revoke и смене документа присутствие очищается. Созданы
ADR-017, wire/client/server tests, браузерный сценарий двух разных аккаунтов,
обновлены README, collaboration, roadmap и презентация. Скрипт recovery теперь
сам собирает бинарник с `dev-auth` и читает локальный DB URL без его вывода.

Проверено: Rust wire/server unit tests, `pnpm test` 53/53, живой targeted
presence integration, полный `pnpm test:integration` 26/26, полный `just check`
с exit 0 (fmt, Clippy, workspace tests/build, wasm32, protobuf drift,
Prettier, ESLint, TS, Vitest, Vite build). После добавления браузерного
сценария `pnpm test:e2e` прошёл 16/16 при DPR 1/2; презентация прошла 10
слайдов на 1440×900 и 390×844. `pnpm test:recovery` прошёл 1/1 после
исправления harness. Его первые два запуска завершились до readiness:
исполняемый файл после `just check` был собран без `dev-auth`; причина
подтверждена ручным запуском, теперь recovery script сам собирает feature.
После последней правки README/документации и client disconnect полный
`just check` повторно завершился с exit 0. Security review дополнительно
обнаружил возможное умножение частоты рассылок на число участников; добавлен
aggregate doc throttle и тест coalesced trailing update. После этой правки
повторно прошли `just check` и живой `pnpm test:integration` 26/26; новый
браузерный сценарий при DPR 1/2 прошёл 2/2. Полный браузерный suite 16/16
был запущен до aggregate throttle; остальных UI-изменений после него не было.
Latency p95 и визуальный pixel diff
удалённой рамки не измерялись.

Ограничения: presence действует только в одном процессе; сервер проверяет
форму stable IDs, а не их актуальность в workbook на каждое движение;
неизвестные IDs отбрасывает клиент. Выделение сейчас передаёт anchor и focus,
но UI использует одну активную ячейку. Dedicated outbound byte queue,
membership revoke, seeded convergence stress и latency gate фазы 3 открыты.
Linux/Compose/remote CI и фазы 4–6 в этом срезе не проверялись.

---

## 2026-09-29 — document ACL epoch и WebSocket payload send gate

Перед срезом прочитаны AGENTS, roadmap, ADR-014/015, collaboration,
threat model, текущие storage/actor/WebSocket/auth/migrations и тесты.
Цель фазы 3: владелец может дать существующему пользователю editor/viewer
доступ и отозвать его так, чтобы после durable ответа об отзыве старый
подписчик не начинал новых document payload sends. Инварианты: PostgreSQL
authoritative, ACL изменение не входит в authored operation log, не повышает
document content version, `access_epoch` и append-only audit атомарны;
отправка проверяет server-issued principal и исходный epoch под gate.

Добавлены `POST /api/documents/{id}/permissions` с UUIDv7 change_id и
grant/revoke, две миграции для append-only `audit_events` schema 1,
идемпотентная owner-only транзакция изменения прав и `access_epoch`, а также
process-local per-document delivery gate. Grant существующего user при
необходимости добавляет membership в workspace; ACL конкретного документа
по-прежнему обязателен. `GET /api/dev/identity` и разрешенный workspace
header для dev login позволяют проверить двух разных пользователей локально;
production dev-auth остается выключенным. WebSocket replay, calculation и
command ACK теперь держат read gate от финальной DB ACL/epoch проверки до
bounded send. ACL transaction держит write gate; после COMMIT публикует epoch,
что закрывает старые подписки. Receipt повторного change_id не сигналит новый
epoch. Weak registry не удерживает неактивные gates. Созданы ADR-016,
документация, README, SQLx metadata, интеграционный и recovery сценарии;
презентация отражает только этот проверенный срез.

Проверено: `cargo sqlx migrate run` применил миграции 202609290002/003;
storage test доказал grant→editor→revoke, один audit record на intent,
collision, запрет изменения owner и append-only trigger. Gate unit test
доказал, что write ждет in-flight read. Live targeted integration и полный
`pnpm test:integration` прошли 25/25: два разных аккаунта, viewer не пишет,
replay/live update, owner-only ACL, повтор/коллизия, subscription close после
revoke до следующей правки. `pnpm test:recovery` прошёл 1/1 с hard restarts
после grant/revoke; `pnpm test:e2e` прошёл 14/14. `cargo sqlx prepare` обновил
offline metadata; `SQLX_OFFLINE=true cargo build --locked --workspace` и
`just sqlx-check` прошли. Первый `just check` остановился на Clippy
too_many_arguments после расширения replay API; общий DocumentDelivery
устранил это без lint suppression. После рефакторинга Rust Clippy/test/build,
wasm32 check, protobuf drift, Prettier, ESLint, TS, 52 Vitest и Vite build
прошли полным `just check` с exit 0. После последнего replay API-рефакторинга
повторно прошли живые `pnpm test:integration` 25/25, `pnpm test:recovery`
1/1 и `pnpm test:e2e` 14/14. Проверка презентации прошла 10 слайдов,
навигацию и layout 1440×900/390×844. После дополнительной проверки
`rows_affected`/owner invariant и документации финальный `just check`
повторно завершился с exit 0. Ни один latency p95 в этом срезе не измерялся.

Ограничения: gate действует в одном server process; direct SQL вне control
lane, membership revocation, presence, dedicated outbound byte queue,
convergence stress и production auth не реализованы. Sharing panel в UI
пока нет. Linux/Compose/remote CI не проверялись в этом срезе. Фаза 3 не
закрыта.

---

## 2026-09-29 — локальный функциональный gate базовой фазы 2

Перед фиксацией gate повторно прочитаны AGENTS, roadmap и критерии фазы 2,
сверены реальные parser/dependency/native+WASM/server tests и отдельный
Criterion benchmark. Контракт phase-2 base slice: A1/formula bar,
SUM/AVERAGE/MIN/MAX/IF, инкрементальный граф, cycle typed error и общий
native/browser WASM result hash. Последний `just check` прошёл полностью,
`just formula-parity` — 99/99 scalar fixtures, 1000 seeded workbooks и 2000
patches. Живой server integration suite — 24/24; E2E из предыдущего среза
после IFERROR не повторялся, поскольку UI flow не менялся. Parser fuzz-like
100 000 bounded arbitrary inputs, canonicalization property, removal of old
dependency edges, only-one-of-10k independent recalculation и iterative
10k chain проходят Rust tests. Reverse range lookup ранее измерен как
269–343 ns для 100 000 disjoint ranges (Criterion, те условия в записи ниже).

Новый `cargo bench --locked -p spreadsheet-formula --bench calculation --
calculate_10000_bound_formula_chain --sample-size 10 --measurement-time 1
--warm-up-time 1` завершился: Criterion interval 41.822–42.905 ms для
создания CalculationProjection из подготовленной книги и полного расчета
10 000 последовательно зависимых формул, optimized build, Windows
10.0.26100, Intel Ultra 9 185H, 22 logical CPUs, 15.6 GiB RAM. Книга
собирается вне измеряемого участка; число evaluated_cells проверяется 10 000.
Это не p95 ввода, commit или браузерного открытия. Локальный функциональный
gate базовой фазы 2 принят; обновлены roadmap и презентация. Excel golden
verification, AND/OR/NOT, volatile-функции, расширенный профиль, Linux/Compose
и remote CI не входят в этот закрытый gate и остаются открытыми. Переход к
фазе 3 не означает их готовность.

---

## 2026-09-29 — IFERROR и исправление точности differential harness

Перед срезом прочитаны AGENTS, roadmap, ADR-009, текущие evaluator/fixtures,
native/browser parity runner и официальный контракт Microsoft IFERROR. Цель —
продолжить фазу 2 одной функцией с серверным и browser WASM вычислением.
IFERROR вычисляет первый аргумент, возвращает его без вычисления fallback,
если spreadsheet error нет; при error вычисляет fallback. Выбранный Blank
становится Text("") по Microsoft Support. Resource errors evaluator
(cancellation/work limit/resolver failure) не перехватываются. Аргументов
строго два; прочая arity даёт профильную #VALUE!, не Excel golden claim.

Изменены evaluator, unit test на budget/cancellation, семь shared fixtures,
stateful parity formula shape, server integration test, docs, presentation
и RUNBOOK. `just formula-parity` сначала прошёл 99 scalar cases, но строгий
stateful compare нашёл one-ULP отличие для деления `-2.75/62.625` после
patch. Native и WASM domain hashes при этом совпадали на каждом шаге:
причина была в native example runner, который повторно парсил уже
сериализованный JSON результата в `serde_json::Value`; второй decimal parse
менял отображаемый f64. Runner теперь проверяет JSON через `RawValue` и
сохраняет его без второго decimal-to-binary преобразования. Exact assertion
не ослаблен. После исправления `just formula-parity` прошёл 99/99 scalar
fixtures и 1000 seeded books/2000 patches. Отчет этого запуска: Chromium
153.0.8010.12, Windows 10.0.26100, Intel Ultra 9 185H, 1.60 MB входа,
132.3 мс суммарного Worker compute и 1244.06 мс harness roundtrip; это
не p95 UI или commit. Живой `pnpm test:integration` прошёл 24/24,
включая durable IFERROR и последующий валидный пересчёт. Prettier применён
к затронутым JS/JSON/Markdown/HTML файлам. Первый `just check` выявил только
rustfmt отклонение в новом unit test; после `cargo fmt --all` повторный
`just check` завершился с exit 0: Rust format/оба Clippy режима/tests/build,
wasm32 check, protobuf drift, Prettier, ESLint, TS, 52 Vitest и Vite
production build. Presentation check прошёл 10 слайдов, навигацию и layout
1440×900/390×844. Новые E2E/recovery сценарии в этом срезе не запускались;
browser WASM проверен отдельным harness.
Excel golden verification, логические AND/OR/NOT, volatile context и phase-2
gate остаются открытыми.

---

## 2026-09-29 — IFS в общем формульном профиле

Перед срезом прочитаны AGENTS, roadmap, ADR-009, formula semantics, evaluator,
общий fixture corpus и server integration tests. Цель — добавить следующий
логический оператор через один native/WASM evaluator, сохранив ограниченный
work budget и отсутствие доверия к клиентскому AST. IFS принимает 1–127 пар
test/value: условия вычисляются слева направо, первый Boolean TRUE выбирает
свой результат, дальнейшие пары не вычисляются; отсутствие TRUE даёт #N/A,
небулевый вычисленный test даёт #VALUE!, ошибка test распространяется.
Пропущенный выбранный результат даёт 0. Для ленивости/omission это проектный
контракт, а не заявленная проверка в Excel. Верхняя граница 127 пар взята из
официальной документации Microsoft; парсер отклоняет 128 пар контролируемым
LimitExceeded до evaluator.

Изменены evaluator и его тесты, девять shared fixtures с provenance,
stateful native/browser WASM generator, живой server integration test,
formula semantics/compatibility/WASM plan, презентация и RUNBOOK. Исходный
красный corpus до реализации вернул #NAME? для первого IFS case. После
исправления тест на 127/128 пар сначала выявил ошибку тестовых данных:
строка из длинных FALSE дошла до parser argument limit на поздней позиции.
Тест использует компактные FALSE-выражения `0=1`; 127 пар вычисляются, 128
отклоняются typed parser error. Проверки: targeted Rust
engine/corpus 12 tests и Clippy formula crate прошли; `just formula-parity`
прошёл 92/92 scalar cases и 1000 книг/2000 patches с одинаковыми
native/browser WASM normalized hashes. Эта серия на Windows 10.0.26100,
Intel Ultra 9 185H, Chromium 153.0.8010.12: 142.3 мс суммарного Worker
compute, 1392.33 мс roundtrip всего harness для 1.60 MB input; это не p95
пользовательского ввода. Живой `pnpm test:integration` прошёл 23/23,
включая durable IFS и пересчёт после изменения входа. Первый `just check`
прошёл Rust Clippy/test/build, wasm32 check и protobuf drift, но остановился
на Prettier в новом integration test. После форматирования повторный полный
`just check` завершился с exit 0: Rust format/оба Clippy режима/tests/build,
wasm32 check, protobuf drift, Prettier, ESLint, TS, 52 Vitest и production
Vite build. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов,
навигацию и layout 1440×900/390×844. Новые E2E/recovery сценарии в этом
срезе не запускались: интеграция проверена живым сервером, native/browser
WASM parity отдельным harness. Excel golden verification, volatile context и
полный gate фазы 2 остаются открытыми.

---

## 2026-09-29 — COUNT и COUNTA в общем Rust-движке

Перед срезом прочитаны AGENTS, roadmap, ADR-013, formula semantics, evaluator,
bound bridge, fixtures и benchmark harness. Цель — продолжить phase-2 function
profile по исходному порядку. Инварианты: один evaluator native/WASM,
структурный `#REF!` не маскируется пустым значением, прямые аргументы и ссылки
имеют разные coercion rules, calculation budget и cancellation остаются
обязательными, production logs не содержат формул.

COUNT считает числовые ссылки, а в прямых аргументах также Boolean и finite
numeric Text; обычные errors пропускаются. COUNTA считает все nonblank значения,
включая Text("") и error values. Оба имеют профильную arity 1–255 и
row-major обход до 100 000 ячеек. Проекция bound AST теперь различает
`InvalidReference` и literal `#REF!`: прямой сломанный target не превращается
в нулевой COUNT. Корпус дополнен шестью случаями и ссылками на официальные
описания Microsoft; все новые expected values имеют `verified_in_excel=false`.
Добавлены native bound tests, range resource-error test для обеих функций,
две формулы в stateful parity generator и Criterion benches. Обновлены
formula semantics, compatibility, WASM plan, презентация и RUNBOOK.

До реализации `cargo test --locked -p spreadsheet-formula --test corpus`
падал на первом новом COUNT fixture с `#NAME?`; после реализации весь crate
suite и Clippy прошли. `just formula-parity` прошел 83/83 scalar fixtures и
1000 seeded books/2000 patches с одинаковым native/browser WASM hash.
Criterion `cargo bench --locked -p spreadsheet-formula --bench scalar -- COUNT
--sample-size 20 --measurement-time 2` на Windows 10.0.26100, Intel Core
Ultra 9 185H, 22 logical processors, 15.6 GiB RAM, release build:
COUNT_range_100k 238.99–259.77 µs, COUNTA_range_100k 238.98–244.43 µs
(Criterion confidence intervals, 20 samples). Диапазон состоит из 100 000
числовых единиц; это изолированная функция, не p95 ввода или полный
document recalculation. Официальный performance threshold на этом Windows
стенде не утверждается. После этого полный `just check` завершился с exit 0:
оба Clippy режима, весь Rust workspace, wasm32 check, protobuf drift,
форматирование, ESLint, TS, 52 Vitest и Vite production build. Живой сервер
прошел `pnpm test:integration` 22/22, включая durable COUNT/COUNTA formula,
`pnpm test:e2e` 14/14 при DPR 1/2 и `pnpm test:recovery` 1/1 после hard
restart. Презентация: 10 слайдов, навигация и layout 1440×900/390×844
прошли `presentation:check`. Эти функциональные проверки не дают p95 продукта.

Ограничение: семантика вложенного выражения со структурно поврежденной ссылкой
и одновременно error value требует отдельного provenance/compatibility
fixture до phase-4 delete/move. Volatile context native/WASM, расширенные
группы функций и phase-2 gate остаются открытыми.

---

## 2026-09-29 — Bounded durable replay до live-подписки

Перед срезом прочитаны AGENTS, roadmap, collaboration contract, ADR-014 и
текущий actor/wire/client код. Цель — не перечитывать всю книгу при нескольких
пропущенных user events и закрыть гонку между replay и live. Инварианты:
авторитетный порядок PostgreSQL, последовательные версии, durable-before-push,
отдельные confirmed/pending states, повторная авторизация перед отправкой и
bounded backpressure. Риск: system event пока не представлен публичным wire
event, а отзыв ACL еще не упорядочен через access_epoch.

Добавлен `ReplayComplete` в wire v1. При Subscribe сервер сначала регистрирует
actor watch, затем фиксирует авторизованный barrier и читает журнал
`(last_seen,barrier]`. До 128 canonical user events / 1 MiB отправляются по
версии. После барьера watch будит socket, а пропущенные live events снова
читаются из PostgreSQL, поэтому coalescing не пропускает версии. При system
event, превышении лимита или непредставимом event отправляется version-only
hint и `resync_required`; браузер получает HTTP projection. Клиент проверяет
барьер и применяет лишь соседние canonical версии. Decision — ADR-015;
ADR-014 сохранен как история. Слайд дорожной карты уточнен: replay ограничен,
фаза 3 не завершена.

Изменены server WS, wire proto/generated Rust+TS и descriptor, browser store/
connection, integration helper и tests, client unit tests, collaboration docs,
ADR-014/015, README, презентация и RUNBOOK. Живые integration tests покрыли три
пропущенные версии, commit во время отправки 50 событий, live после барьера,
system-event fallback, gap >128 и logout. `pnpm test:integration` 21/21; `pnpm
test:e2e` 14/14 (до последней узкой race fix, которая меняла только
отсечение событий после barrier); `pnpm test` 52/52; `pnpm proto:check`,
Rust fmt и targeted Clippy all-targets/all-features прошли. Финальный
`just check` завершился с exit 0: оба Clippy режима, все Rust tests,
native/WASM checks, proto drift, форматирование, lint/typecheck, 52 Vitest
и production build. `pnpm --filter @pxel/web presentation:check` прошел
10 слайдов в 1440×900 и 390×844. Первая попытка `pnpm proto:check` после изменения
комментариев proto выявила drift descriptor; повторная генерация устранила
его, повторный check прошел. Первая попытка recovery test после `just check`
не подняла сервер: production build переписал локальный binary без dev-auth.
После `cargo build --locked -p pxel-server --features dev-auth` повторный
`pnpm test:recovery` прошел 1/1. Причина связана с порядком сборок test
harness, а не с ошибкой восстановления документа.
После self-review journal fetch разбит на порции максимум по восемь events:
прежний запрос мог материализовать 128 payload до проверки 1 MiB outbound
cap. Для окончательной правки повторно прошли targeted Rust Clippy, сборка
server с dev-auth, `pnpm test:recovery` 1/1, `pnpm test:integration` 21/21,
`pnpm test:e2e` 14/14 и проверка презентации на двух viewport. Полный
`just check` выше относится к состоянию до этого ограниченного изменения;
остальные Rust/TS modules в нем не менялись.

Ограничения: это еще не полный gate фазы 3. Нет atomic access_epoch revoke,
presence, byte-bounded outbound queue metrics, cross-process prompt fanout и
100×1000 convergence stress. Системный event требует projection resync.
Никаких p95 latency из функциональных тестов не выводится. Следующий срез —
ACL access_epoch/send gate и затем presence/convergence harness.

---

## 2026-09-29 — Статус презентации

По замечанию пользователя титульный слайд уточнен: текущий результат назван
рабочим прототипом с локально проверенными фазами 0–1, а не завершенным MVP.
Слайд дорожной карты теперь прямо говорит, что презентация обновляется после
каждого пройденного gate. Фазы 2–6 и продуктовые p95 в ней не объявлены
завершенными. Это правка текста без изменения runtime. `pnpm --filter
@pxel/web presentation:check` прошел 10 слайдов, навигацию и отсутствие
горизонтального переполнения при 1440×900 и 390×844; `pnpm exec prettier
--check presentation/index.html RUNBOOK.md` прошел после форматирования.
Первая попытка обеих команд без `scripts/env.ps1` не запустилась: `pnpm`
не был в PATH текущей PowerShell-сессии.

---

## 2026-09-29 — Fast path для следующего canonical event

После phase-3 version hint измеримый риск стал виден в самой схеме: каждая
удаленная правка вызывала HTTP-загрузку всей sparse projection. Цель среза —
применять одиночный соседний по версии canonical event без полной загрузки,
сохраняя fallback при gap и system events. Прочитаны AGENTS, roadmap,
ADR-014, actor/wire/client код. Инварианты: server order, отсутствие partial
apply, confirmed/pending separation, slow-client coalescing, per-send auth.

Actor watch теперь хранит только последнюю `HeadUpdate {version,event}`;
после durable commit публикуется immutable user event, после replay/system
advance — только head без event. Additive wire field `latest_operation` в
`DocumentHeadChanged` использует существующий `OperationCommitted` DTO;
storage envelope не менялся. Сервер проверяет ACL и действующую dev-сессию
перед отправкой. Browser применяет event лишь при `head_version = confirmed+1`,
валидирует IDs/version/hash, сохраняет optimistic queue отдельно и обновляет
server calculation generation. При gap, system event или отсутствии payload
читается каноническая HTTP projection. Подробное решение — ADR-014.

Изменены proto/generated descriptor и TS, collab watch/тесты,
server documents/main, browser store/e2e/unit, integration test, README,
collaboration/ADR-014 и RUNBOOK. `pnpm proto:generate`, Rust all-targets check,
Clippy all-features, `pnpm typecheck`, `pnpm test` 49/49,
`pnpm test:integration` 17/17 и двухвкладочный Playwright DPR1/2 2/2
прошли. Browser test дополнительно зафиксировал **0 full-document GET**
после удаленной одиночной правки. Затем прошли `pnpm test:e2e` 14/14 dev,
`pnpm test:recovery` 1/1, `just check` exit 0 (оба Clippy режима, Rust
workspace tests, native/WASM checks, proto drift, 49 Vitest, lint/typecheck,
production build), `pnpm --filter @pxel/web presentation:check` на двух
viewport и `pnpm test:e2e` 14/14 на built Axum assets/CSP.
После self-review сборщик `OperationCommitted` стал возвращать `None` для
непредставимой в текущем wire операции, чтобы будущая команда вызвала resync,
а не пустой canonical payload. Добавлен тест coalesced gap с непригодным
последним событием: клиент игнорирует его и берет версию 3 из HTTP. После этой
точечной правки прошли `pnpm test` 50/50, Rust Clippy all-targets/all-features;
полный `just check` выше относится к состоянию непосредственно до неё.
Нет измерений p95/байтов сети; единичный тест доказывает только отсутствие
лишнего GET в данном сценарии. Фаза 3 не принята: durable replay/live barrier,
ACL epoch, presence и convergence stress остаются открытыми.

---

## 2026-09-29 — Проверка действующей dev-сессии на WebSocket

Фаза 3, security slice. Перед изменением прочитаны AGENTS, roadmap,
collaboration contract, auth и PostgreSQL session adapter. Ранее logout отзывал
cookie для новых HTTP/WS соединений, но уже открытый WS держал principal до
истечения срока. Теперь `Principal` хранит только SHA-256 token hash, не token;
`Auth::active` сверяет его с live-записью PostgreSQL и identity. Connection
проверяет его перед command, subscription и calculation requests, перед
document head/calculation payload send и на heartbeat. При отзыве соединение
закрывается. Содержимое документа и credential не попадают в tracing.

Изменены `apps/server/src/auth.rs`, `main.rs`, unit test, живой тест
`tests/integration/document.test.mjs`, ADR-014, collaboration docs и RUNBOOK.
Новый integration test использует разные cookies одного dev account:
после DELETE одной сессии ее WS не получает следующий head hint от правки
другой сессии. `pnpm test:integration` 17/17, `pnpm test:e2e` 14/14 dev,
`pnpm test:recovery` 1/1, `just check` exit 0 (оба Clippy режима, Rust tests,
native/WASM checks, proto drift, 48 Vitest, lint/typecheck/build).
`cargo test --locked -p pxel-server --features dev-auth` 4/4 и targeted
Clippy также прошли. После production build дополнительно прошел Playwright
на built Axum assets/CSP: 14/14. SQLx metadata не менялась; remote CI/Linux
не запускались.

Ограничение: DB recheck и socket close не являются атомарным `access_epoch`
send gate для изменения document ACL; гонка между check и фактической
отправкой пока возможна. Это следующий security gate фазы 3 вместе с
canonical replay/live barrier.

---

## 2026-09-29 — Первый phase-3 slice: две вкладки и version hints

Перед началом прочитаны AGENTS, roadmap, collaboration contract, действующий
wire/actor/client код. Цель: открытый второй клиент узнает о новом durable head
без reload. Затронуты инварианты server-authoritative state, отсутствие
содержимого документа в неавторизованном push, bounded backpressure, race-free
subscribe/commit и разделение confirmed/pending. Риски: coalescing hint не
заменяет canonical replay, ACL revoke/send gate и multi-process fanout.

Wire v1 дополнен `SubscribeDocument`/`DocumentHeadChanged`; persisted storage
schema не менялась. Document actor держит Tokio watch для head version и
публикует после commit/replay. Subscriber регистрируется до авторизованной
проверки head; сервер отправляет немедленный hint, если клиент отстал, и
проверяет ACL/head непосредственно перед каждым дальнейшим hint. Slow receiver
видит последнюю версию, не блокируя actor; payload содержит только document ID
и версию. Browser при hint перечитывает каноническую HTTP projection, оставляя
optimistic queue отдельной. Reconnect повторно подписывается с подтвержденной
версии. Interim решение записано в ADR-014; конечный phase-3 replay/live
barrier из `docs/collaboration.md` сохраняется как отдельный gate.

Изменены wire protobuf/generated Rust+TS, collab actor/watch tests, Axum WS,
browser connection/store/tests, integration helper/test, двухвкладочный
Playwright сценарий, README/collaboration/ADR-014, презентация и RUNBOOK.
После первой сборки новый field `head_updates` отсутствовал в трех test
инициализаторах; исправлены сами fixtures. Первый Vitest обнаружил, что
reconnect теперь правильно отправляет subscribe перед pending command;
assertion уточнен для проверки обоих сообщений без ослабления свойства.
Первый Clippy обнаружил вложенный `if` в server; свернут.

Реально выполнены: `pnpm proto:generate`; Rust all-targets/all-features check;
targeted collab/wire tests 7/7 и 5/5; `pnpm test` 47/47;
`pnpm test:integration` 16/16 (два sockets, поздняя подписка и canonical HTTP
read); `pnpm test:recovery` 1/1 hard restart; `pnpm test:e2e` 14/14 на Vite
dev и 14/14 на built assets с production CSP, включая обмен в обе стороны при
DPR 1/2. Повторный `just check` exit 0: формат, оба Clippy режима, весь Rust
workspace, native/WASM checks, protobuf drift, lint/typecheck/Vitest/build.
`pnpm --filter @pxel/web presentation:check` прошел 1440×900 и 390×844;
обновленный roadmap-слайд визуально просмотрен в обоих размерах. Новых
dependencies нет. Linux/Compose/remote CI и latency p95 не запускались.

После этих проверок обнаружен крайний случай временной ошибки HTTP refresh
после version hint: прежняя одна попытка могла оставить вкладку отставшей.
`DocumentStore` получил отдельный bounded exponential retry (1–30 секунд)
для максимальной замеченной head version, с отменой при disconnect; таймер
не пересекается с retry пользовательской команды. Unit regression test
подтверждает восстановление после первой сетевой ошибки без повторной отправки
command. После правки прошли `pnpm test` 48/48, `pnpm typecheck`, `pnpm lint`.
Также прошли `pnpm format:check`, `pnpm build` и повторная проверка презентации
в двух viewport.
После уточнения проверки верхней границы версии hint и сброса retry при смене
документа снова прошли `pnpm test` 48/48, `pnpm typecheck`, `pnpm lint` и
`pnpm format:check`.
Built-assets Playwright 14/14 и full `just check` выше выполнены до этой
последней правки и не заявляются повторно для нее.

Ограничения: version hint не несет canonical operation и не дает replay
barrier/`ReplayComplete`. ACL epoch и ordered subscription invalidation,
presence, byte-bounded canonical outbound queues, convergence stress,
структурные операции, snapshots и XLSX остаются открытыми. Следующий
минимальный slice: durable canonical event replay/live barrier с ограничениями
по сообщениям и байтам; после него access-epoch revoke gate.

---

## 2026-09-29 — Persisted calculation context and presentation

Продолжена фаза 2 по ADR-013 после чтения AGENTS, roadmap, ADR и текущего кода.
Цель среза: воспроизводимо хранить эпоху расчета до включения volatile-функций
и дать пользователю листаемый обзор реального состояния проекта. Затронуты
инварианты durable-before-ACK, CAS, отдельного system principal, монотонной
версии, deterministic domain hash и запрета считать browser preview каноном.
Риски: system event не должен выглядеть пользовательским; старые schema 1/2 и
их hash не должны меняться; повтор explicit recalc не должен создавать новую
эпоху; поздний результат расчета нельзя показывать поверх нового контекста.

Реализованы типизированные `CalculationContext` и `AdvanceCalculationEpoch` в
domain, явный `EventPrincipal::System`, проверка последовательной эпохи и
контекста до commit, контекст в versioned domain hash. Storage envelope schema 3
и additive migration `202609290001_system_calculation_events.sql` отделяют
системное событие от старых user schema 1/2; БД проверяет допустимую комбинацию
principal и actor. PostgreSQL journal фиксирует event через прежний row-lock/CAS
путь с idempotency и synchronous commit. Actor получает одну выборку времени,
seed и context ID на trigger, готовит candidate до commit, при CAS retry
переигрывает тот же sample на новом head. HTTP `POST
/api/documents/{id}/recalculate` принимает `operation_id`, проверяет
Origin/session/ACL и возвращает receipt. Серверный расчет и WebSocket доставка
содержат фактические epoch/context ID; browser отвергает response для другого
контекста. RAND/NOW/TODAY и передача контекста в WASM Worker не включены.

Добавлена standalone презентация `presentation/index.html` из 10 слайдов:
продукт, архитектура, commit path, формулы, реальные замеры, сохраненный
контекст, честное сравнение с Google Sheets и дорожная карта. Листание кнопками,
стрелками, PageUp/PageDown и касанием; локальный файл открывается без сервера.
Иллюстрация редактора явно помечена схемой. `apps/web/scripts/check-presentation.mjs`
проверяет навигацию, ошибки JS и горизонтальный overflow в Chromium при
1440×900 и 390×844. Скриншоты для ручной проверки созданы только в игнорируемой
`.local/presentation`; desktop/mobile cover и metrics визуально проверены.
README, ADR-013 и wasm-plan синхронизированы с частичной реализацией.

Проверки этого среза: migration применена на private PostgreSQL 18.6;
`cargo sqlx prepare --workspace -- --all-targets --all-features` и повторный
`cargo sqlx prepare --check --workspace -- --all-targets --all-features` прошли.
`just check` сначала остановился на одном `matches!` Clippy в codec, затем
на таком же в storage, после исправления — на Prettier integration test;
после исправлений повторный `just check` завершился exit 0: Rust unit/integration
crate tests, два Clippy режима, native/WASM checks, protobuf drift, 44 Vitest,
ESLint, TypeScript и Vite production build. `pnpm test:integration` 15/15,
`pnpm test:recovery` 1/1 hard restart, `pnpm test:e2e` 12/12 на dev Vite и
12/12 на built Axum assets/CSP, `pnpm --filter @pxel/web presentation:check`
2/2 viewport. `just formula-parity` прошел: 77/77 scalar fixtures и 1000/1000
seeded books с 2000 patches, identical normalized native/browser WASM hashes.
Последний workbook run: Worker compute 148.2 ms и полный harness roundtrip
1341.5 ms при 1.60 MB входа; это не latency одной правки и не p95 продукта.
Стенд: Windows 10.0.26100, Intel Core Ultra 9 185H, Chromium 153.0.8010.12,
Node 24.18.0. Результаты fixture Excel вручную не верифицированы.

Затронуты `spreadsheet-domain`, `spreadsheet-formula`, `spreadsheet-application`,
`spreadsheet-storage-codec`, `spreadsheet-storage`, `spreadsheet-collab`,
`apps/server`, `apps/web`, protobuf storage, migration, `.sqlx`, integration
tests, README/docs, presentation и RUNBOOK. Новых crates/dependencies нет.
Известные ограничения: snapshot recovery gate ADR-013, volatile native/WASM
parity, timer/open triggers, ordered ACL revoke/send gate и live multi-user
subscriptions еще не реализованы. Linux/Compose/remote CI здесь не запускались.
Следующий минимальный срез — общий расчетный контекст в native/WASM и volatile
fixtures; затем фаза 3 с упорядоченным replay/live и отзывом прав.

---

## 2026-09-29 — WebSocket calculation delivery and test-session cleanup

Продолжена полная цель стадий 1–6. Перед срезом прочитаны AGENTS, roadmap,
ADR-011, contracts и действующий Rust/TS код. Цель этого среза — доставлять
серверный расчет по отдельному versioned WebSocket response и атомарно
публиковать его в браузере. Затронуты инварианты server-authoritative results,
stale-version rejection, bounded queues/frames, tenant authorization и
разделение durable content, calculated cache и WASM preview. Риски:
межпроцессный порядок ACL revoke/send, большие результаты и тестовые сессии.

Добавлен additive wire v1 `RequestCalculation`/`CalculationCommitted` с
bounded chunks, последовательностью, context/engine/profile метаданными,
typed result/error и итоговым hash. Durable storage codec не менялся.
`apps/server/src/calculation_stream.rs` кодирует максимум 256 KiB на кадр в
bounded blocking pool, ограничивает outbound queue двумя кадрами и сроком
30 секунд, сверяет доступ и версию перед отправкой. `spreadsheet-collab`
выдает авторизованный calculation watcher и head. Browser store убрал HTTP
polling: запрашивает current version, собирает chunks до final, отклоняет
stale/неполные/поврежденные ответы, после чего публикует confirmed result.
WASM preview остается отдельно. HTTP calculation endpoint сохранен для
диагностики. Решение записано в ADR-012, README/collaboration/wasm-plan
синхронизированы. Epoch 0 и context ID=document ID здесь обозначают только
nonvolatile профиль; persisted contexts еще не реализованы.

Playwright global setup теперь выдает одну адресную dev-сессию на suite и
global teardown отзывает ее. Путь private storage state передается worker
процессам через runner environment. HTTP/recovery harness адресно удаляет
выданные им cookies в `afterEach`/`finally`; новый dev-only DELETE endpoint
проверяет Origin и отзывается по token hash, не по account. SQLx metadata
переподготовлена после DELETE query. Все изменения касаются `proto/pxel/wire/v1`,
wire/collab/storage crates, server main/auth/calculation_stream, web
connection/document store/e2e config/setup/teardown, integration/recovery tests,
`.sqlx`, README, docs и этого RUNBOOK. React header logo уже убран;
10 SVG вариантов доступны в `design/logos` из предыдущего среза.

Первый браузерный прогон после setup завершился 12 ошибками ENOENT: Playwright
workers вычисляли разные PID-пути к storage state. Исправлено единое наследуемое
имя файла без ослабления browser assertions. Ранее накопленные тестовые сессии
достигли DB cap 128 и дали login 503. В private PostgreSQL удалены ровно четыре
сессии, выданные предыдущим тестовым harness с известным prefix; пользовательские
сессии и документы не затрагивались. После полного набора проверок в БД
123 active sessions, private Playwright auth files не осталось. Остающиеся
старые сессии истекают по TTL; dev logout не закрывает уже открытый WebSocket.

Реально выполнены: `cargo sqlx prepare --workspace -- --all-targets --all-features`
(exit 0); `just check` (exit 0: Rust workspace tests, обе Clippy конфигурации,
native/WASM checks, protobuf drift, Prettier, ESLint, TypeScript, 44 Vitest,
production Vite build); `node --test tests/integration/server.test.mjs
tests/integration/document.test.mjs` (14/14); `pnpm test:e2e` (12/12 dev Vite,
DPR 1/2); targeted formula browser test после assert отсутствия HTTP polling
(2/2); `PXEL_WEB_URL=http://127.0.0.1:8080 pnpm test:e2e` (12/12 built assets
с production CSP); `node --test tests/recovery/server-restart.test.mjs` (1/1,
hard restart). После последних документационных и e2e изменений также прошли
`pnpm format:check`, `pnpm lint` и `pnpm typecheck`. Linux/Compose/remote CI
здесь не запускались.

Ограничения: это one-shot calculation response, а не replay/live подписка
canonical operations. ACL per-frame check еще не является ordered access_epoch
send gate при изменении прав действующего WebSocket. Persisted
CalculationContext/AdvanceCalculationEpoch, volatile RAND/NOW/TODAY,
структурные оси, snapshots и XLSX остаются следующими срезами. Начат следующий
срез: прочитаны domain/application/storage/codec/protobuf контракты и
зафиксирован ADR-013 для system principal, schema 3, context hash, устойчивых
CAS retries и native/WASM parity. Код persisted context еще не менялся;
acceptance ADR-013 не объявлен пройденным.

---

## 2026-09-29 — stateful WASM preview

Предыдущий goal turn дал проверенный прогресс: durable formulas и browser/recovery
проверки. Продолжаем полную цель, не объявляя фазу 2 завершенной. Прочитаны AGENTS,
roadmap, wasm-plan, текущие adapter/store/worker и ADR-010. ADR-011 фиксирует
stateful preview: checkpoint один раз, затем ограниченные cell patches с expected
input version и pending revision. Preview никогда не становится confirmed result.

Root изменил justfile и scripts/dev.mjs: WASM generation выполняется перед
frontend checks/startup. Vite выпускает отдельные module Worker/WASM assets.
Production CSP разрешает wasm-unsafe-eval и same-origin workers, сохраняя запрет
JavaScript unsafe-eval; HTTP header assertions добавлены. Playwright baseURL
настраивается PXEL_WEB_URL для проверки built assets через Axum. Добавлены
browser draft/cancel test и seeded native/WASM workbook harness (1000 книг,
2000 patches); до фактического запуска они не являются пройденными проверками.

Отдельный coalescing regression test блокирует CPU semaphore, публикует версии
до 50, затем сравнивает последний результат и hash с полным расчетом. Команды
`cargo test -p spreadsheet-collab --lib` (3 passed) и Clippy all-targets прошли.
Первый compile нового теста обнаружил неявный i32; диапазон уточнен как u32.
Пересборка сервера встретила Windows executable lock от dev session 65397;
процесс остановлен через его handle, отсутствие pxel-server подтверждено CIM,
повторный cargo build -p pxel-server --features dev-auth прошел.

Domain agent добавил production hydrate и atomic preview_candidate, сохранив
строгий +1 canonical candidate. 32 domain tests, Clippy, wasm32 check и отдельные
5 hydration tests без test-support прошли. Files: domain workbook.rs,
tests/hydration_preview.rs, README. Native/WASM adapter и frontend preview
находятся в отдельных агентских областях; итог интеграции будет записан после
сборки и реальных browser checks. Наличие API не подтверждает product preview.

Итог интеграции: stateful.rs и stateful_runner используют общий domain/engine;
начальное состояние 32 MiB, patch 1 MiB/1000 cells. Review обнаружил накопление
AST до aggregate budget check; добавлен проверяемый накопительный счетчик сразу
после каждого binding и regression compact-source/expanded-AST test. 9 adapter
tests и Clippy прошли. Первые 3 tests ошибочно сравнивали serde_json integer
с binary64; ожидаемые значения исправлены на float без изменения семантики.

Frontend files calculation/{protocol,client,workbook.worker,client.test},
document/{store,store.test}, App, connection/useSession. Preview stamps/cache
отделены от confirmed. Очередь coalesces по stable address, один job in flight;
256 cells/1 MiB cap. InvalidFormula при atomic apply допускает исправление
без нового checkpoint; calculate failure имеет отдельную phase и не имитирует
rollback уже примененного patch. Worker закрывается при unmount; WS reconnect
не уничтожает preview. 37 Vitest tests, TypeScript и ESLint прошли отдельно.

Реальные новые проверки: 1000 seeded workbooks / 2000 patches native/browser
WASM parity passed (seed 1347962188); 10 dev browser scenarios passed; затем
12 browser scenarios passed через http://127.0.0.1:8080 (built Vite + Axum CSP),
включая missing-WASM controlled failure. Неверный test selector был исправлен
на существующий calculation-preview badge; счетчик загрузок теперь считает
application/wasm, исключая Vite JS ?url module. Условия durable equality и
одной загрузки binary сохранены. 12 HTTP integration tests с CSP assertions
тоже прошли. Полный итоговый just check выполняется отдельно.

WASM stateful benchmark harness: 170.7 ms Worker compute / 1293.15 ms roundtrip,
1 601 377 input bytes, Chromium 153.0.8010.12, Ultra 9 185H, Windows 26100.
Это один harness run для 1000 книг, не optimistic p95. Corpus использует
неизменные оси одного листа, семь arithmetic/aggregate/IF shapes; не доказывает
весь Excel profile или structural parity. just formula-parity теперь вызывает
и scalar, и workbook harness; существующий CI step включает обе проверки.

Открыты CalculationCommitted push, persisted contexts/volatile, расширенный
corpus и benchmark boundary на пользовательском сценарии; фаза 2 не завершена.

Финальный `just check` exit 0: обе Clippy configurations, workspace Rust tests,
native build, wasm32 checks, protobuf drift, WASM generation, Prettier/ESLint,
TypeScript, 37 Vitest tests, Vite production build. `just dev` после изменения
startup реально построил WASM и поднял Axum/Vite; серверный session handle 5906.
После lifecycle fix повторные 12 production browser tests и recovery test
с hard restart прошли. Итоговый
`just formula-parity` прошел scalar77 и workbook1000/patch2000; WASM786462 bytes.
Повторный workbook run при параллельных browser tests: compute704.5ms,
roundtrip4220.62ms; не сравнивается как регрессия с предыдущим одиночным run.
Linux/Compose/remote CI в этом срезе не выполнялись. Gate полной фазы 2 открыт.

---

## 2026-09-29 — durable formulas, calculation projection и UI

Продолжена цель всех стадий по ADR-008. Логотип в header уже убран,
10 вариантов доступны в design/logos; текущий срез посвящен фазе 2.

Domain разделяет CommandPayload и canonical Operation, хранит sparse CellContent
Value/Formula и versioned bound AST. Старые value-only hashes сохранены;
formula semantic hash отделен от protobuf bytes. Wire принимает source-only
formula_command; присланный клиентом canonical AST отклоняется. Persisted formula
envelope имеет schema 2, старые события остаются schema 1. Решение — ADR-010.

CalculationProjection связывает graph/range index с bound evaluator, удаляет
старые зависимости, сохраняет transactional result cache и пересчитывает dirty
подграф. Calculation worker работает через bounded CPU semaphore вне actor,
coalesces новые версии и отбрасывает stale completion. Остановка actor отменяет
worker, включая ожидание CPU permit. HTTP calculation encoding также ограничен
semaphore и вынесен из async runtime thread. Content hash не включает results.

Frontend отправляет source-only intent; formula bar сохраняет source, Canvas
и accessibility mirror используют подтвержденный result соответствующей версии.
Polling ограничен, имеет cancellation/generation guards; ошибки расчета видны
в интерфейсе. Pending source не превращается сам в confirmed result.

Основные файлы: domain/workbook/formula, formula/calculation, collab/lib/calculation,
storage journal и storage-codec/formula_codec, wire-protocol/formula/build,
proto storage/wire, apps/server/documents/main, web/document/store и tests,
web/grid/renderer/CanvasGrid, App, browser/integration/recovery tests, manifests
и lockfile, README, ADR-010. SQLx metadata подготовлена на реальной БД.

Реально выполнены: pnpm test:integration — 12 passed; pnpm test:e2e — 8 passed
в Chromium при DPR 1/2; pnpm test:recovery — 1 passed с двумя hard restarts,
включая сохраненный formula source и равенство result hash после восстановления.
Проверены incremental affected-only update, цикл, запрет чужого доступа,
source idempotency/collision, untrusted AST rejection и invalid/base-zero formula.

Первый just check остановился на rustfmt. После форматирования Clippy обнаружил
large enum variants у Submission и generated ClientEnvelope. Добавлены Box через
Prost config и Rust type; allocation event выполнена до COMMIT. Assertions и lint
policy не ослаблялись. Повторный `just check` завершился exit 0: rustfmt,
обе конфигурации Clippy, workspace Rust tests, native build, wasm32 checks,
protobuf drift, Prettier, ESLint, strict TypeScript, 31 Vitest tests и Vite build.

Повторный `just formula-parity` также exit 0: 77/77 native/browser WASM cases
совпали по expected values и normalized hashes; missing-WASM controlled failure
прошел. Chromium 153.0.8010.12, Windows 10.0.26100, Ultra 9 185H, Node 24.18.0;
binary 291662 bytes, init 23.2 ms, evaluation 20.7 ms, roundtrip 112.09 ms.
Это единичный замер общего scalar corpus, не p95 и не workbook preview gate.

Ограничения: HTTP polling — промежуточный transport; CalculationCommitted push,
persisted CalculationContext/AdvanceCalculationEpoch, stateful WASM preview и
1000 generated workbook parity еще нужны. Epoch 0 этого среза не реализует
volatile semantics. Binding stale base доказан только для неизменных осей;
перед structural operations необходим checkpoint+tail. Фаза 2 не завершена,
фазы 3–6 и Linux/Compose/remote CI не объявлены пройденными.

---

## 2026-09-29 — сверка повторного архитектурного review

Статус архитектурной основы: **ACCEPTED FOR PHASE 0**. Это не общее
принятие формульной модели и collaboration semantics. Последующее разрешение
реализовывать срезы не заменяет проверки их семантики и acceptance criteria.
Текущее поручение обработано как сверка обязательных поправок; новые продуктовые
функции и инфраструктура в рамках этой сверки не добавлялись.

Все десять поправок уже записаны в ADR-006. Ниже карта актуальных контрактов,
чтобы первоначальное предложение в конце RUNBOOK не воспринималось как действующее.

| Поправка                                            | Авторитетный контракт                                    | Граница реализации                                                                                                                                    |
| --------------------------------------------------- | -------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| CAS head и полный повтор подготовки/транзакции      | docs/persistence.md, CAS transaction                     | CAS присутствует в PostgreSQL journal; локальный actor не заменяет DB constraint                                                                      |
| Candidate до COMMIT, prepared swap после            | docs/collaboration.md, Commit algorithm                  | Candidate preparation присутствует; recovery/quarantine проверяются отдельно                                                                          |
| OperationCommitted отдельно от CalculationCommitted | docs/collaboration.md, Operation и calculation сообщения | Полный calculation transport остается незавершенным; HTTP расчет не доказывает выполнение этого контракта                                             |
| Persisted context и AdvanceCalculationEpoch         | docs/formula-semantics.md                                | Требование будущего расчета volatile-функций; epoch 0 не является реализацией persisted context                                                       |
| Binding на base_version                             | docs/collaboration.md, Окно преобразований               | 10 000 версий / 16 MiB и axis checkpoint — контракт; неизменные оси раннего среза не заменяют историческое окно после появления structural operations |
| Copy/move/cut/insert/delete/sort отдельно           | docs/formula-semantics.md                                | SortRange отложен; будущая перестановка ограничена прямоугольником                                                                                    |
| Access epochs и прекращение отправки                | docs/collaboration.md, access epochs                     | Проверка command ACL не подтверждает отзыв live-подписки; send gate остается отдельным gate фазы 3                                                    |
| Wire отдельно от persisted codec                    | docs/architecture.md, docs/persistence.md                | Независимые crates/schema; protobuf bytes не являются domain hash                                                                                     |
| Replay/live barrier                                 | docs/collaboration.md, Subscribe                         | Регистрация до чтения replay обязательна; live barrier еще не объявляется реализованным                                                               |
| LocalFilesystemBlobStore                            | docs/persistence.md, BlobStore                           | Локальный adapter присутствует; S3/Garage contract suite относится к фазе 5                                                                           |

Сохранены уточнения: RAND должен различать несколько вызовов внутри одной формулы
через стабильную identity AST node; relative delta нормализуется после structural
transform; отправленные до отзыва прав байты невозможно отозвать. ReplayComplete
завершает replay до выдачи buffered live events. Эти детали дополняют требования,
не ослабляя их.

Изменен только RUNBOOK.md. Прочитаны AGENTS, roadmap, ADR-006, актуальные
architecture/collaboration/persistence и найденные места кода actor/journal.
Использованы Get-Content, rg и git status; выполнена проверка существования
документов, указанных в таблице. Unit/integration/browser tests и сборка для
этой документационной сверки не запускались. Уже существующие изменения кода
не приписываются этому review и не объявляются проверенными этой записью.

Следующий минимальный технический срез остается проверкой текущей интеграции
durable formulas и расчетов; завершенность фаз 2–6 этим review не подтверждается.

---

## 2026-09-28 — фаза 2, формульный профиль и граф зависимостей

Продолжение полной цели по ADR-008; предыдущий срез дал конкретный прогресс:
исправления документации и прошедшие проверки, а не ожидание внешнего события.
Прочитаны AGENTS, roadmap, formula-semantics, текущие domain/manifests и ADR-008.
Применены test-driven-development и documentation-and-adrs. Цель текущего среза:
чистый bounded parser/evaluator, стабильная bound AST model, затем native/WASM
corpus и интеграция с actor. Фаза 2 пока не завершена.

Затрагиваемые инварианты: server canonical calculation, stable cell identity,
versioned AST без parser spans, отдельный calculated cache, bounded resources,
детерминированный порядок и запрет содержимого формул в логах. Риски: coercion,
ложное заявление Excel parity, глубина AST, большой диапазон и stale calculation.

Добавлен workspace member spreadsheet-formula. В нем direct forward/reverse graph
удаляет старые зависимости, распространяет dirty только по затронутому подграфу
и строит deterministic Kahn order без рекурсии. Blocked включает циклы и их
заблокированных downstream consumers; это не утверждение точного SCC membership.
Reverse range index — сбалансированный row-interval tree с column filtering;
не создает отдельного ребра для каждой ячейки диапазона. Перестроение геометрии
индекса отдельно от запроса на обычную правку значения. Пока graph/index еще
не подключены к серверному расчету, интерфейс не выдается за рабочую UI-функцию.

Файлы root: Cargo.toml/Cargo.lock, spreadsheet-formula/src/{dependency,range_index}.rs,
tests/{dependency,range_index}.rs, benches/dependencies.rs. Отдельные агенты ведут
parser/evaluator, bound model/corpus и WASM adapter; их результаты записываются
после выполнения, не заранее.

Проверки: первое выполнение dependency test выявило отсутствующий module export;
после export компилятор выявил неоднозначный integer count, исправлен явным usize.
`cargo test -p spreadsheet-formula --test dependency --test range_index`:
5 tests passed, включая iterative chain 10000 и property comparison interval
lookup с полным scan. `cargo clippy -p spreadsheet-formula --lib -- -D warnings`
прошел на этом состоянии.

Реальный Criterion `cargo bench -p spreadsheet-formula --bench dependencies --
--sample-size 10 --measurement-time 1 --warm-up-time 1`, release на указанном
Windows стенде: dirty+schedule цепочки 10000 — 9.62–12.89 ms; lookup в 100000
непересекающихся ranges — 269–343 ns; rebuild индекса — 16.40–19.58 ms.
Это scheduler/index benchmarks, не время вычисления 10000 формул и не p95.
Во время работы возможна фоновая нагрузка; performance target пока не принят.

ADR-009 и fixtures/formulas/profile-v1.json содержат 77 случаев с provenance,
все verified_in_excel=false. Найдена неверная прежняя Microsoft ссылка на
precedence (PowerPoint tav); заменена официальной таблицей Excel. Противоречие
двух Microsoft страниц о direct AVERAGE coercion явно отмечено как непроверенное.

Добавлены domain::formula (immutable validated bound arena, 6 tests), parser,
canonical printer, evaluator SUM/AVERAGE/MIN/MAX/IF/arithmetic/comparisons,
binder (2 tests) и bound evaluator bridge (5 tests). Binder получает конкретную
Workbook axis projection и сохраняет ее версию; сам не притворяется историческим
rebase service. Bridge не перепарсивает source. Bound AST не содержит spans.
Проверки malformed/deep input, budget/cancellation, scalar/reference coercion
и property canonicalization входят в engine tests. Код еще не включен в actor.

WASM adapter spreadsheet-wasm использует точно закрепленные wasm-bindgen
0.2.129 и serde_json 1.0.151 (MIT/Apache-2.0, проверены upstream metadata).
CLI скачан в .tools, SHA256 проверен, version command выполнена. Thin adapter
и native fixture runner вызывают одно ядро; boundary DTO ограничен 1 MiB.
scripts/wasm.mjs генерирует bindings, tests/parity/formula.mjs сравнивает native
и настоящий browser module Worker. Добавлены just formula-parity, CI step и
исключение generated WASM из Git; изменены justfile/.github/workflows/ci.yml.

Первый реальный parity run: 77/77 expected values и normalized hashes совпали,
WASM загружен по сети один раз; второй worker доказал controlled failure при
недоступном binary. 3 adapter tests, Clippy и wasm32 compilation прошли.
Первый binary 281517 bytes; init 23.4 ms, evaluation 77 cases 18 ms, общий
roundtrip 105.08 ms — единичный замер, не p95 и не продуктовый preview benchmark.
Отчет .local/formula-parity.json, контракт/ограничения docs/wasm-plan.md.
Stateful worker приложения, server CalculationCommitted и durable formula codec
остаются следующими интеграционными шагами. CSP тестового harness не доказывает
готовность production server CSP; она будет проверена при подключении worker.

После review исправлены две воспроизведенные ошибки: resource failure диапазона
мог скрываться за ранее встретившимся spreadsheet error; canonical parentheses
для 100 unary operators превышали parser call-depth, хотя AST оставался допустим.
Результат resolver теперь проверяется прежде spreadsheet error, а parser call-depth
256 отделен от AST depth 128. Regression tests сохранены, assertions не ослаблены.
Первый общий just check поймал RED canonical-depth до исправления; последующий
запуск проверяет исправленный код, не выдается за первоначально зеленый результат.

Финальный `just formula-parity` после этих исправлений прошел 77/77 cases;
binary 291662 bytes, init 8.6 ms, evaluation 10.1 ms, roundtrip 57.75 ms.
Параметры и различия нагрузки не позволяют трактовать это как оптимизацию.
Scalar Criterion: SUM range100k 700–727 us, AVERAGE 1.10–1.35 ms,
MIN 1.05–1.16 ms, MAX 1.17–1.89 ms; parse IF 4.00–5.11 us.
Команда агента: `cargo bench -p spreadsheet-formula --bench scalar --
--sample-size 10 --measurement-time 1 --warm-up-time 1`, release, тот же стенд;
фоновые сборки присутствовали. Это отдельные aggregate kernels, не workbook recalc.

Следующий минимальный срез: FORMULA-DURABLE — CellContent/SetCellFormula,
независимый persisted AST codec и hash extension с сохранением старого replay;
затем CALC-PROJECTION — связать dirty graph/range index/bound evaluator и
проверить независимые формулы/10k chain; CALC-TRANSPORT — input-versioned
CalculationCommitted и stateful Worker patches. Эти зависимости нужны до
включения редактирования формул в UI. History binding window, persisted contexts,
1000 seeded workbook parity, parser snapshots/libFuzzer и все фазы 3–6 остаются
открытыми; общая цель не сокращается до прошедшего scalar corpus.

Финальная общая проверка: **just check завершился exit 0** после исправлений:
rustfmt, обе конфигурации Clippy, 80 Rust tests, native build, wasm32 checks,
protobuf drift, Prettier, ESLint, TypeScript, 24 Vitest tests и Vite build.
Существующие HTTP/recovery/Playwright сценарии в этом формульном срезе повторно
не запускались (серверный protocol и UI не менялись); их предыдущие результаты
описаны ниже. Новый browser WASM parity harness выполнен отдельно успешно.
README, architecture/formula-semantics и ADR-009 синхронизированы с кодом.

`cargo deny check` прошел: advisories/bans/licenses/sources ok; транзитивные
duplicate versions дают warnings, policy не ослаблялась. `actionlint -shellcheck=`
прошел; это static workflow check, shellcheck и remote CI не запускались.

---

## 2026-09-28 — повторная сверка required changes и фактического состояния

Архитектурное решение остается **ACCEPTED FOR PHASE 0**, не общим ACCEPTED.
Десять поправок находятся в ADR-006 и разделе «Десять обязательных поправок»
ниже. Историческое предложение с полной сортировкой строк не является текущим
контрактом. Формулы и collaboration требуют отдельных семантических gates.

Уточнения к принятому тексту: ReplayComplete(V) отправляется после replay и
**до** buffered live events; original_delta нормализуется после структурного
изменения относительно актуального origin; RAND включает stable AST node ID,
чтобы два вызова в одной формуле не получали одно значение. Snapshot activation
проверяет не только checksum объекта, но и восстановление domain state.

### Реализация, уже присутствующая в рабочей папке

- Domain: sparse workbook, стабильные оси, typed values, подготовка candidate,
  собственный canonical hash; лимиты 100000 cells и 16 MiB суммарного текста.
- Application/storage/codec: journal port, отдельный persisted protobuf envelope,
  CAS + append-only журнал, повторная ACL/epoch проверка в транзакции,
  idempotency fingerprint, quarantine при поврежденном replay. Миграции 002/003
  добавляют журнал и persistent development identities. SQL timeouts ограничены.
- Collab/server: bounded actor, create/read, SetCellValue/ClearCell, receipt,
  replay после restart. Dev auth изолирован feature flag; существование receipt
  не обходит авторизацию. Подписки и отзыв их outbound queues еще не реализованы.
- Web: confirmed/pending разделены, sparse Canvas, overlay editor и навигация,
  создание/редактирование/reload. Исправлены stale HTTP response и потеря pending
  при неудачном refresh после receipt. Формулы и WASM preview отсутствуют.
- Удален верхний левый логотип; отдельные десять SVG-концептов и галерея находятся
  в design/logos. Это не изменение выбранной архитектуры.

Измененные группы файлов: crates/{domain,application,storage,storage-codec,collab}
(с префиксом spreadsheet-), apps/server, apps/web, proto, migrations, .sqlx,
Cargo manifests/lock, package.json, scripts/{proto,smoke}, tests/{helpers,integration,
recovery}, design/logos. Код последующих фаз не заявляется готовым.
README и вводный статус docs/collaboration.md синхронизированы с текущим кодом;
описание будущих механизмов не считается доказательством их реализации.

### Реальные проверки текущего среза

- `cargo test --workspace --all-features`: 48 Rust tests passed, включая реальный
  PostgreSQL, CAS/idempotency, rollback, lock timeout и filesystem contracts.
- `cargo sqlx prepare --workspace -- --all-targets --all-features`: успешно;
  offline query metadata обновлены по реальному PostgreSQL.
- `just smoke`: 10 HTTP/WebSocket tests, 1 recovery test и 6 Chromium Playwright
  tests passed. Браузер проверяет ввод/reload при DPR 1 и 2.
- `just check`: rustfmt, обе конфигурации Clippy, Rust tests/build, WASM target
  check и protobuf drift check прошли; первоначальная остановка на Prettier
  исправлена форматированием четырех файлов. Оставшиеся команды выполнены
  отдельно: format:check, ESLint, strict TypeScript, 24 Vitest tests и Vite build
  прошли. Полный агрегат после этого повторно не запускался.
- Recovery test завершает процесс после получения, но намеренного игнорирования
  ACK клиентом; после restart проверяет replay и receipt при повторе команды.
  Это не fault injection точно между COMMIT и отправкой ACK.

Criterion domain (release, 10 samples, warmup/measurement по 1 s; машина описана
ниже): candidate для 100k cells первоначально 17.36–19.96 ms, повторно
6.94–7.37 ms; hash 100k 9.43–10.20 ms; отклонение превышения 16 MiB до clone
97–100 ns. Разница candidate отражает неконтролируемую фоновую нагрузку,
а не доказанную оптимизацию. Commit p95 и полная recovery matrix не измерены.

Linux Compose runtime и удаленный CI не запускались: Docker Engine/WSL и remote
repository отсутствуют. Фазы 2–6, native/WASM formula parity, replay/live barrier,
access revocation, snapshots и XLSX не проверены и не объявляются завершенными.

---

## 2026-09-28 — разрешено продолжение всех стадий

Пользователь поручил продолжать все остальные стадии, убрать верхний левый логотип
и параллельно подготовить субагентом 10 новых вариантов на основе текущего знака.
Цель остается полной: фазы 1–6, а не только следующий срез. См. ADR-008; старые
phase-0-only формулировки ниже являются историей. AGENTS/roadmap обновлены.
WSL status повторно подтвердил отсутствие WSL, Docker Engine не найден. Открытый
Linux gate сохраняется, production readiness не заявляется.

Запущен отдельный субагент logo_concepts с изолированными файлами header/CSS и
design/logos. Основная работа: фаза 1, create/edit/durable commit/replay. Перед
реализацией прочитаны AGENTS, roadmap, architecture, ADR-006, текущие domain,
application/storage, migrations и wire schema. Результаты срезов записываются ниже.

---

## 2026-09-28 — phase 0 skeleton и локальная проверка

**Статус: phase 0 реализована и проверена локально; полный gate пока не закрыт.**
Архитектурный статус остается **ACCEPTED FOR PHASE 0**. Docker Engine/WSL на этом
Windows host отсутствуют, remote GitHub repository не настроен; поэтому Compose
runtime и Linux CI не объявляются пройденными. Фаза 1 не начиналась.

### Что изменено

- Root: `AGENTS.md`, `README.md`, Cargo/pnpm workspace manifests и lockfiles,
  `rust-toolchain.toml`, `justfile`, EditorConfig/rustfmt/Clippy/ESLint/Prettier,
  ignore/line-ending rules, `deny.toml`, `docker-compose.yml`, GitHub workflow.
  Локальный Git создан на ветке `main`; commit/push не выполнялись.
- `apps/server`: Axum binary, migrations CLI, health/readiness, строгая проверка
  Origin, cookie dev-auth с compile feature + runtime flag + loopback constraint,
  protobuf WS handshake, heartbeat/timeouts, connection/rate/body/frame caps,
  static frontend serving, CSP/security headers, content-free JSON tracing,
  Ctrl+C и Linux SIGTERM handling.
- `apps/web`: React/TS/Vite shell, собственный Canvas renderer с viewport/overscan,
  DPR, rAF и resize/scroll cleanup, keyboard scrolling, accessibility region,
  настоящий Hello/Welcome, bounded reconnect. Действия редактирования явно disabled;
  нет фиктивного сохранения, per-cell DOM или cell values в React state.
- `spreadsheet-domain`: используемые BlobKey/Checksum/metadata types;
  `spreadsheet-application`: используемый streaming BlobStore port;
  `spreadsheet-storage`: реальный filesystem adapter и SQLx PostgreSQL adapter;
  `spreadsheet-wire-protocol`: v1 Hello/Welcome/Error, limits и checked decoder.
  Formula/collab/storage-codec/WASM/XLSX crates заранее не создавались.
- `migrations/202609280001_foundation.sql`: users, workspaces, memberships,
  documents, document_permissions; access epochs, tenant FKs и role constraints.
  Document operations/CAS persistence остаются фазой 1, сейчас это принятый контракт.
- `proto/`, Buf configs, checked-in descriptor и TypeScript bindings;
  Rust генерируется Prost из того же descriptor без обязательного protoc при Cargo build.
  `.sqlx/` получен реальным `cargo sqlx prepare` и позволяет offline compilation.
- `scripts/`: отдельный native/Compose PostgreSQL bootstrap, общий запуск приложения,
  protobuf generation/drift, live smoke runner, опциональный Windows env helper.
- `tests/integration/server.test.mjs`, Rust unit/DB/filesystem contracts,
  Vitest и Playwright tests. Документы `docs/`, ADR-001…007 и dependency review
  синхронизированы. Десять архитектурных поправок подробно перечислены ниже.

### Решения и границы

MinIO/Garage/S3 не включены. LocalFilesystemBlobStore пишет ограниченный поток во
временный versioned envelope, проверяет checksum/length, синхронизирует файл,
публикует immutable object через hard link; retries одинакового content идемпотентны.
Private owner-controlled directory — обязательная trust boundary. Windows std
не дает гарантии directory fsync; crash-orphan cleanup и platform limitations
описаны в `crates/spreadsheet-storage/README.md`. Это не snapshot implementation.

Dev sessions переиспользуют действующий cookie; duplicate cookies отвергаются.
Handshake cap 1 KiB, WS hello deadline 5 s, send timeout 2 s, heartbeat 15 s,
pong deadline 45 s, 128 connections, глобально 64 admission requests/s.
Document queues/ACL revocation и command cap 1 MiB пока являются контрактами будущего actor.
Production identity adapter отсутствует; default build не может включить dev-auth.

Wire и persisted event formats разделены архитектурно; текущий protobuf содержит
только handshake. Blob envelope имеет свою schema version. Candidate-state/CAS,
calculation contexts и replay barrier не выданы за реализованный document flow.

### Toolchain и подготовка машины

Стенд: Windows 11 Pro 10.0.26100; Intel Core Ultra 9 185H, 16 cores / 22 logical,
15.6 GiB RAM. Native x86_64 Rust GNU и PostgreSQL на одной машине/loopback; Docker
не использовался. Эти сведения не являются результатом performance benchmark.

Совместно локально проверены Rust 1.98.1 + wasm32-unknown-unknown; Node 24.18.0;
pnpm 12.6.0; PostgreSQL 18.6; SQLx/CLI 0.9.0; Prost 0.14.4; Buf 1.73.0;
protobuf-es 2.15.0; React 19.3.0; TS 7.0.2; Vite 8.3.1; Vitest 5.0.2;
Playwright 1.63.0 с Chromium 153.0.8010.12 revision 1243. Полный Linux tested
baseline еще не заявляется. Node 24.21 из proposal не испытывался.

ESLint 10 не подошел по peer range стабильного Babel parser; typescript-eslint
исключает TS7. Использованы ESLint 9.39.5 + Babel 7.29.7 / parser 7.29.9,
без peer overrides и с отдельным строгим `tsc`. Deprecated ESLint 9 — временное
LINT-COMPAT exception; критерии удаления в `apps/web/TOOLING.md` и ADR-007.

В игнорируемую `.tools/` установлены official Rust/rustup, GNU GCC 16.2.0,
PostgreSQL 18.6 binaries, pnpm, just 1.58.0, cargo-nextest 0.9.146,
sqlx-cli 0.9.0, cargo-deny 0.20.2, cargo-audit 0.22.2, actionlint 1.7.12,
Compose standalone CLI 5.5.1. Release asset SHA-256 проверены перед использованием.
SQLx CLI и cargo-audit собраны `cargo install --locked`; CLI SQLx upstream lock
предупреждал о yanked tool-only dependencies, это не workspace Cargo.lock.
Первоначальный LLVM-mingw остался в `.tools/`, но не используется в PATH.

Создан отдельный `.local/postgres` cluster на `127.0.0.1:55432`, SCRAM и случайный
пароль в ignored files. Существующая PostgreSQL service 18.3 на 5432 не изменялась.
Секреты не включены в RUNBOOK или Git. Никакие VDS/Ariel services не затрагивались.
Playwright установил отдельный browser; личный Chrome не менялся. DevTools MCP
не имел debug port, поэтому проверка выполнена изолированным Playwright.

### Реально выполненные команды и результаты

| Команда / проверка                                                                               | Фактический результат                                                                                                                                       |
| ------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm install`, затем `pnpm install --frozen-lockfile`                                           | Pass, lockfile воспроизводим                                                                                                                                |
| `node scripts/infra.mjs --native --database-only`, `just infra-native`                           | Private PostgreSQL поднята; миграции применены; повторный запуск успешен                                                                                    |
| `just dev`                                                                                       | Backend 8080 и Vite 5173 работают от одной команды                                                                                                          |
| `just check`                                                                                     | Pass: rustfmt, Clippy default/all-features с `-D warnings`, Rust tests/build, WASM target check, Buf drift, Prettier, ESLint, TS strict, Vitest, Vite build |
| `cargo test --workspace --all-features`                                                          | 21 passed после auth regression fix; до него 20 passed                                                                                                      |
| `cargo nextest run --workspace --all-features --locked`                                          | 21 passed, включая реальные DB tests                                                                                                                        |
| `cargo sqlx prepare --workspace -- --all-targets --all-features`                                 | Metadata записана в `.sqlx/`                                                                                                                                |
| `cargo sqlx prepare --check --workspace -- --all-targets --all-features`                         | Pass                                                                                                                                                        |
| `SQLX_OFFLINE=true cargo build --workspace --locked`, без DATABASE_URL                           | Pass, default build                                                                                                                                         |
| `cargo check -p spreadsheet-domain -p spreadsheet-wire-protocol --target wasm32-unknown-unknown` | Pass; это не engine parity                                                                                                                                  |
| `pnpm proto:generate`, `pnpm proto:check`, direct `node scripts/proto.mjs --check`               | Pass, общий descriptor/Rust/TS golden fixture                                                                                                               |
| `pnpm exec buf breaking proto --against proto/wire-descriptor.bin`                               | Pass против initial baseline; PR CI сравнивает base commit                                                                                                  |
| `pnpm test`                                                                                      | 12 Vitest passed                                                                                                                                            |
| `pnpm build`                                                                                     | Pass; production JS 297.33 kB / gzip 90.96 kB, CSS 6.66 kB / gzip 2.08 kB; это build output, не latency benchmark                                           |
| `just smoke`                                                                                     | Pass: общий старт/остановка, 7 real HTTP/WS integration tests + 4 Chromium browser cases                                                                    |
| Real `/readyz` при остановке private PostgreSQL                                                  | 503, `/healthz` 200; после restart readiness 200                                                                                                            |
| `SHOW synchronous_commit; SHOW fsync; SHOW full_page_writes`                                     | Все `on`, PostgreSQL сообщает 18.6                                                                                                                          |
| Default binary + `PXEL_DEV_AUTH=1`                                                               | Контролируемый отказ: feature не compiled in                                                                                                                |
| Dev binary + `PXEL_BIND=0.0.0.0:18081`                                                           | Контролируемый отказ: dev auth требует loopback                                                                                                             |
| `cargo deny check` / повтор с `--log-level error`                                                | Pass advisories/bans/licenses/sources; transitive duplicates остаются warnings                                                                              |
| `cargo audit`                                                                                    | Pass, 246 dependencies, 1273 advisories загружены, vulnerabilities не выданы                                                                                |
| `pnpm audit`                                                                                     | No known vulnerabilities found                                                                                                                              |
| `actionlint -shellcheck='' .github/workflows/ci.yml`                                             | Pass; shellcheck отдельно не запускался                                                                                                                     |
| `docker-compose.exe config --quiet` (5.5.1)                                                      | Pass syntax/config; Docker Engine не запускался                                                                                                             |
| `git init -b main`, `git check-ignore` для credentials/tools/screenshots                         | Pass; private files игнорируются                                                                                                                            |

Rust tests включают 11 filesystem contracts, key validation, 3 storage tests
(из них 2 с настоящими изолированными PostgreSQL databases), 3 wire tests и
3 auth tests. Regression повторяет выдачу с одной cookie 256 раз, сохраняя одного
principal и одну session запись. Unix-only symlink contract на Windows не компилировался
и не считается выполненным.

Browser проверяет actual Welcome, pixels Canvas, DPR 1/2, resize, keyboard
scroll, reload, mobile 390 px и отсутствие page/console errors. Визуально
проверены 1440×900 и 390×844. DOM: 115 nodes в Vite и 114 в production static
bundle, без cell DOM. Production bundle проверен через Axum 8080 с его CSP,
реальный handshake успешен. Локальные screenshots: `.local/reports/phase0-desktop.png`
и `phase0-mobile.png`; test reports игнорируются Git.

### Найденные и исправленные ошибки

1. Первый Cargo запуск был раньше готовности server manifest; повтор после его
   создания продолжил сборку. LLVM-mingw не имел GCC runtime `gcc_eh/gcc`; заменен
   используемый linker на GNU GCC MSVCRT. Assertions не менялись.
2. Первоначальный npm shim pnpm на Windows не запускал CLI; isolated `.tools/bin`
   использует официальный `pnpm.mjs`. Shell-based Node spawning давал DEP0190;
   scripts теперь вызывают Node/executables прямо. Vite package exports не позволял
   resolve `vite/bin/vite.js`; путь берется от exported package.json.
3. Clippy нашел needless return, исправлено; default-feature warnings устранены
   cfg на dev-only imports/constants. Оба feature profiles прошли Clippy.
4. Review нашел расходование 128 dev sessions при reconnect: действующий token
   теперь переиспользуется, добавлен regression. Также bootstrap мог наследовать
   чужой DATABASE_URL; migration child теперь получает URL только созданной private DB.
5. Vite первоначально связался с недоступным IPv6 localhost: реальные браузерные
   тесты падали до assertions. Bind/proxy заменены на 127.0.0.1; все 4 cases прошли.
6. Первый HTTP upload-limit test получил ECONNRESET при раннем 413 и отправке
   непрочитанного body. Test теперь использует HTTP Expect: 100-continue для
   наблюдения раннего отказа без upload/shutdown race; строгий assert 413 сохранен.
7. `cargo-deny` обнаружил неявные wildcard path dependencies и отсутствующий
   CDLA license allowlist entry. Добавлены точные workspace versions; LICENSE
   webpki-roots прочитан и CDLA-Permissive-2.0 явно разрешена с documented obligations.
   Security advisory exclusions не добавлялись.
8. Direct `node scripts/proto.mjs --check` вне pnpm не видел plugin PATH;
   script теперь добавляет локальный pinned `.bin`, оба способа вызова прошли.
9. Chromium CDN имел timeout на загрузке; официальный installer повторил загрузку
   и установил требуемую revision. Browser tests не заменялись mock.

### Непроверенное и следующий минимальный этап

- **PH0-LINUX-GATE:** причина — нет Docker Engine/WSL и remote CI. Критерий:
  `just infra-up`, полный `.github/workflows/ci.yml` и Unix blob tests проходят
  на Linux с реальной PostgreSQL. Compose schema уже проверена; это не заменяет
  runtime. Пока gate не закрыт, фаза 1 не начинается.
- Remote CI не запускался; GitHub workflow только создан и проверен actionlint.
  Поэтому candidate toolchain не объявлен общим tested baseline.
- Unit/property/fuzz/convergence/recovery для будущих formula/document/snapshot/XLSX
  modules не запускались: этих модулей сейчас нет. Полный native/WASM differential
  corpus, 100k populated cells и latency p95 не измерялись. Результаты не вымышлены.
- Windows LocalFilesystemBlobStore не обещает POSIX directory-fsync durability;
  production storage contracts проверяются на Linux в своих этапах.
- Deprecated lint toolchain exception имеет явный критерий удаления, ADR-007.

Следующий срез — закрыть PH0-LINUX-GATE. Только после этого планировать фазу 1:
create document → SetCellValue → candidate + CAS commit → replay. Формульная и
collaboration семантика потребуют отдельного подтверждения перед своими фазами.

---

## 2026-09-28 — обязательная корректировка архитектуры перед skeleton

**Текущий статус: ACCEPTED FOR PHASE 0.** Пользователь разрешил переход к skeleton после внесения перечисленных поправок. Это ограниченное принятие foundation, не общее принятие семантики формул и collaboration. Перед фазами 2 и 3 их контракты проходят отдельное подтверждение. До проверки acceptance criteria фазы 0 фаза 1 не начинается.

Этот раздел и документы `docs/` описывают текущие решения. Предложение ниже сохранено только как историческая запись; его несовпадающие положения отменены. В частности, больше не действуют ранняя сортировка полных строк под именем SortRange, общий transport/storage codec, применение операции впервые после COMMIT, S3 в phase 0 и MSRV, приравненный к текущему toolchain.

### Принятые исправления

1. **CAS и полный retry.** Каждый commit проверяет ожидаемый `documents.head_version` посредством условного UPDATE; операция, обновление head, idempotency и необходимые control records находятся в одной транзакции. Ограничения: `(document_id, committed_version)` и `(document_id, operation_id)` уникальны. UUIDv7 operation IDs генерируются глобально; серверная дедупликация scoped к документу. Нулевой CAS, serialization failure или deadlock отменяют всю транзакцию: actor загружает tail, заново авторизует, валидирует, связывает ссылки, преобразует и готовит candidate. Нельзя повторять только последний SQL. Retry ограничен, затем typed retryable conflict.
2. **Candidate до commit.** Canonical operation сначала применяется к изолированному candidate/delta; проверяются инварианты и вычисляется новый content hash. После COMMIT выполняется подготовленная атомарная замена actor projection, без нового fallible domain apply. Crash восстанавливается replay. Нарушение после commit переводит документ в quarantined, прекращает команды и запускает проверяемое восстановление; неизвестный исход COMMIT разрешается по тому же operation_id, без ложного rejected.
3. **Раздельные сообщения.** `OperationCommitted` подтверждает authored state без calculated cache. Асинхронный `CalculationCommitted` несет `input_version`, `calculation_epoch`, `context_id`, engine/profile, `patch_sequence`, `is_final` и result hash. Устаревшие patches не становятся текущими; новый расчет отменяет либо supersede старый. WASM preview не становится confirmed без server event. Обязательного recalculation_patch в ACK нет.
4. **Persisted calculation context.** `CalculationContext` содержит context ID, epoch, UTC instant, workbook timezone, seed, engine/profile. Только отдельный durable system event `AdvanceCalculationEpoch` меняет контекст; seed/time не копируются в каждую пользовательскую операцию. RAND получает deterministic key из document/epoch/cell/seed и стабильного AST node ID для нескольких RAND в одной формуле. Порядок workers не влияет на результат.
5. **Binding на base version.** Transformation window: последние 10 000 document versions и максимум 16 MiB структурных записей на активный документ, что наступит раньше. В памяти хранится axis checkpoint на lower bound и последующие structural events; оси `base_version` восстанавливаются replay от checkpoint, включая sheet names и tombstones. Formula text парсится и связывается сервером на этой версии, затем references преобразуются до head. Слишком старый base дает `RebaseWindowExceeded`/`resync_required=true`. Клиентский AST только hint. Snapshot/log archival не расширяют неявно это окно; durable log в MVP не compacts/deletes.
6. **Отдельные reference semantics.** Stable targets и `Relative { original_delta }`/Absolute отделены. Copy смещает относительные ссылки; move/cut сохраняют bindings формулы и преобразуют входящие ссылки по content relocation map; insert меняет A1 без смены target; delete дает typed REF. `original_delta` нормализуется при структурных изменениях относительно новой позиции origin, иначе последующее копирование стало бы неверным. SortRange исключен из раннего MVP: будущая атомарная сортировка переставляет содержимое только прямоугольника. SortSheet — отдельная будущая команда. Подробные будущие контракты требуют подтверждения перед соответствующим срезом.
7. **Access epoch и отзыв подписок.** Document и membership epochs входят в серверный authorization ticket. Упорядоченная control lane блокирует payload gate существующих подписок, транзакционно меняет ACL/epoch и пишет authorization control event, затем удаляет старые outbound payloads и требует повторной авторизации. Epoch проверяется перед каждым новым send. Уже переданные транспорту bytes отозвать невозможно; граница revoke линейризуется с send gate. Успешный idempotent retry возвращает только operation_id, committed_version, canonical_event_hash и current_head_version после новой проверки прав.
8. **Зависимости без цикла.** `spreadsheet-application -> domain` содержит используемые ports/use cases. `collab -> application + domain (+ formula)`, `storage -> application + domain (+ storage-codec)`. Wire protobuf и persisted event encoding имеют отдельные schemas/жизненные циклы; отдельный storage-codec crate появляется с первым persisted event в фазе 1. В phase 0 только реально используемый BlobStore port, без пустых OperationJournal/Clock/CalculationExecutor traits. Diagnostic AST/spans не входят в versioned persisted AST.
9. **Replay/live barrier.** Actor в своей очереди фиксирует barrier V, регистрирует subscriber для >V, затем читает replay `(last_seen,V]`. Live события буферизуются с byte/message caps. После replay отправляется `ReplayComplete(V)`, затем buffered events >V в порядке версий; только после marker клиент переходит на live. Gap, overflow или устаревшее окно ведут к checkpoint/resubscribe, без потери окна между DB read и subscription.
10. **Blob storage.** Phase 0 содержит BlobStore и LocalFilesystemBlobStore с contract tests. MinIO не используется; Garage/S3 service в Compose отсутствует. Phase 5 вводит S3BlobStore и общий contract suite для filesystem, Garage 2.4.0 и выбранного российского S3. Garage — кандидат, а не подтвержденная совместимость.

### Дополнительные уточнения

- Snapshot lifecycle: UPLOADING → STORED → VERIFIED → ACTIVE либо REJECTED. Проверяются объект и восстановленное состояние; activation проходит короткую DB-транзакцию. Ранее активированные поколения остаются доступными для fallback. GC отдельно удаляет orphan/incomplete/retired объекты только после retention; ACTIVE metadata без доступного проверенного объекта не создается.
- Axis хранит ordered IDs, position index и visible extent. Начало 1000×26 материализует только оси. ExtendRows добавляет следующие 1000 IDs; UI автоматически запрашивает расширение у края, пустые cells не создаются и extent не сжимается по удалению содержимого.
- Range dependencies получают reverse row-interval index с column filtering. Phase 2 caps: 100 000 cells/range, 256 ranges/formula, 1 000 000 deterministic evaluation work units/job; превышение — typed CalculationLimitExceeded, не неверное значение. Пороги — проектные лимиты для будущей проверки, не измеренный результат.
- Детерминированная арифметика: fixed order, без fast-math и reassociation, SUM в workbook row-major order. Первый `^` ограничен целым exponent в [-1024,1024] с фиксированным exponentiation-by-squaring; прочие exponent дают typed unsupported-profile diagnostic. Portable math dependency вводится только отдельно. Convergence, Excel compatibility и bitwise Excel parity — разные обещания; последнее не делается.
- Отдельные caps: command frame 1 MiB, batch 10 000 cells, decoded command 4 MiB, persisted event 1 MiB, broadcast frame 1 MiB, outbound 128 messages/8 MiB, calculation budget отдельно. Preimages и envelope overhead учитываются до commit. В первой реализации принимаются только операции, укладывающиеся во все caps; chunked delivery не заявляется реализованным.
- XLSX worker: Linux cgroups v2/process groups/FD limits, Windows Job Objects; versioned framed stdin/stdout; без DB/S3 credentials; разрешены только входной файл и private temp directory. Parent убивает всю группу и повторно валидирует IR. Реализация только в phase 6.
- Candidate toolchain: версии прежнего исследования еще не являются tested baseline. Pinned Rust candidate 1.98.1; MSRV policy not established before production v1. Combined native/WASM, SQLx/PostgreSQL, Rust/TS protobuf и browser toolchain проверяются реальными командами; новые доказательства записываются отдельно.
- Performance gates: PostgreSQL synchronous_commit/fsync/full_page_writes=on, release build, warm pool, описанная топология. Queue wait, validation, checkout, transaction/commit, broadcast enqueue и E2E ACK измеряются раздельно. Shared CI runner не проверяет жесткий p95; официальный порог требует закрепленного стенда. Реальных результатов пока нет.

### Изменения документации и проверка этого среза

Прочитан весь прежний RUNBOOK, включая матрицу операций. Применены навыки documentation-and-adrs и api-and-interface-design. Текущие документы: `docs/architecture.md`, `roadmap.md`, `collaboration.md`, `persistence.md`, `formula-semantics.md`, `compatibility.md`, `threat-model.md`, `testing.md`; ADR-001…005 фиксируют принятые foundation decisions, ADR-006 — изменения после аудита. Полная актуальная матрица операций находится в `docs/collaboration.md`; историческая ниже не является текущим контрактом.

Исполняемые изменения, установки, команды, проверки и ограничения phase 0 записываются последующими отдельными записями. Эта запись не утверждает прохождение сборки, CI, тестов или benchmarks.

Документационный срез завершен: созданы восемь тематических документов и шесть ADR (001–006). Исполнены `Get-Content` для полного чтения прежнего RUNBOOK/навыков, `rg --files docs`, поиск ключевых отмененных решений и PowerShell-проверка относительных Markdown links. Результат link check: 14 documentation files, no broken local Markdown links. Код, manifests и root AGENTS/README этим срезом не изменялись; unit/build/browser checks относятся к последующей реализации и здесь не запускались.

---

## Историческая запись: 2026-09-28 — первоначальное архитектурное предложение

**SUPERSEDED: исторический proposal. Следующий текст не является текущей спецификацией.** Его сохраняем для прозрачности перехода; действуют принятые исправления выше и документы `docs/`.

Статус: PROPOSED, ожидает подтверждения пользователя. Это исследование и проектирование, не реализация. Создан только этот документ; код, manifests, миграции, Git-репозиторий и окружение не создавались. Корень предлагаемого monorepo: `C:\projects\PXEL`. Обнаружен также пустой каталог `pxel-dev-v01`; он не изменялся.

### Цель и ограничения этапа

Подготовить архитектуру modular monolith, границы crates, operation flow, модель identity, начальный профиль формул и проверяемые критерии фаз 0–2. Фаза 3 описана дополнительно как первый collaboration gate. Все 20 инвариантов из задания остаются обязательными. Особое внимание: durable commit до ACK; стабильные ID; воспроизводимый replay; разделение confirmed/pending; отсутствие содержимого документов в логах; независимость domain от transport и XLSX.

Применены think-tank для сопоставления компромиссов и api-and-interface-design для контрактов. Рассмотрены позиции надежности хранения, Excel-совместимости, отзывчивости UI и эксплуатационной простоты. Выводы: actor начинается в фазе 1; commit отделен от завершения расчета; сортировка полного ряда и сортировка прямоугольника требуют разных семантик; жесткая изоляция импорта требует процесса. Дополнительные остановки внутри навыков не применялись: пользователь прямо запросил полное архитектурное предложение с одним подтверждением после ответа.

### Что фактически проверено

- `Get-Location`, `Get-ChildItem -Force`, `rg --files` с фильтрами AGENTS/RUNBOOK/roadmap/architecture/manifests/ADR и затем с `--hidden`: исходников и проектных инструкций не найдено.
- `git status --short --branch` в корне и `git -C ...\pxel-dev-v01 status --short --branch`: оба завершились с сообщением, что каталог не является Git-репозиторием. Это состояние окружения, не ошибка сборки.
- Прочитаны локальные SKILL.md: using-agent-skills, think-tank, api-and-interface-design, planning-and-task-breakdown. Отдельное перепланирование уже заданных фаз не выполнялось.
- Через web проверены официальные релизы Rust, React, TypeScript, Vite, PostgreSQL, Node.js, Buf; документация Tokio, SQLx, protobuf, Rust f64, Microsoft Excel; состояние MinIO и релизы Garage.
- PowerShell `Invoke-RestMethod` прочитал публичные metadata npm `/latest` и crates.io `/api/v1/crates/<name>`; для crates выбраны стабильные, не yanked версии. Ничего не установлено.
- `Get-Command`: в PATH доступны node, git, wsl; rustc, cargo, pnpm, docker, just отсутствуют. Это не проверка всех возможных каталогов установки.
- `node --version`: `v24.18.0`. `wsl --list --quiet`: подсистема Windows Subsystem for Linux не установлена.
- Unit/integration/browser/compatibility/recovery tests, сборки и benchmarks НЕ запускались: исполняемого проекта еще нет. Реальных результатов производительности нет.

### Проверенные версии-кандидаты

Это результаты исследования на 2026-09-28, а не установленный или уже прошедший совместную сборку набор. На старте фазы 0 проверить актуальность patch releases, зависимости и лицензии, затем закрепить версии и digest. MSRV предлагаемого проекта — 1.98.1; edition 2024; одинаковый toolchain native/WASM. Более низкий MSRV без потребителя сейчас не обещается.

| Компонент                    | Проверенная версия            | Источник                                                                                                            |
| ---------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Rust                         | 1.98.1                        | https://blog.rust-lang.org/2026/09/03/Rust-1.98.1/                                                                  |
| PostgreSQL                   | 18.6                          | https://www.postgresql.org/support/versioning/                                                                      |
| React / react-dom            | 19.3.0                        | https://react.dev/versions ; npm metadata                                                                           |
| TypeScript                   | 7.0.2                         | https://registry.npmjs.org/typescript/latest ; https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/ |
| Vite                         | 8.3.1                         | https://registry.npmjs.org/vite/latest ; https://vite.dev/releases                                                  |
| Node.js                      | 24.21.0 LTS; 26.10.0 Current  | https://nodejs.org/en/about/previous-releases ; https://nodejs.org/dist/index.json                                  |
| pnpm                         | 12.6.0                        | https://registry.npmjs.org/pnpm/latest                                                                              |
| Tokio / Axum                 | 1.53.1 / 0.8.9                | crates.io metadata                                                                                                  |
| Tower / tower-http           | 0.5.3 / 0.7.1                 | crates.io metadata                                                                                                  |
| SQLx                         | 0.9.0, dependency MSRV 1.94.0 | https://docs.rs/sqlx/latest/sqlx/ ; crates.io metadata                                                              |
| Serde / Prost                | 1.0.229 / 0.14.4              | crates.io metadata                                                                                                  |
| Buf CLI                      | 1.73.0                        | https://github.com/bufbuild/buf/releases/tag/v1.73.0                                                                |
| protobuf-es / protoc-gen-es  | 2.15.0 / 2.15.0               | npm metadata                                                                                                        |
| uuid / tracing               | 1.26.1 / 0.1.44               | crates.io metadata                                                                                                  |
| thiserror / anyhow           | 2.0.21 / 1.0.104              | crates.io metadata                                                                                                  |
| rustls                       | 0.23.45                       | crates.io metadata                                                                                                  |
| proptest / criterion / insta | 1.11.0 / 0.8.2 / 1.48.0       | crates.io metadata                                                                                                  |
| wasm-bindgen                 | 0.2.129                       | crates.io metadata; adapter only, фаза 2                                                                            |
| quick-xml / zip              | 0.42.0 / 8.6.0                | crates.io metadata; кандидаты для фазы 6                                                                            |
| Vitest / Playwright          | 5.0.2 / 1.63.0                | npm metadata                                                                                                        |
| ESLint / Prettier            | 10.11.0 / 3.9.9               | npm metadata                                                                                                        |
| Garage                       | 2.4.0                         | https://garagehq.deuxfleurs.fr/_releases.html                                                                       |

Metadata исследованных Rust crates указывает MIT/Apache-2.0 и варианты разрешительных лицензий; rustls также предлагает ISC. Это не аудит транзитивных зависимостей. До добавления каждой зависимости проверяются upstream, лицензия выбранной версии и features, MSRV, advisories и необходимость. cargo-nextest, cargo-deny, cargo-audit, just и дополнительные codegen plugins закрепляются после отдельной проверки при установке в фазе 0. Будущие crates не добавляются заранее.

### Уточнения требований и предлагаемые решения

1. Document actor указан в фазе 3, но последовательное durable применение нужно уже в фазе 1. В фазе 1 вводится минимальный actor; в фазе 3 добавляются подписки, presence и полноценный reconnect.
2. `accepted` не означает durable. Терминальный успешный ответ — только `committed`; состояние приема в очередь не считается успехом. Расчет может завершиться после ACK, с отдельным `calculated_version`.
3. Один ordered log недостаточен для повторения NOW/RAND: event содержит версию engine/profile, calculation epoch, время/дату и seed, когда они нужны. Системный Recalculate является явным durable event, а не скрытым таймером.
4. `f64` сам по себе не гарантирует побитовый native/WASM parity всех математических функций. Для POW и будущих функций нельзя полагаться на платформенную libm. Нужен один фиксированный переносимый алгоритм, одинаковый порядок редукций и differential gates; точность относительно Excel проверяется отдельно.
5. Protobuf serialization не является канонической: https://protobuf.dev/programming-guides/serialization-not-canonical/ . SHA-256 сохраненных байтов защищает payload; domain state hash строится по отдельной версионированной канонической кодировке.
6. Stable identity не задает Excel-семантику копирования, перемещения и сортировки. `$` ограничивает относительное копирование, но не запрещает структурное преобразование ссылки при вставке. Все преобразования получают отдельные fixtures.
7. Перестановка row IDs перемещает всю строку, включая ячейки за пределами выбранных колонок. Предлагаемый первый SortRange сортирует полные строки выбранного интервала; частичный прямоугольник получает UnsupportedSortScope. Это явно частичная Excel-совместимость.
8. Вектор всех потенциальных Excel-строк на каждом листе не нужен. MVP хранит только выделенный extent, отдельные пустые Cells не создаются. Превышение лимитов дает typed error, а не молчаливое обрезание.
9. Жесткий лимит RAM/time для недоверенного XLSX невозможно обеспечить обычным spawn_blocking. Tokio описывает невозможность abort уже запущенной blocking task: https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html . С фазы 6 нужен дочерний Linux-процесс/контейнер с OS-лимитами, запускаемый тем же server binary в worker-режиме; это не отдельный сервис продукта.
10. MinIO community архивирован и объявлен неподдерживаемым: https://github.com/minio/minio . Предлагается Garage 2.4.0 для локального Compose, за тем же узким S3-контрактом. Это изменение явно заданного baseline, ожидает выбора пользователя. Образ и лицензия Garage должны быть проверены до его добавления. Альтернатива — согласованная закрепленная локальная сборка MinIO без обещания production-support.
11. Запрет unsafe относится к собственному коду. Запрет всех транзитивных unsafe несовместим с обычным Rust runtime/TLS stack; внешние реализации рассматриваются аудитом зависимостей, а не скрытым снятием forbid в нашем коде.
12. cargo-fuzz/libFuzzer обычно требует nightly toolchain. Основные сборки остаются stable; первоначально proptest и corpus mutation на stable. Изолированный nightly fuzz job возможен только после явного обоснования исключения, без nightly runtime.
13. Append-only log и retention совместимы при явной политике удаления документа/tenant и его backups. В MVP не включается автоматическое удаление журналов. Retention живого документа не подменяет журнал snapshot-файлом.

### Архитектура и bounded contexts

Один основной Axum binary; PostgreSQL — durable authority; S3 — blobs. React main thread содержит Canvas/UI/input, worker содержит долгоживущую WASM-проекцию и preview. Rust calculation выполняется в ограниченном CPU pool вне async IO executor. Микросервисы, Kafka, NATS, Redis, distributed leader election и sequence CRDT отсутствуют.

| Контекст / crate     | Ответственность                                                                                                                  | Зависимости внутри workspace                                  | Появляется                                                        |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- | ----------------------------------------------------------------- |
| spreadsheet-domain   | Типизированные IDs, значения, normalized AST data types, workbook, операции, чистое применение и canonical encoding              | Нет                                                           | Фаза 0: только используемые identity/roles; модель книги в фазе 1 |
| spreadsheet-protocol | Версионированные transport и persisted protobuf schemas/codecs, ограничения декодирования                                        | Нет                                                           | Фаза 0: Hello/Welcome/errors                                      |
| spreadsheet-collab   | Document actor, validation/rebase/undo, SessionDirectory, интерфейс атомарного journal commit                                    | domain; formula с фазы 2                                      | Фаза 1                                                            |
| spreadsheet-storage  | SQLx, atomic commit, ACL barrier, checksums/replay, затем S3 snapshots/jobs                                                      | domain, protocol; collab для реализации journal port с фазы 1 | Фаза 0: миграции и readiness                                      |
| spreadsheet-formula  | Lexer/parser, syntax AST, resolver, coercion, registry, dependencies, scheduler/evaluator, locale/formatter, diagnostics/profile | domain                                                        | Фаза 2                                                            |
| spreadsheet-wasm     | Тонкий бинарный commands/patches adapter для domain/formula                                                                      | domain, formula, protocol                                     | Фаза 2                                                            |
| spreadsheet-testkit  | Seeded generators, виртуальные клиенты, fault injection; только dev dependency                                                   | domain, collab; formula по мере использования                 | Фаза 1 при общем использовании тестами                            |
| spreadsheet-xlsx     | ZIP/OOXML и отдельная normalized import/export IR; ограниченные conversions в domain                                             | domain, formula                                               | Фаза 6                                                            |
| apps/server          | Composition root, HTTP/WS, auth adapters, limits, runtime, tracing/optional OTel, worker entrypoint                              | Нужные crates                                                 | Фаза 0                                                            |

domain содержит форму persisted AST, но не вычислитель; formula содержит синтаксический AST/parser и lowering в domain AST. Так избегается цикл domain ↔ formula. protocol не импортирует domain; явные conversions выполняются на серверной, storage и WASM границах. XLSX IR не становится workbook model. collab не зависит от PostgreSQL; storage реализует его commit port. Engine не зависит от collab/storage/protocol/UI/wasm-bindgen.

Порты вводятся по реальным потребностям: journal — PostgreSQL и deterministic fault-injection implementation; evaluation context — сервер и WASM/fixtures; S3 blobs — snapshots и import/export; SessionDirectory предусмотрен явным требованием пользователя. Порядок строк скрыт за конкретным AxisOrder с узким API, без trait для единственной реализации.

### Server-authoritative operation flow

1. UI немедленно помещает intent в pending queue и отображает optimistic overlay. Confirmed projection при этом не меняется.
2. WS decoder проверяет размер, версию envelope и operation kind. AuthContext формируется сервером. actor_id из команды сверяется с principal и не используется как доказательство полномочий. client_instance_id уточняет scope client_sequence между вкладками.
3. SessionDirectory направляет команду в единственный actor документа через bounded queue. Проверяются tenant/document ACL и durable operation_id. Повтор с тем же fingerprint возвращает ранее committed результат; другой payload под тем же ID получает IdempotencyConflict. Ответ другого tenant никогда не раскрывается.
4. Проверяется base_version: будущая версия отклоняется; устаревшая команда преобразуется только по документированному правилу. Обращение к удаленному ID дает RowDeleted/ColumnDeleted. A1 в формулах разрешается в контексте base_version, затем преобразуется к head. Если необходимой истории для разрешения нет — RebaseRequired, никакой тихой привязки к новым координатам.
5. До транзакции готовятся ValidatedOperation, bounded delta, inverse/preimage и изменения reference bindings. Полная копия workbook на каждую команду не нужна. Лимиты размера, количества ячеек, AST и batch проверяются до записи. Клиентский AST не считается доверенным.
6. Короткая PostgreSQL-транзакция: блокировка/проверка текущего document head; повторная проверка прав с сериализацией относительно revocation; повторная дедупликация; INSERT immutable event; UPDATE head_version; INSERT idempotency record. Уникальны operation_id и (document_id, committed_version). ACL-изменения используют совместимый барьер блокировок membership/permissions, поэтому cached ACL не допускает запись после упорядоченного revocation.
7. COMMIT с synchronous_commit=on и штатным WAL/fsync. При неизвестном исходе commit клиент получает outcome-unknown/reconnect, а повторяет ТОТ ЖЕ operation_id. Сервер сверяется с БД. Timeout не превращается в ложный rejected или rollback уже committed операции.
8. Actor применяет committed delta к projection, обновляет confirmed version и рассылает committed event/patch. Сбой после commit восстанавливается replay, в том числе если ACK потерян. Никакого следующего commit из отставшей/поврежденной projection; actor перезагружается.
9. Dirty calculation идет в bounded CPU executor. ACK может содержать calculation_status=pending; CalculatedPatch содержит document version, engine/profile и epoch. Старый результат не накладывается на новую версию. В первой реализации actor завершает расчет текущей версии перед обработкой следующей mutating command, сохраняя короткую DB-транзакцию и работу других документов.
10. Клиент применяет canonical events строго последовательно, удаляет свой подтвержденный intent и заново накладывает оставшуюся pending queue. При gap запрашивает replay; дубликаты не применяются повторно. ACK и broadcast имеют один operation_id/version, поэтому не удваивают правку.

Один batch = одна проверка, одна транзакция, один committed_version, ноль частичных применений. Rejected команды не входят в основной журнал. Changed cells — IDs/content delta, recalculation patch — отдельные derived values; большие ответы передаются bounded chunks с manifest/hash.

Начальные конфигурируемые ограничения, НЕ результаты benchmark: actor queue 256 команд; pending на вкладку 32; WS frame 1 MiB; outbound 128 сообщений и 8 MiB, что наступит раньше; atomic paste 10 000 cell mutations в пределах лимита bytes; heartbeat 15 s, разрыв после 45 s; DB statement timeout 5 s. Все лимиты включают тесты на границах. Payload не копируется без учета байтов. Slow consumer отключается и восстанавливается через replay; presence можно коалесцировать/терять, committed события — только восполнять из БД. При отзыве чтения подписка закрывается, новые данные не отправляются.

Subscribe захватывает согласованный cut V: snapshot/projection на V плюс tail от V+1 и live-подписка без окна потери событий. До persisted snapshots фазы 5 сервер может выдать transient, версионированный и chunked checkpoint своей projection; он не является durable backup. Для fallback превышенный replay window заменяется checkpoint, после чего pending intents перебазируются.

Protocol policy: major version согласуется handshake; неизвестная major/operation version или отсутствующий recognized oneof отклоняются целиком. Optional поля добавляются совместимо, tags не переиспользуются; обязательная новая семантика требует capability/version gate. Неизвестное durable событие останавливает восстановление документа с контролируемой ошибкой. SQLx использует query!/query_as!, .sqlx metadata и prepare --check на реальной мигрированной PostgreSQL; offline build не заменяет integration tests.

### Identity и модель книги

- Workbook: DocumentId/WorkspaceId, ordered SheetIds, sheets, date system, compatibility/engine profile, declared extent/limits и document version.
- Sheet: StableRowId/StableColumnId, AxisOrder, sparse cells, styles, named ranges, merges, document SheetView defaults.
- AxisOrder: Vec<StableId> и map ID→position; начальный extent 1000×26, рост explicit structural operation. Начальный проверяемый масштаб — до 100 000 rows, 1000 columns, 100 000 nonempty cells, 10 sheets и не более 1 млн allocated axis IDs суммарно; лимиты остаются предлагаемыми до benchmark. Полный Excel extent не обещается.
- CellKey: (SheetId, StableRowId, StableColumnId). A1 — только представление текущей позиции. Пустые Cell отсутствуют; запись с одним стилем допустима. Deleted IDs не переиспользуются; undo восстанавливает именно старую identity специальным событием.
- Cell: отдельно content (literal или Formula), style reference и property writer metadata. Blank отличается от Text(""); ClearCell по умолчанию удаляет content, style очищается явной style operation.
- Formula: исходный ввод, canonical text, normalized bound AST, AST/profile version. AST references содержат stable target IDs и отдельные absolute/relative flags для каждой оси; смещение при copy считается по текущим позициям origin/target. Ссылки между листами содержат SheetId, не имя.
- Диапазон: прямоугольник с anchors и граничной affinity, а не список миллиона cell IDs. Вставка внутри расширяет диапазон; удаление границы сжимает к следующей живой позиции; удаление всего диапазона дает #REF!. Эти правила преобразуют anchors явно.
- CellStyle — нормализованный interned record. Patch стиля меняет указанные поля; value и style независимы. Крупные range styles не создают Cells на всю потенциальную площадь; MVP ограничивает площадь операции, позднее вводит доказанную benchmark структуру runs.
- NamedRange связывается с SheetId/range anchors; MergeRegion не уничтожает скрыто значения: первоначально merge разрешен только если остальные ячейки пусты. SheetView хранит общие frozen panes/filter defaults; личный viewport, scroll, selection и presence туда не попадают.
- Snapshot — versioned binary envelope с domain data; graph/caches можно восстановить. Formula results отдельно от authored content; пользователь не может задавать canonical cached result.

State hash: SHA-256 canonical domain encoding v1 с фиксированными type tags, length prefixes, порядком байтов, сортировкой maps, ordered axes и нормализованными f64. Presence, pending, runtime caches, transport order и audit/writer metadata исключены из content hash. Calc result hash отдельно включает нормализованные values плюс engine/profile/context; сравнивается только для завершенного расчета одной версии. Версия и hashes передаются раздельно; checksum persisted bytes не заменяет их.

### Конфликты и матрица операций

Все строки ниже — контракт к последующей реализации, а не доступные операции. E означает editor или owner в текущем tenant/document, R — read право дополнительно для source. Во всех persisted events: operation_id, principal, client_instance/sequence, committed_version, payload schema, checksum, engine/profile при необходимости. P означает восстановление preimage с проверкой, что затронутые свойства не перезаписаны чужой операцией. Inverse — новая server-validated compensating operation, никогда не уменьшение head. Полный inverse может быть внутренним Restore* событием, а не пользовательской командой исходного набора.

| Операция       | Preconditions                              | Validation                                     | Transform / stale base                                    | Inverse                                                | Dependency nodes                         | Auth                            | Persisted payload                                                   |
| -------------- | ------------------------------------------ | ---------------------------------------------- | --------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------- | ------------------------------- | ------------------------------------------------------------------- |
| SetCellValue   | Живой target                               | Тип/размер/finite number                       | Stable ID, server-order content write                     | P content                                              | Target и downstream                      | E                               | Key + value + preimage                                              |
| SetCellFormula | Живой target, доступный base context       | Parse spans, AST limits, resolver              | Resolve на base, transform references до head             | P content                                              | Удалить старые edges; новые + downstream | E                               | Source + bound AST/version + preimage                               |
| ClearCell      | Живой target                               | Content-clear mask                             | Stable ID; независим от style                             | P content                                              | Target и downstream                      | E                               | Key + mask + preimage                                               |
| SetCellStyle   | Живой target                               | Allowlisted style fields                       | Server order отдельно по style fields                     | P измененных полей                                     | Нет, пока нет style-dependent functions  | E                               | Key + field patch + preimage                                        |
| SetRangeStyle  | Живой bounded rectangle                    | Площадь/bytes/style fields                     | При изменении границ retryable structural conflict        | P field patches                                        | Нет                                      | E                               | Canonical range + patch + preimages                                 |
| InsertRows     | Живой before anchor или end                | Count, caps, ID uniqueness                     | Перед stable anchor; concurrency упорядочена commit order | Delete новых IDs при отсутствии чужих зависимых правок | Axis/range/reference nodes               | E                               | Новые IDs + resolved anchor                                         |
| DeleteRows     | Все перечисленные IDs живы                 | Bounds; merge/reference transform              | IDs сохраняют смысл; повтор под новым op ID отклоняется   | RestoreRows с прежними IDs и P                         | Axis, references, ranges, downstream     | E                               | IDs + anchors + bounded preimages/transforms                        |
| InsertColumns  | Живой before anchor или end                | Count/caps/ID uniqueness                       | Как InsertRows                                            | Guarded DeleteColumns                                  | Axis/range/reference nodes               | E                               | Новые IDs + anchor                                                  |
| DeleteColumns  | Живые target IDs                           | Bounds/merge/reference transform               | Как DeleteRows                                            | RestoreColumns + P                                     | Axis, references, ranges, downstream     | E                               | IDs + anchors + preimages/transforms                                |
| ResizeRow      | Живой row ID                               | Допустимый размер                              | Last committed property write                             | P height                                               | Нет                                      | E                               | Row ID + size + prior size                                          |
| ResizeColumn   | Живой column ID                            | Допустимый размер                              | Last committed property write                             | P width                                                | Нет                                      | E                               | Column ID + size + prior size                                       |
| RenameSheet    | Живой SheetId                              | Имя, длина, workbook uniqueness                | Identity остается; name collision rejected                | P name                                                 | Name resolver/formatter, не numeric DAG  | E                               | ID + old/new name                                                   |
| AddSheet       | Книга не превышает caps                    | IDs/name/extent limits                         | Stable anchor, commit order                               | Guarded DeleteSheet                                    | Workbook/name nodes                      | E                               | ID + name + initial axes + anchor                                   |
| DeleteSheet    | Лист жив, останется минимум один           | Bounds, preimage size, references              | Concurrent target use -> typed rejection/rebase           | RestoreSheet + P                                       | Cross-sheet references + downstream      | E                               | ID + data/preimages + reference transforms                          |
| ReorderSheet   | Sheet и anchor живы                        | Не self anchor                                 | Stable anchor, server order                               | Restore prior order при guard                          | Только order, 3D refs не поддержаны      | E                               | ID + resolved anchor + prior order                                  |
| MergeCells     | Прямоугольник, вне top-left content пуст   | Нет несовместимого overlap; caps               | Изменение геометрии/содержимого -> conflict               | Guarded Unmerge                                        | Layout, без переноса content             | E                               | Merge ID + anchored rectangle                                       |
| UnmergeCells   | Merge ID существует                        | Revision merge совпадает                       | Пересозданный merge -> conflict                           | Guarded Merge прежней области                          | Layout                                   | E                               | Merge ID + preimage                                                 |
| MoveRange      | Живые source/destination                   | Geometry/limits; несовместимые merges rejected | Смена read/write set -> retryable conflict                | P обеих областей и ссылок                              | Src/dst, входящие ссылки, downstream     | E+R                             | Frozen source delta + destination + ref transforms + P              |
| CopyRange      | Живые source/destination                   | Geometry/limits/copy mode                      | Source читается на base; если недоступен — retry          | P destination                                          | Destination + transformed formula refs   | E+R                             | Frozen copied content + transformed AST + P                         |
| SortRange      | Полные строки интервала, header явно задан | Keys/types/limits; merges rejected             | Изменение sort inputs/структуры после base -> retry       | Guarded inverse permutation + ref P                    | Axis/range/formula reference nodes       | E                               | Конечная row-ID permutation + ref delta; replay не сортирует заново |
| SetFrozenPanes | Живой sheet                                | Bounds и лимиты                                | Server order для document view property                   | P frozen panes                                         | Нет                                      | E                               | Sheet ID + stable boundaries + prior                                |
| OperationBatch | Все дочерние preconditions                 | Общие caps, все или ничего                     | Rebase/validate всего batch до записи                     | Guarded inverse в обратном порядке                     | Объединение dirty sets                   | E и права всех children         | Один envelope + children + preimages                                |
| Undo / Redo    | Исходная операция принадлежит actor        | Текущие property/dependency guards             | Чужая последующая правка -> UndoConflict                  | Новая компенсация, redo тоже новое событие             | По компенсируемым изменениям             | E + ownership                   | compensates_operation_id + canonical inverse                        |
| Recalculate    | Живой документ                             | Доступный engine/profile, context limits       | Явный calc epoch                                          | Не обычный пользовательский undo                       | Volatile nodes + downstream              | System или E для явного запроса | Context/seed/time/profile                                           |

Прямые references в MVP следуют stable target identity при перестановке целых строк. Это продуктовая семантика, не заявление о полной Excel-совместимости сортировки. Конкурентная запись после принятой сортировки идет по row ID; stale sort при изменившихся ключах дает retry. Для undo insert/delete/move/sort guard охватывает также структурные зависимости и новые чужие references, а не только последнее значение ячейки. Если восстановление затронет чужую работу, undo отклоняется целиком. Big preimages должны укладываться в лимиты операции; крупные imports идут через job/publication flow, а не бесконечный WS batch.

### Начальный профиль формул: pxel-excel-subset-v1

Статус ниже означает целевую поддержку после tests, не уже реализованную. Первая поставка формул в фазе 2: literals, operators, A1/mixed absolute refs, bounded ranges, cross-sheet parser fixtures, SUM/AVERAGE/MIN/MAX/IF. COUNT/COUNTA и остальные группы добавляются последовательными срезами после этого gate.

| Категория         | Контракт                                                                                                                                                                                                                                      |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Синтаксис         | Начальный =; numeric/string/boolean/error literals; unary +/-; %, ^, */+-, &, comparisons; parentheses; calls; A1/$A$1/A$1/$A1; A1:B10; quoted sheet names с doubled apostrophe                                                               |
| Locale            | Канонический английский, точка и запятая аргументов; RU adapter для UI выполняет token-aware преобразование, не replace внутри strings; persisted текст для вычислений канонический                                                           |
| Value             | Blank, finite Number, Text, Boolean, DateTime(serial/date-system), Array; Reference только intermediate; Error как значение                                                                                                                   |
| Errors            | NULL, DIV/0, VALUE, REF, NAME, NUM, N/A, SPILL и CYCLE; unsupported syntax — diagnostic/rejection, не panic                                                                                                                                   |
| Числа             | IEEE-754 binary64; -0 → +0; nonfinite input rejected; computed NaN/infinity → NUM, division by zero → DIV/0; fixed evaluation/reduction order; без fast math/FMA reassociation; subnormals сохраняются и отмечаются как граница Excel-профиля |
| POW и функции     | Единая переносимая математическая реализация native/WASM; выпуск ^ блокирован до differential fixtures. std powf с unspecified precision не является достаточным доказательством parity                                                       |
| Coercion          | Правила отдельно для operators, direct arguments и references/ranges; Blank не равен пустому тексту во всех контекстах; no universal to_number                                                                                                |
| IF                | 2–3 аргумента; скалярный lazy выбор ветви; coercion logical_test и omitted/empty branch покрываются отдельными fixtures                                                                                                                       |
| Aggregates        | SUM/AVERAGE/MIN/MAX: 1–255 аргументов; диапазоны обходятся детерминированно; text/bool/blank в ссылках не считаются числовыми, zero считается; direct arguments имеют отдельные rules/tests                                                   |
| Даты              | Система книги 1900 или 1904; finite binary64 serial days, часть дня — fraction; serial 60 в 1900 сохраняет фиктивное 1900-02-29 отдельной семантикой, без конвертации в невозможный Gregorian date; timezone не берется из ОС                 |
| Volatile, позднее | NOW/TODAY используют injected civil time/date; RAND — seed/epoch, versioned PRNG и стабильный key на cell/node, чтобы dirty order не менял результаты; context сохраняется для replay                                                         |
| Пока вне профиля  | Array literals/spill evaluation, implicit intersection, structured/3D refs, whole-column/whole-row syntax, external refs, UDF, iterative circular calculation, LET/LAMBDA и lookup/criteria/date/text groups до своих срезов                  |

Минимальные примеры fixtures: =1+2*3 → 7; =-2^2 → 4; =1/0 → DIV/0; пустой A1 + 1 → 1; SUM(A1:A3) для 1, text("x"), 2 → 3; AVERAGE пустого диапазона → DIV/0; MIN/MAX пустого диапазона → 0; IF(FALSE,1/0,7) → 7; неизвестная FOO(1) → NAME; ссылка на удаленную строку → REF; SCC A1↔B1 → CYCLE. Это ожидаемые контрактные примеры, не выполненные Excel-tests.

Источники: https://support.microsoft.com/en-gb/excel/the-order-in-which-excel-performs-operations-in-formulas ; https://support.microsoft.com/en-us/excel/functions/sum-function ; https://support.microsoft.com/en-us/excel/functions/average-function ; https://support.microsoft.com/en-us/excel/functions/if-function ; https://support.microsoft.com/ru-ru/excel/date-systems-in-excel ; https://learn.microsoft.com/en-us/troubleshoot/microsoft-365-apps/excel/wrongly-assumes-1900-is-leap-year . Документация не заменяет golden fixtures Excel с версией приложения/ОС/locale; пока verified_in_excel=false. Спорные coercion/rounding cases не объявляются совместимыми до проверки.

Dependency graph содержит forward/reverse direct edges и range subscription nodes; диапазон не развертывается в миллионы edges. Range membership query и dirty propagation индексируются; первая простая реализация — range nodes по листу и проверка пересечения, измеряемая отдельно. Evaluation проходит sparse cells диапазона в фиксированном row-major order. Topological/SCC проход ограничен dirty subgraph, детерминирован и не рекурсивен на глубину цепочки. При смене формулы удаляются старые зависимости. Консервативные зависимости IF первоначально включают обе ветви; self-reference даже в невыбранной ветви считается циклом и отмечается как частичная совместимость. Runtime cancellation не превращается в значение ячейки: расчет остается pending/cancelled, предыдущие results не выдаются за текущие.

Бюджет вычисления измеряется work units и проверяется кооперативно; wall-clock watchdog обеспечивает живость, но не меняет математический результат. WASM worker загружает initial checkpoint один раз, затем принимает operations/patches. Pending preview отделен от confirmed results. Измеряются encode/decode и round-trip boundary отдельно от Rust compute. UTF-8 spans parser переводятся в UTF-16 offsets UI явным adapter.

### XLSX и безопасная совместимость

Фаза 6: ZIP → OOXML relationships/streaming XML → NormalizedImportWorkbook → validation/Formula binding → canonical document publication. Export выполняется из согласованной document version через Export IR. Import создает новый документ по умолчанию; merge в существующий документ отдельно проектируется. Серверный importer авторитетен, browser worker лишь помогает локальному preview.

| Статус профиля             | Содержимое                                                                                                                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Supported                  | Worksheets, string/number/boolean, shared strings, реализованные formulas, row height/column width, базовые styles/merges/frozen panes                                                                                   |
| Partially supported        | Number formats, autofilter subset, simple defined names, formula semantics и structural transforms только из матрицы fixtures                                                                                            |
| Preserved but not editable | Формулы с распознаваемым AST и неизвестной функцией: текст/AST сохраняются, canonical result NAME; импортированный cache помечается untrusted и не является результатом движка                                           |
| Dropped with warning       | Charts/drawings/pivots и неподдержанные безопасные formatting extensions; отчет перечисляет части/лист/ячейку вне обычных логов                                                                                          |
| Rejected                   | Encrypted files, VBA/macros/OLE/external links, malformed relationships, zip bombs, XML entities, limits violations, неподдержанная формульная грамматика/динамические массивы без безопасного normalized representation |

Не сохраняются произвольные opaque OOXML parts с обещанием безопасного round-trip. Лимиты проверяются по реально распакованным bytes, а не только ZIP headers; ограничены entries, compression ratio, глубина XML, attrs/text, relationships и paths, количество ячеек/строк/формул, CPU/RAM/time. Path traversal/duplicate entries/DTD/entities/network relationships запрещены. Malware scanning adapter запускается до parser, без внешнего SaaS. Объекты imports/exports и reports private, tenant-scoped; создание exports и права аудируются без содержимого.

### Persistence, безопасность, масштабирование

PostgreSQL tables вводятся по используемым срезам: users/workspaces/memberships/documents/document_permissions/document_operations/idempotency/audit; snapshot metadata с фазой 5, import/export jobs с фазой 6. Workbook не сохраняется единым JSON. Для первого среза authority — metadata + immutable events; relational current-cell projection не требуется до доказанной потребности. Historical reads всегда авторизованы.

Snapshot scheduler с фазы 5: получить immutable version V; сериализовать с schema/engine/profile/context и checksum; загрузить immutable S3 object; прочитать/проверить checksum и восстановление; записать verified metadata. Сбой до metadata оставляет orphan для безопасного GC. Старый verified snapshot хранится до проверки нового; corrupt snapshot → предыдущий + tail или full replay. Неподдерживаемая schema не пропускается. По умолчанию log не очищается. Local durable COMMIT не является гарантией переживания полной потери диска: production потребует WAL backup/PITR и выбранной политики репликации/RPO.

Auth: development login отдельным compile feature плюс runtime opt-in, production build его не содержит. Серверный principal, tenant scoping во всех запросах, owner/editor/viewer, проверка каждой команды и каждой subscription. Owner управляет ACL/delete document; editor редактирует структуру; viewer только читает. TLS termination как deployment boundary; cookie mode требует Origin/CSRF validation. WS имеет exact origin allowlist, no query tokens; CSP/security headers, HTTP/WS size caps и rate limiting. Secrets через env/file providers. OTel adapter выключен по умолчанию; tracing не принимает Debug workbook/command bodies. Метрики имеют bounded labels; document IDs допустимы в logs, но не в бесконтрольных metric labels. Формулы, cells, comments, clipboard, names людей, cookies/tokens в обычных logs запрещены.

Метрики: validation/commit/broadcast/recalc latency; dirty count; active sessions/clients; queue depth/overflow; rejections; replay/snapshot/import durations; snapshot bytes; reconnects; formula errors по code. Production runtime не требует внешних SaaS; локальные assets/fonts. CI shell commands переносимы на self-hosted runner в российском контуре.

Первый deployment имеет ровно один backend process. SessionDirectory in-process создает actor single-flight и выгружает idle sessions с учетом references и memory limits. DB compare-head защищает от stale writer. Будущий scale-out: маршрутизация документа к одному leader, lease + monotonically increasing fencing token, проверяемый DB в той же commit-транзакции; sticky sessions сами по себе недостаточны. Это записанный дизайн, а не инфраструктура MVP.

### План файлов без создания пустых модулей

Корневые файлы: AGENTS.md, README.md, RUNBOOK.md, Cargo.toml/Cargo.lock, package.json/pnpm-lock.yaml/pnpm-workspace.yaml, rust-toolchain.toml, justfile, docker-compose.yml, buf.yaml/buf.gen.yaml, formatting/lint/security configs. docs: architecture, roadmap, compatibility, formula-semantics, collaboration, persistence, threat-model, testing; ADR-001 authority, 002 stable axes, 003 native/WASM, 004 journal/snapshots, 005 Canvas, дополнительные ADR по deterministic math и import isolation перед соответствующим срезом. proto/pxel/v1 — public messages, proto/pxel/storage/v1 — persisted formats. apps/server и apps/web. Rust crates по таблице выше. fixtures/formulas, collaboration, xlsx; tests/integration, convergence, recovery, compatibility; benches и fuzz появляются вместе с реальными executable targets. .github/workflows вызывает те же just/shell команды, что локальная проверка. Сам по себе root tests directory не является Cargo test target: integration suites регистрируются в server/testkit manifests.

Frontend modules: viewport; axis sizes; selection; input/keyboard; clipboard; canvas; editor/formula-bar с общей state machine; collaboration; confirmed projection; pending queue; wasm worker; accessibility mirror. Основная сетка Canvas 2D, React содержит chrome и overlay. render только visible+overscan, DPR, rAF и event coalescing. Caret — DOM editor. Sparse projection вне React state; UI store небольшой useSyncExternalStore-compatible, без TanStack Query для sheet state. Размеры axis: defaults + sparse overrides, индекс prefix sums создается с feature resize и benchmark. TSV/rich clipboard ограничен размерами; a11y mirror ограничен viewport и active cell.

### Measurable gates

Общий harness фиксирует CPU/RAM/OS, native vs WSL/Docker, container limits, SSD, browser/version/DPR/viewport, build profile, dataset и random seed. Предлагаемый reference envelope: 4 CPU cores, 8 GiB RAM, SSD, release builds, viewport 1440×900, DPR 1/2. Latency: warm-up 100 commands, затем минимум 1000 samples, 5 runs, p50/p95/p99; render — 20 page loads, отдельно cold/warm assets. Цифры ниже — цели, не результаты.

| Фаза                   | Минимальный slice                                                       | Проверяемый gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0                      | Dev auth/health/readiness + protobuf handshake + empty Canvas           | `just infra-up` поднимает PostgreSQL/S3 и применяет migrations; `just dev` запускает backend+frontend. liveness 200; readiness 503 при недоступной обязательной зависимости и 200 после восстановления. Playwright получает Welcome v1, unknown version отклоняется; Canvas рисуется при DPR 1/2. p95 first usable grid <1 s после готовности данных. Нет DOM cells на полный лист. CI реально выполняет fmt/clippy/lint/typecheck/unit/build, protobuf drift/breaking check и browser smoke. Production build без dev-auth. |
| 1                      | Create document → edit value → durable WS commit → reload/restart       | Значение и stable IDs переживают reload и restart server. 100 повторов одного operation_id дают одну запись/версию; иной payload с тем же ID rejected. Сбой до commit не дает ACK/новой версии, после commit до ACK повтор возвращает исходный результат. Viewer и чужой tenant rejected. p95 simple commit <50 ms на reference стенде. Sparse fixture 100k cells открывается без proportional DOM; memory/parse/render costs записаны.                                                                                      |
| 2                      | Formula editor → Rust engine → incremental server result + WASM preview | A1+B1 и SUM/AVERAGE/MIN/MAX/IF fixtures проходят; изменение A1 пересчитывает только reachable dirty formulas. Из 10k независимых формул изменение одной не вычисляет остальные. 10k-chain завершается без recursive stack overflow, измеряются time/allocations без выдуманного порога. Cycles дают CYCLE, не hang/panic. Все общие fixtures и 1000 seeded generated workbooks имеют одинаковые normalized native/browser result hashes; parser canonical round-trip properties и 100k bounded arbitrary inputs без panic.   |
| 3, дополнительный gate | Два клиента, pending queue, presence, reconnect                         | 100 seeded сценариев по 1000 commands с duplicates/delays/disconnects/restarts заканчиваются одинаковыми version/content/result hashes. p95 optimistic update <16 ms, second-client visible delivery <100 ms. Same-cell conflict дает server order. ACK/broadcast duplicate не удваивает apply. Presence не создает WAL domain events. Slow consumer отключается, actor продолжает commits. Revocation прекращает новые команды и новые broadcasts.                                                                          |

Структурные insert/delete/sort/undo convergence tests добавляются в фазе 4 с реализацией операций; не выдаются за пройденные в фазе 3. Snapshot+10k-tail replay benchmark и crash stages object/metadata появляются в фазе 5. Excel golden fixtures требуют ручной проверки Excel и metadata окружения; до этого только документированные expected cases. Fuzz harness добавляется вместе с каждым decoder/parser.

Фаза 0 после подтверждения разбивается на последовательные рабочие проверки: (a) toolchain/workspaces/quality commands и docs; (b) Compose/migrations/server/health; (c) protocol generation/WS/browser Canvas; (d) полный clean-start и CI-equivalent gate. По каждому срезу сюда заносятся конкретные изменения, команды, результат и ограничения. До подтверждения архитектуры ничего из этого не выполняется; до прохождения gate 0 фаза 1 не начинается.

### Открытые решения

- До Compose нужно согласовать изменение MinIO baseline: рекомендуемый Garage 2.4.0 либо явно выбранная локальная MinIO-сборка. Причина — подтвержденное прекращение поддержки upstream. Критерий закрытия: выбран backend, проверены лицензия/образ и S3 smoke test.
- До фактических acceptance checks нужна рабочая Linux/Docker среда. Проверка установила отсутствие WSL и docker в PATH. Следующий этап должен сначала установить/подключить среду или зафиксировать системный blocker, если потребуется административное действие/перезагрузка. Проверки не заменяются заявлениями об успехе.
- SortRange full-row scope, stable-target semantics и ограничения размера предлагаются как часть утверждения архитектуры. Полная Excel-семантика сортировки прямоугольника не обещается и не скрывается за названием операции.

Следующее действие: дождаться подтверждения архитектуры пользователем; затем реализовать только фазу 0.

## 2026-09-28 — side task: iOS squircle для вариантов логотипа

По отдельному запросу пользователя изменены только design/logos/01–10 SVG, README этой галереи и gallery/gallery-mobile screenshots. Внешние rect с rx заменены настоящими SVG paths с cornerRadius = width × 0.2237 и cornerSmoothing = 0.6. Для варианта 07 применено к четырем отдельным плиткам. Внутренние glyphs и исходный reference сохранены. Генератор figma-squircle 1.1.0 (MIT) использован offline в ignored .local/squircle-edit, архив проверен по registry SHA-512; зависимости приложения/lockfiles не менялись. Chromium проверил 1280/390 px: 10 cards, 0 broken images, 0 page errors, нет horizontal overflow; screenshots обновлены. Основная разработка, git state и субагенты не затрагивались.
