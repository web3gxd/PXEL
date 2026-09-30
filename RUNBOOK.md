# PXEL — RUNBOOK

## 2026-09-29 — локальный PITR до named restore point

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-004/041, recovery
harness и документация PostgreSQL 18 по continuous archiving,
`pg_create_restore_point`, `recovery.signal`, `restore_command` и promotion.
Цель — восстановить книгу на выбранной точке после ACK v4 и до ACK v5.
Инварианты: журнал и snapshot должны дать canonical hash v4, поздняя v5 не
должна попасть в восстановленный head, тестовые archive files не должны
попасть в обычные логи приложения. Риски: локальная команда копирования не
равна надежному production WAL archiver; S3 online consistency не проверяется.

`tests/recovery/pitr-restore.test.mjs` поднимает отдельный PostgreSQL 18 с
`archive_mode=on` и durable настройками, публикует ACTIVE snapshot v3,
останавливает backend для копирования blobs и проверенного base backup.
Затем v4 получает ACK, PostgreSQL пишет `pxel_v4` restore point, v5 получает
ACK, сегмент WAL принудительно переключается и дожидается архивации. Новый
кластер применяет `recovery.signal` + `restore_command`, доходит до маркера
и продвигается в writable состояние. Тест сверяет head/hash v4, содержимое,
историческую v3 и runtime загрузку snapshot v3 + одного события. Решение
ADR-042; добавлен рецепт `just pitr-restore-postgres`, обновлены README,
persistence/roadmap и промежуточная презентация.

Первый прогон дошёл до recovery, но Windows `cmd copy` не принял путь с `/`
перед `%f`; архивные сегменты были на месте. Путь в test config приведён к
Windows separators. Следующий прогон восстановил точку, но backend начал
миграции во временном read-only состоянии до promotion; harness теперь ждёт
`pg_is_in_recovery() = false`. После исправлений адресный тест прошёл 1/1
за 13.31 s (время runner, не измерение RTO). Этот прогон сохранял приватный
диагностический каталог под игнорируемой `.local`; финальный рецепт без
`PXEL_TEST_KEEP_RECOVERY` должен удалить свой новый каталог. Не проверены
Linux, целевой S3, production WAL archive, off-site retention, RPO/RTO и
повреждение сегмента.
Повторный `just pitr-restore-postgres` без keep-флага прошёл 1/1 за 12.32 s
и удалил собственный тестовый каталог. `pnpm format:check`, `node --check`,
`git diff --check` и presentation check (10 слайдов, 1440×900 и 390×844)
прошли. Полный `just check` не повторялся: Rust/runtime/dependencies не
менялись, этот срез затронул JS recovery harness и документацию.

## 2026-09-29 — изолированный physical backup + snapshot restore drill

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-004/040, snapshot
recovery и текущий приватный PostgreSQL harness. Цель — реально поднять новый
кластер из физической копии PostgreSQL и сохранённых blob objects, затем
сверить canonical state. Инварианты: копия должна включать metadata и объект
ACTIVE snapshot, журнал остаётся authority, восстановленная версия не может
содержать более поздние исходные операции. Риски: этот quiescent порядок не
решает online-consistent backup, WAL archiving/PITR и off-site retention.

Общие функции приватного recovery стенда вынесены в
`tests/recovery/private-postgres.mjs` и используются двумя настоящими тестами.
Новый `tests/recovery/backup-restore.test.mjs` создает отдельный PostgreSQL 18
с `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`, публикует ACTIVE
snapshot v3 и коммитит v4. Backend останавливается, локальные blob objects
копируются, `pg_basebackup -Fp -X stream -c fast` создает физическую копию,
`pg_verifybackup -q` проверяет manifest и WAL. Исходный документ доходит до
v5, затем новый PostgreSQL из backup и копия blobs восстанавливают ровно v4.
Проверены hash v4 из ACK, значение ячейки, исторический v3 и runtime
`document_recovery` = snapshot v3 + один event хвоста. Решение ADR-041;
добавлен `just backup-restore-postgres`, обновлены README, persistence,
roadmap и живая презентация.

Реально выполнено: новый backup/restore test 1/1 за 10.99 s; прежний
immediate-stop test после выделения helper 1/1 за 6.27 s. Это время node
runner на локальном Windows, не RTO. PostgreSQL docs сверены для
`pg_basebackup`/`pg_verifybackup`. Linux, Garage/российский S3, online backup
при активных писателях, off-site copies, PITR и power-loss не проверены.
Повторный `just backup-restore-postgres` прошёл 1/1 за 9.16 s. `pnpm
format:check`, `node --check` нового теста и `git diff --check` прошли.
Presentation check прошёл все 10 слайдов при 1440×900 и 390×844. Полный
`just check` в этом срезе не повторялся: Rust/runtime и dependency graph не
менялись; затронуты только JS recovery harness и документация.

## 2026-09-29 — PostgreSQL immediate-stop recovery со snapshot + tail

Перед срезом прочитаны AGENTS.md, roadmap, ADR-004/033/037/039, snapshot
publisher/reader, server recovery composition и существующие recovery tests.
Цель — проверить подтвержденный хвост после аварийной остановки отдельной БД.
Инварианты: ACK следует durable commit, ACTIVE snapshot не заменяет журнал,
после WAL recovery canonical hash совпадает с ACK; содержимое ячеек не логируется.
Риск: `pg_ctl -m immediate` имитирует отказ процесса, но не потерю питания.

Добавлены `tests/recovery/postgres-immediate-stop.test.mjs`, рецепт
`just crash-recovery-postgres`, ADR-040, структурированное событие
`document_recovery` с версиями/count/duration и обновления README,
persistence/roadmap/презентации. Harness поднимает отдельный PostgreSQL 18
со `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`; v3 snapshot
проверяется в metadata до v4 commit. После immediate stop/restart проверяются
v4 hash и content, исторический v3 и фактическая загрузка snapshot v3 + одного
event по runtime recovery событию.

Первый запуск теста завис: захваченные stdout/stderr `pg_ctl` остались открыты
у дочернего PostgreSQL; `spawnSync` теперь не захватывает их. Второй запуск
обнаружил, что общий debug binary не собран с dev-auth; рецепт явно собирает
его. Следующий запуск выявил неверное ожидание scheduler: интервал 2 ставит
snapshot на v3, поскольку genesis v1 не считается; сценарий исправлен по
существующему правилу. Затем afterEach helper пытался отозвать test session
после остановки сервера; harness теперь отзывает ее до shutdown. После этих
исправлений адресный тест прошёл 1/1 за 7.24 s (время node runner, не latency
восстановления). Отдельная сборка `cargo build --locked -p pxel-server
--features dev-auth` прошла. Физическая потеря питания, disk corruption,
PITR/backup restore, Linux и Garage этим тестом не проверены. Прерванные
диагностические каталоги старых неуспешных прогонов остаются в игнорируемой
`.local`; успешный harness удаляет свой приватный кластер после остановки.
Повторный `just crash-recovery-postgres` прошёл 1/1 за 7.63 s. `just check`
прошёл Rust fmt/Clippy default + all-features, workspace tests/build, WASM,
protobuf drift, frontend format/lint/typecheck, Vitest 71/71 и Vite build.
Presentation check прошёл 10 слайдов при 1440×900 и 390×844. Это не
подтверждает CI на Linux или официальный latency threshold.

## 2026-09-29 — optional S3 adapter для snapshot объектов

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-004/033/037/038,
BlobStore port/контракты, server composition root и snapshot code. Цель —
подключить S3-совместимый объектный adapter без изменения PostgreSQL authority
и протокола снимков. Инварианты: immutable key, проверка фактических bytes и
SHA-256 до публикации, ACK документа только после durable PostgreSQL COMMIT,
никаких credential/body в логах, локальный adapter остаётся default.
Риски: S3-compatible серверы могут различаться в `If-None-Match: *`, metadata,
retry и durability; Moto не является подтверждением Garage/российского S3.

Добавлен feature `spreadsheet-storage/s3` и серверный `--features s3`.
`ConfiguredBlobStore` выбирает локальный adapter или S3 по
`PXEL_S3_ENDPOINT/Bucket/Region`; production credentials берутся через SDK
providers. Endpoint вне loopback требует HTTPS. S3 put stage ограничен 64 MiB,
проверяет checksum, использует условную запись, 412 → проверенный
идемпотентный retry либо `KeyConflict`, 409 → ограниченный retry. Get/Head
скачивают и проверяют весь объект во временный файл перед результатом;
delete идемпотентен. SDK operation/attempt timeouts 30/10 s, download body
timeout 60 s. Ошибки SDK наружу отображаются без содержания запросов.
Решение — ADR-039; обновлены README, architecture/persistence/roadmap,
justfile и промежуточная презентация.

Версии проверены через `cargo info`: `aws-sdk-s3 1.150.0` и `aws-config 1.12.0`
стабильны, Apache-2.0, rust-version 1.94.1; `url 2.5.8` MIT/Apache-2.0.
Первый `cargo deny check` и `cargo audit` нашли четыре advisory в старой
TLS-ветке (`h2 0.3.27`, `rustls-webpki 0.101.7`), которую притянул feature
SDK `rustls`. Флаг удалён; современный `default-https-client` использует
`rustls 0.23.45`. Повторные `cargo audit` и полный `cargo deny check` прошли:
advisories/bans/licenses/sources OK; deny сохранил informational duplicate
dependency warnings. Legacy rustls 0.21 отсутствует по `cargo tree`.

Для локального теста только в игнорируемом `.local/moto-venv` установлен
Moto 5.2.3, запущен на loopback, создан временный bucket с фиктивными
credentials. `just s3-contract` прошёл 8/8 через реальный HTTP endpoint:
streaming, limits, missing/invalid metadata, tampered body, checksum,
idempotent retry, conflict, conditional concurrent put и delete.
Первый большой async-тест на Windows завершился stack overflow; разделение
на независимые случаи устранило его при стандартном размере стека.
`cargo build --locked -p pxel-server --features s3,dev-auth` и прямой
hard-restart integration test прошли дважды с `PXEL_SNAPSHOT_INTERVAL=1`:
число ACTIVE снимков выросло 1→10, затем 10→16. Последний S3 объект имел
размер 18 602 bytes и checksum metadata; отдельный HEAD это подтвердил.
После тестов Moto-процесс остановлен.

Финальный `just check` прошёл Rust fmt/Clippy (default/all features),
workspace tests/build, WASM, proto drift, frontend format/lint/typecheck,
Vitest 71/71 и production Vite build. `just sqlx-check` прошёл.
Presentation check проверил 10 слайдов на 1440×900 и 390×844.
Первый полный check обнаружил dead-code поля S3 config в сборке без feature;
конфигурация разделена через `#[cfg(feature = "s3")]`, оба Clippy режима
повторно прошли. Не запускались Garage 2.4.0 и выбранный российский S3:
локально нет Docker/Podman/установленного WSL и нет их endpoint/credentials.
Также не проверены Linux/Compose/remote CI, power-loss/PITR и реальные S3
latency benchmarks. Фаза 5 остаётся открытой.

## 2026-09-29 — ограниченный retention старых ACTIVE snapshots

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-004/033/037,
snapshot publisher/reader, cleanup worker и миграции. Цель — ограничить
накопление старых снимков без сокращения журнала и потери fallback.
Инварианты: PostgreSQL остаётся authority; preferred snapshot не выводится;
как минимум два более новых ACTIVE снимка разных версий остаются;
историческая версия восстанавливается из журнала при отсутствии снимка.
Риски: повреждение обоих оставшихся объектов ведёт к полному replay;
конкурентные GC workers, S3 consistency и power-loss не проверены этим тестом.

`retire_old_active_snapshots` переводит в REJECTED не более 100 старых ACTIVE
записей за проход, только при двух более новых ACTIVE версиях. Возраст задаётся
`PXEL_SNAPSHOT_ACTIVE_RETENTION_DAYS` (default 30, диапазон 1..3650).
Существующий почасовой cleanup worker сначала выводит кандидатов, затем
повторно удаляет объекты REJECTED через BlobStore. Metadata и append-only
operation log сохраняются. Решение записано в ADR-038, README/persistence/
roadmap и отображено в промежуточной презентации.

Реально выполнено: адресный PostgreSQL/LocalFilesystemBlobStore test 1/1:
четыре committed версии и ACTIVE снимка, cutoff, bounded batch size 1,
сохранение v3/v4, preferred pointer, удаление объектов v1/v2, historical v1
через full replay и восстановление head через v3+tail после потери v4.
Server config test 1/1 и clippy storage/server прошли. `cargo sqlx prepare`
обновил checked query metadata; `just check` прошёл Rust fmt, workspace
Clippy/tests/build, WASM, protobuf drift, frontend format/lint/typecheck,
Vitest 71/71 и production Vite build. `just sqlx-check` прошёл.
Presentation check проверил 10 слайдов на 1440×900 и 390×844;
`git diff --check` прошёл. Новый latency benchmark не запускался; один
локальный SQLx test не доказывает поведение при десятках тысяч снимков.
Linux/Compose/remote CI, S3 contracts, power-loss, parallel GC и PITR/backup
остаются открытыми; фаза 5 не закрыта.

## 2026-09-29 — статус презентации во время разработки MVP

Пользователь уточнил, почему презентация появилась до завершения всех фаз.
Проверена текущая версия `presentation/index.html`: обложка уже помечает её
промежуточной, а roadmap явно оставляет gates фаз 3–5 открытыми и XLSX будущей
фазой. Укорочен перегруженный текст девятого слайда; вместо перечня деталей
оставлены статус и ссылки на этот RUNBOOK и roadmap. Презентация обновляется
после проверенных вертикальных срезов и не считается финальной демонстрацией
MVP. В этом изменении product-код, метрики и статус фаз не менялись.

## 2026-09-29 — persisted benchmark snapshot + 10 000 tail events

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-033/035/037,
domain-only benchmark, storage recovery/journal и testing docs. Цель —
измерить реальный PostgreSQL + BlobStore + codec путь до любой оптимизации.
Инварианты: snapshot v10001 берётся только после durable commit; ещё 10 000
событий доходят до v20001; каждый timed результат равен committed domain
hash и полному replay. Риски: fixture с одной ячейкой и тёплыми кешами не
моделирует большую книгу, холодный restart или нагрузку других клиентов.

Добавлен игнорируемый release SQLx harness `recovery_benchmark` и команда
`just bench-recovery-storage`. Он создает изолированную test database,
выполняет 20 000 обычных durable commits, публикует проверенный snapshot,
делает по два warm-up и 20 чередующихся samples для snapshot+tail и полного
authenticated journal replay. Сборка test harness без запуска и storage
clippy прошли. `just bench-recovery-storage` запущен дважды; оба прогона
прошли и подтвердили одинаковый hash. Run 1: 20k commits + snapshot 20.477 s,
snapshot p50/p95 49.795/87.611 ms, full replay 107.578/139.006 ms.
Run 2: та же подготовка 21.167 s, snapshot 50.739/71.356 ms, full replay
102.740/131.171 ms. PostgreSQL readiness проверил `fsync`,
`full_page_writes`, `synchronous_commit` и точную миграцию. Машина,
методика и ограничения записаны в
`docs/benchmarks/2026-09-29-snapshot-replay-persisted-local.md`.
`just check` и `just sqlx-check` прошли: Rust fmt/clippy/tests/build,
WASM, protobuf drift, frontend format/lint/typecheck, Vitest 71/71 и Vite
build; SQLx checked metadata не менялась. Презентация прошла 10 слайдов на
1440×900 и 390×844, `git diff --check` прошёл. Значение p95 — локальный
baseline одной ячейки, не SLO холодного восстановления большой книги.

## 2026-09-29 — уборка незавершённых snapshots

Перед срезом прочитаны AGENTS.md, RUNBOOK, roadmap, ADR-033/035,
snapshot publisher, BlobStore и миграция metadata. Цель — возвращать место
после прерванной публикации без удаления ACTIVE fallback и durable log.
Инварианты: только старые UPLOADING/STORED/VERIFIED переходят в REJECTED;
publisher проигрывает гонку по проверенному `rows_affected`; ACTIVE и
preferred pointer не меняются; поздний объект остаётся обнаружимым для
повторной уборки. Риски: гонка GC/publisher, ошибка удаления, starvation
ограниченной пачки и локальная durability файловой системы.

Миграция 015 добавляет `cleanup_attempted_at` и допускает контролируемое
обновление REJECTED metadata. `cleanup_incomplete_snapshots` использует
PostgreSQL `FOR UPDATE SKIP LOCKED`, bounded batch и идемпотентный BlobStore
delete. Сервер запускает worker раз в час; retention настраивается через
`PXEL_SNAPSHOT_CLEANUP_HOURS`, default 24. Publisher проверяет все три
перехода после object put и не активирует запись, уже отклонённую GC.
ADR-037 описывает предел: старые ACTIVE поколения, orphan objects без
metadata и S3 пока не удаляются.

Реально выполнено: `cargo sqlx migrate run` применил миграцию 015;
целевой SQLx test остановок и GC 1/1 прошёл на PostgreSQL и
LocalFilesystemBlobStore; `cargo clippy -p spreadsheet-storage -p pxel-server
--all-targets --all-features -- -D warnings` прошёл. `cargo sqlx prepare
--workspace -- --all-targets --all-features` записал checked query metadata.
`just check` прошёл: Rust fmt/clippy/tests/build, WASM, protobuf drift,
frontend format/lint/typecheck, Vitest 71/71 и Vite build. `just sqlx-check`
прошёл. Runtime smoke: сервер `cargo run --locked -p pxel-server --features
dev-auth` запустился с `snapshot_cleanup` (0 rejected, 0 delete attempts),
`/readyz` вернул 200, после чего сервер остановлен. Презентация прошла
10 слайдов на 1440×900 и 390×844; `git diff --check` прошёл.
Power-loss, S3 contract, full retained-ACTIVE GC и Linux/Compose/remote CI
этим срезом не проверялись.

## 2026-09-29 — чтение исторической версии документа

Перед срезом прочитаны AGENTS.md, roadmap, ADR-033 и текущие recovery,
journal, HTTP projection и тесты. Цель — дать авторизованный read-only доступ
к точной подтверждённой версии, используя snapshot не новее этой версии.
Инварианты: текущий head и actor не меняются; незавершённые и будущие версии
не выдаются; журнал и snapshot проверяются; текущий ACL обязателен.
Риски: дорогой replay и большой JSON projection, гонка отзыва ACL с HTTP
body send. Для восстановления введены 2 permits и timeout 30 секунд;
последняя гонка остаётся production-задачей и записана в ADR-036.

Добавлены `recover_document_at_version` и HTTP
`GET /api/documents/{id}/versions/{version}`. Общая функция JSON projection
используется текущим и историческим маршрутом. Отдельный SQLx-тест с
настоящей PostgreSQL и LocalFilesystemBlobStore проверяет snapshot eligibility,
точный replay, fallback без объектов, unauthorized/future/zero и head.
HTTP integration test проверяет v1/v2, ошибки и изоляцию workspace.

Первый целевой прогон выявил Windows test-thread stack overflow после
вложения нового async recovery future; внутренний recovery future вынесен
на heap через `Box::pin`, и три snapshot SQLx-теста прошли 3/3. Clippy
выявил пропущенный вариант `SnapshotError` в server mapping и лишний borrow;
оба исправлены. HTTP-тест сначала упал на dev-account длиной >32, затем на
ошибочном ожидании 403: существующий контракт скрывает чужой документ кодом 404. После исправления fixture и точного ожидания HTTP-тест прошёл 1/1;
полный `node --test tests/integration/document.test.mjs` прошёл 33/33
на локальном сервере. `just check` прошёл: Rust fmt/clippy/tests/build,
WASM target/build, protobuf drift, frontend format/lint/typecheck,
Vitest 71/71 и Vite build. `just sqlx-check` и
`node apps/web/scripts/check-presentation.mjs` прошли; презентация —
10 слайдов на 1440×900 и 390×844. `git diff --check` прошёл.
GC, S3 contract, исторические calculated results и UI истории не
реализованы. Linux/Compose/remote CI этим срезом не проверялись.

## 2026-09-29 — controlled interruption при публикации snapshot

Перед срезом прочитаны AGENTS.md, roadmap, ADR-033, persistence/testing docs
и текущий publisher/reader. Цель — проверить, что незавершённая публикация
не активирует snapshot и не скрывает durable журнал. Инварианты:
`ACTIVE` только после DB COMMIT, preferred pointer обновляется атомарно,
reader не использует UPLOADING/STORED/VERIFIED, восстановленный domain hash
совпадает с журналом. Риск: controlled early return не моделирует потерю
питания и гарантию flush конкретного диска.

В `publish_snapshot_inner` добавлены только test-only точки остановки после
metadata INSERT, blob PUT, STORED, VERIFIED, UPDATE внутри activation
transaction и activation COMMIT. Отдельный SQLx test использует настоящие
PostgreSQL и LocalFilesystemBlobStore. Он проверяет metadata state, наличие
объекта, preferred pointer и `recover_document` после каждой остановки.
Пять остановок до COMMIT дают полный journal replay; UPDATE ACTIVE внутри
незавершённой transaction откатывается. Остановка после COMMIT оставляет
читаемый ACTIVE snapshot и preferred pointer. Ранее проверенный тест двух
поколений и повреждения объекта также остался зелёным.

Реально выполнено: targeted crash test 1/1, прежний snapshot test 1/1,
`cargo clippy -p spreadsheet-storage --all-targets --all-features -- -D
warnings`, общий `just check` (Rust fmt/clippy/tests/build, WASM,
protobuf drift, frontend format/lint/typecheck, Vitest 71/71, Vite build),
`just sqlx-check` и `node apps/web/scripts/check-presentation.mjs`
(10 слайдов на 1440×900 и 390×844). Первый `just sqlx-check` выявил
отсутствующую metadata для нового test query; выполнен `cargo sqlx prepare
--workspace -- --all-targets --all-features`, затем check прошёл.
Первый targeted run прошёл, но clippy выявил одинаковый префикс вариантов
test enum; названия исправлены и targeted/clippy повторены. Power-loss,
GC, полный persisted benchmark и S3 остаются открытыми.

## 2026-09-29 — domain benchmark snapshot + 10 000 tail events

Перед срезом прочитаны AGENTS.md, roadmap, testing docs и действующие
snapshot/benchmark fixtures. Цель — создать реальный harness для 10 000
событий хвоста до любой оптимизации recovery. Инварианты: одинаковый
канонический domain hash после snapshot+tail и full replay, проверка
expected hash каждого события, подготовка fixture вне измеряемого цикла.
Риск — одна ячейка не моделирует большой разреженный документ и не включает
PostgreSQL/BlobStore/codec.

Добавлен Criterion bench `crates/spreadsheet-domain/benches/recovery.rs`:
10 000 изменений до snapshot v10001 и 10 000 после него, сравнение с
full replay 20 001 events. `cargo check -p spreadsheet-domain --features
test-support --benches` и bench clippy прошли. Реальный запуск:
`cargo bench --locked -p spreadsheet-domain --features test-support --bench
recovery -- --sample-size 10 --warm-up-time 1 --measurement-time 3` прошёл.
Criterion 95% interval: snapshot + tail 5.7527–8.6369 ms; full replay
17.024–20.579 ms. Машина, профиль, dataset, ограничения и отсутствие
PostgreSQL/BlobStore перечислены в
`docs/benchmarks/2026-09-29-snapshot-replay-domain-local.md`.
Финальные `just check` и presentation browser check прошли: Rust
fmt/clippy/tests/build, WASM, protobuf drift, ESLint, TypeScript, Vitest
71/71, Vite build; презентация — 10 слайдов на 1440×900 и 390×844.
`just sqlx-check` после этого domain-only среза не запускался: SQL queries
не менялись. Первый общий check остановился на Prettier для benchmark
report; формат исправлен, повторный check прошёл. Следующий gate — полный
persisted recovery benchmark и crash matrix.

## 2026-09-29 — bounded snapshot scheduler

Перед срезом прочитаны AGENTS.md, roadmap, ADR-033/034 и actor/storage/server
code. Цель — создавать новые snapshots автоматически из подтверждённой
projection без ожидания записи объекта в ACK path. Инварианты: enqueue
только после durable commit и prepared swap; snapshot ускоряет recovery,
журнал остаётся authority; неограниченных очередей и содержимого документа
в логах нет. Риски: пропущенный milestone при переполнении очереди,
неотменяемое blocking encoding и 32 MiB cap.

Добавлен default threshold 1000 committed событий после genesis,
`PXEL_SNAPSHOT_INTERVAL` ограничен диапазоном 1..100000, bounded channel
capacity 1 и один server worker. Состояние передаётся `Arc`, codec работает
в `spawn_blocking`, затем прежний verified publication lifecycle. Retry
operation_id не ставит duplicate job. Тест с threshold 1 проверил enqueue
только после committed v3 и отсутствие enqueue при повторе той же команды.
Документировано в ADR-035, persistence, roadmap и презентации.

Точечный PostgreSQL/actor тест `active_snapshot_round_trip_and_corrupt_newest_falls_back`
прошёл 1/1; `cargo check -p pxel-server --features dev-auth` и точечный
clippy прошли. Локальный server smoke с threshold 1: после одной
WebSocket операции PostgreSQL показал `document_snapshots.state=ACTIVE`,
`snapshot_version=2`, `documents.head_version=2`; после hard restart HTTP
прочитал v2 с сохранённым содержимым. Содержимое документа не включено в
RUNBOOK или production tracing.
Финальный `just check` прошёл (Rust fmt/clippy/tests/build, WASM,
protobuf drift, ESLint, TypeScript, Vitest 71/71, Vite build), в том числе
unit-проверка диапазона `PXEL_SNAPSHOT_INTERVAL`. `just sqlx-check` прошёл;
browser presentation check: 10 слайдов на 1440×900 и 390×844.
`pnpm test:recovery` прошёл 1/1 с hard restart; `pnpm test:integration`
прошёл 40/40 на штатном interval 1000. E2E после этого server-only среза
не повторялся; предыдущий полный browser gate 48/48 записан ниже.

Первый общий check остановился на Prettier для README, формат исправлен.
Первый запуск `pnpm test:recovery` не дошёл до теста: Windows linker не мог
перезаписать исполняемый файл работающего dev-сервера. Dev-сервер остановлен,
тот же recovery-test прошёл, затем `just dev` поднят снова.
Crash matrix, GC, S3 contract, 10k-tail benchmark и XLSX остаются открытыми.

## 2026-09-29 — actor startup из проверенного snapshot

Перед срезом прочитаны AGENTS.md, roadmap, ADR-004/033 и пути collab,
storage, server. Цель — использовать проверенную snapshot projection в
рабочем document actor. Инварианты: ACL до и после открытия, только один
actor на документ, committed state отделён от calculation cache, журнал
остаётся authority. Риски: гонка параллельных открытий и отсутствие
calculated cache у restored image.

`InProcessSessionDirectory::with_recovery` вызывает recovery вне общего
mutex, затем повторно проверяет каталог перед spawn. Admission ограничен
четырьмя recovery одновременно и timeout 10 секунд. Server подключает
`PgDatabase::recover_document` с LocalFilesystemBlobStore. Actor получает
готовую книгу, запускает полный расчёт, затем `refresh` догоняет события,
появившиеся после recovery. Базовый каталог без callback по-прежнему
поддерживает полный replay. Решение — ADR-034. PostgreSQL/BlobStore тест
проверил, что actor открывает v2 snapshot без journal tail и получает
calculated state v2; тот же сценарий проверяет fallback к v1 + tail и full
replay после повреждения/удаления объектов.

Точечный тест `cargo test -p spreadsheet-storage
active_snapshot_round_trip_and_corrupt_newest_falls_back -- --nocapture`
прошёл 1/1; `cargo check -p pxel-server --features dev-auth` и точечный
clippy для collab/storage/server прошли. Финальный `just check` прошёл:
Rust tests/build, WASM, protobuf drift, format/lint/TypeScript, Vitest
71/71 и Vite build. `just sqlx-check` прошёл. `pnpm test:recovery`
прошёл 1/1 с hard server restart, `pnpm test:integration` — 40/40,
`pnpm test:e2e` — 48/48 в Chromium DPR 1/2. Презентация проверена на
1440×900 и 390×844: 10 слайдов, навигация и layout без ошибок.

Первый общий check остановился на rustfmt после дополнительного timeout;
формат исправлен, повторный check прошёл. Первый integration запуск был без
локального сервера (`ECONNREFUSED`); после запуска `just dev` первый полный
прогон дал 39/40 из-за timeout calculation stream. Отдельный повтор этого
теста прошёл, следующий полный прогон дал 40/40. Причина разового timeout
пока не воспроизведена; это открытое наблюдение, а не заявленная
детерминированная исправленная ошибка. Scheduler, crash
matrix, S3 contract, 10k-tail benchmark и XLSX остаются открытыми.

## 2026-09-29 — authenticated snapshot tail recovery API

Цель следующего среза фазы 5 — доказать восстановление из снимка и хвоста
реального PostgreSQL log до подключения actor startup. Инварианты: ACL на
каждом чтении, только `ACTIVE` snapshot, непрерывный журнал, проверка
конечной версии и hash; повреждённый снимок не скрывает событие. Риск:
конкурентный новый head при восстановлении — reader фиксирует target head в
начале и ограничивает каждый page этим target.

`PgDatabase::recover_document` возвращает книгу и provenance: версию
использованного снимка и число применённых событий. В том же
PostgreSQL/BlobStore тесте проверены 0 событий после актуального v2,
1 событие после повреждения v2 и выбора snapshot v1, затем 2 события после
удаления v1 и полного journal replay. Большие async futures теста вынесены
на heap, чтобы штатный Windows test thread не переполнял стек.
`cargo test -p spreadsheet-storage
active_snapshot_round_trip_and_corrupt_newest_falls_back -- --nocapture`
прошёл 1/1. Actor startup wiring остаётся следующим шагом.

## 2026-09-29 — локальная activation snapshots (фаза 5, внутренний срез)

Перед срезом прочитаны AGENTS.md, roadmap, ADR-004/031/032 и domain,
codec, storage, server runtime. Цель — сделать бинарный образ пригодным для
локального восстановления только после повторной проверки объекта.
Инварианты: PostgreSQL остается authority, snapshot не заменяет log,
`ACTIVE` означает проверенный объект, старое поколение сохраняется,
кодек и metadata versioned. Риск — разрыв между snapshot publication и
startup actor; он оставлен явно открытым.

Migration 014 добавила immutable metadata и state transition trigger;
`SnapshotId` введён как доменный ID. `PgDatabase::publish_snapshot`
сверяет источник с проверенным journal event, пишет объект в
LocalFilesystemBlobStore, перечитывает bytes/checksum/decoded hash и
активирует metadata короткой transaction. `latest_valid_snapshot` читает
только `ACTIVE` и выбирает более старый при повреждении нового. Reader
проверяет максимум 128 поколений за вызов. Решение — ADR-033; обновлены
persistence, roadmap и живая презентация.

Реально выполнено: `cargo run --locked -p pxel-server --features dev-auth --
migrate` (migration 014); точечный PostgreSQL/BlobStore тест 1/1 с
round-trip двух поколений, повреждением более нового и fallback до полного
replay; `cargo check -p spreadsheet-storage`; `cargo clippy -p
spreadsheet-storage --all-targets -- -D warnings`; `cargo sqlx prepare
--workspace -- --all-targets --all-features`; финальные `just check` и
`just sqlx-check` прошли. Browser presentation check проверил 10 слайдов
на 1440×900 и 390×844. `just check` включает Rust tests, format/clippy,
WASM build, protobuf drift, ESLint, TypeScript, Vitest 71/71 и Vite build.

Первый общий check остановился на Prettier для изменённого слайда; формат
исправлен. После добавления дополнительного теста Windows test thread
переполнил штатный стек из-за размера составного async future; тестовый
сценарий вынесен в heap-allocated future, затем тот же тест прошёл на
штатном стеке. После добавления checked SQL query `.sqlx` metadata
перегенерирована; повторный `just sqlx-check` прошёл.
Автоматический startup через snapshot, scheduler, tail replay integration,
crash injection, GC, S3 contract, XLSX и фаза 6 остаются открытыми.

## 2026-09-29 — schema-9 DeleteColumns with durable tombstones

Перед срезом прочитаны AGENTS.md, roadmap, ADR-023 и текущие domain,
formula, codec, actor, server и browser paths. Цель: удалять стабильные
столбцы без перенаправления ссылок на соседнюю A1-координату. Инварианты:
candidate до CAS, typed `#REF!`, один durable event/version, idempotent
receipt, hash-equal replay и запрет повторного выделения удалённого ID.
Риски: полный проход по формулам и рост tombstone set; будущий snapshot
codec обязан их сохранить.

`DeleteColumns` принимает 1–100 уникальных живых IDs и оставляет хотя бы
один столбец. Schema-9 event, SQL migration 009, отдельные wire/storage
protobuf fields, semantic hash tag и command fingerprint добавлены без
переписывания schema 1–8. Candidate удаляет sparse cells, обновляет
text/AST budgets, переводит scalar bound refs к удалённой колонке в typed
`Invalid(DeletedTarget)`, сужает partial ranges и пересчитывает relative
column offsets; удалённые IDs остаются в capped tombstone set и входят в
domain hash. Calculation graph перестраивается. Canvas UI отправляет
удаление выбранного столбца и ждёт каноническую projection. Изменены domain,
formula, collab, codec/journal, server, web store/UI, protobuf/generated TS,
migration и unit/integration/recovery/browser tests. Решение — ADR-024;
обновлены collaboration/formula semantics, roadmap, benchmark и слайды.

Реально выполнено: `pnpm proto:generate`; `cargo run --locked -p pxel-server
--features dev-auth -- migrate` (migration 009); точечные domain, binder,
codec, PostgreSQL/WebSocket и browser DPR 1/2 tests; `pnpm test:recovery`
1/1 с hard restart; `just check` pass (Rust fmt/clippy/tests/build/WASM,
protobuf drift, Prettier/ESLint/TypeScript, Vitest 60/60, Vite build);
`just sqlx-check` pass; `pnpm test:integration` 35/35; окончательный
`pnpm test:e2e` 32/32. Презентация открыта Chromium на ширинах 1280/390 px:
10 слайдов, горизонтального переполнения нет. Criterion
`delete_target_column_10000_formulas`: 12.241–13.377 ms, point estimate
12.712 ms, 10 samples/220 iterations; условия —
docs/benchmarks/2026-09-29-column-deletion-local.md. Это domain candidate,
не p95 end-to-end commit.

Первый browser suite 31/32 зафиксировал гонку старого offline-теста: WS
автоматически переподключился до проверки кратковременного статуса. Тест
теперь устанавливает offline сразу после закрытия socket и проверяет
восстановление canonical данных после возврата сети; полный 32/32 прошёл.
Первый integration suite при параллельных файлах получил HTTP 429 на общем
dev rate limit; `--test-concurrency=1` в package script даёт стабильный
функциональный прогон 35/35. Нагрузочные сценарии остаются отдельно.
Copy/paste, undo, multi-sheet, snapshots, XLSX и Linux/Compose/remote CI не
закрыты.

## 2026-09-29 — schema-8 InsertColumns with stable formula targets

Перед срезом прочитаны AGENTS.md, roadmap, ADR-021/022 и текущие domain,
formula, codec, actor, server и browser paths. Цель: вставлять столбец перед
stable anchor без перепривязки ячеек или формульных целей. Инварианты:
server order, candidate до CAS, separate wire/storage schema, hash-equal replay,
авторизация каждой команды. Риск: преобразование сканирует все формулы; stale
formula после изменения оси по-прежнему требует resync.

Добавлены `InsertColumns` client intent и `InsertColumnsRebased` schema-8
durable event, SQL migration 008, отдельные protobuf fields, semantic hash
tag и command fingerprint. Кандидат проверяет 1–100 новых уникальных IDs,
живой anchor и общий предел 1,000 столбцов; после вставки пересчитывает
relative column offsets, сохраняет stable targets и исходный текст, проверяет
выводимый current A1 text до CAS. Calculation graph перестраивается.
UI-кнопка вставляет столбец перед выделенным, WebSocket pending queue ждёт
canonical projection. Изменены domain/formula/collab, codec/journal/server,
web store/UI, protobuf и generated TypeScript, migration и тесты. Решение —
ADR-023, benchmark report, collaboration/formula semantics, roadmap и слайды.

Реально выполнено: `pnpm proto:generate`; `cargo run --locked -p pxel-server
--features dev-auth -- migrate` (migration 008); точечные domain, binder,
codec, PostgreSQL/WebSocket и browser DPR 1/2 tests; `pnpm test:recovery`
1/1 с hard restart и проверкой schema-8 hash; `just check` pass (Rust
fmt/clippy/tests/build/WASM, protobuf drift, Prettier/ESLint/TypeScript,
Vitest 59/59, Vite build); `just sqlx-check` pass; окончательный
`pnpm test:integration` 34/34 и `pnpm test:e2e` 30/30 на реальном dev-сервере.
Презентация открыта Chromium при ширине 1280/390 px: 10 слайдов,
горизонтального переполнения нет.
Criterion `insert_front_column_10000_formulas` дал 11.853–14.140 ms,
point estimate 12.883 ms, 10 samples/220 iterations; условия —
docs/benchmarks/2026-09-29-column-insertion-local.md. Это стоимость
optimized domain candidate, не p95 commit.

Параллельный browser прогон с 2 workers дал 2 сбоя создания документа на
общем dev rate limit 64/s; последовательный 30/30 прошёл. Playwright
functional config переведён на 1 worker, concurrency/slow-reader проверяются
отдельно. Первый полный integration прогон дал transient failure endpoint
расчёта; повторный и окончательный 34/34 прошли. Тестовый helper теперь
показывает HTTP status при такой ошибке без содержимого документа.
Нереализованы удаление столбцов, copy/paste, undo, multi-sheet и snapshots.
Linux/Compose/remote CI не запускались.

## 2026-09-29 — schema-7 DeleteRows with durable tombstones

Перед срезом проверены AGENTS.md, roadmap, ADR-021 и текущие domain,
formula, codec, actor, server и browser paths. Цель: удалять живую строку по
stable ID без перенаправления формулы на соседнюю A1-координату. Инварианты:
candidate до CAS, typed `#REF!`, монотонная версия, идемпотентный receipt,
невозможность повторно выдать удалённый row ID, hash-equal replay. Риски:
полный проход по формулам, рост tombstone set и отсутствие snapshot codec.

Реализованы ограниченная команда `DeleteRows` (1–1,000 уникальных живых IDs,
хотя бы одна строка остаётся), schema-7 persisted event и SQL migration 007.
Удаление убирает sparse cells и их бюджеты, превращает scalar bound target в
`Invalid(DeletedTarget)`, сужает частично живые ranges и пересчитывает
relative offsets. Удалённые IDs остаются в capped tombstone set и участвуют
в domain hash; новые insert/extend с тем же ID rejected. Calculation graph
перестраивается. Canvas UI отправляет команду для выбранной строки, обновляет
projection и отменяет активный draft при структурном изменении. Решение
зафиксировано в ADR-022; обновлены formula/collaboration semantics, roadmap
и живая презентация. Изменены domain/formula/collab, wire/storage protobuf,
storage codec/journal, server transport, web store/UI, migration, benchmark
и domain/formula/codec/integration/recovery/browser tests.

Проверено на локальном PostgreSQL 18 и настоящем WebSocket: migration 007
применена; `pnpm proto:generate`; `pnpm test:integration` 32/32 после
отдельного запуска; `pnpm test:recovery` 1/1 с hard restart; полный Playwright
`pnpm --filter @pxel/web exec playwright test --workers=2` 28/28 при DPR 1/2;
`just check` pass (Rust fmt/clippy/tests/build/WASM, protobuf drift,
Prettier/ESLint/TypeScript, Vitest 58/58, Vite build); `just sqlx-check` pass.
Презентация открыта в Chromium при ширине 1280 и 390 px: 10 слайдов,
горизонтального переполнения нет. `pnpm format:check` и `git diff --check`
после документационных правок прошли.
Criterion `delete_target_10000_formulas` на optimized domain candidate дал
14.974–17.812 ms, point estimate 15.916 ms, 10 samples/165 iterations;
условия — docs/benchmarks/2026-09-29-row-deletion-local.md. Первый вызов
benchmark без `--features test-support` закономерно отказал; с feature прошёл.

Наборы integration и browser одновременно запускались на одном dev-сервере и
вызвали глобальный rate limit 64/s (`429`); последовательный запуск прошёл.
Browser offline-сценарий обнаружил, что Chromium/Vite может перейти на
`chrome-error://`; тест теперь явно выполняет новую навигацию после возврата
сети и доказывает durable convergence, а не исключительно WebSocket resume.
Структурные browser tests ждут загрузки projection после reload перед
клавиатурным вводом. Текущий срез не закрывает columns, undo, copy/cut/sort,
historical-axis formula binding, multi-sheet или snapshots; будущий snapshot
обязан сохранять tombstones. Linux/Compose/remote CI не запускались.

## 2026-09-29 — schema-6 row insertion with formula rebase

Перед срезом прочитаны AGENTS, roadmap, ADR-002/020, formula semantics,
collaboration contract и текущий путь domain → codec → journal → actor → HTTP/UI.
Цель: разрешить вставку строки в книге с формулами без потери stable targets,
copy offsets или восстановления после сбоя. Инварианты: candidate до CAS,
неизменность старых schema-5 событий, server-authoritative адреса и hash,
отказ до COMMIT при превышении формульных лимитов. Риск: все формулы пока
перебираются при структурной операции; многолистовые fixtures отсутствуют.

Новые команды `InsertRows` нормализуются в отдельную durable операцию
`InsertRowsRebased` schema 6. Domain сначала вставляет row ID, затем для каждой
формулы пересчитывает relative row delta по stable target и новому origin,
проверяет диапазоны и выводимый A1-текст. Абсолютные row modes и целевые IDs
не меняются. Старые schema-5 события сохраняют прежний formula-free guard;
их protobuf bytes и semantic hash не переписываются. SQL migration 006
разрешает schema 6 для user events. Calculation graph после commit строится
заново. HTTP projection теперь содержит `current_text`, полученный из bound
AST и актуальных осей; UI использует его для редактора и WASM preview только
если он отличается от сохранённого canonical text, иначе сохраняет исходный
авторский ввод. Решение и миграция описаны в ADR-021; обновлены ADR-020,
collaboration/roadmap и слайды 6, 9, 10.

Реально выполнено: `pnpm proto:generate`; `cargo run --locked -p pxel-server
--features dev-auth -- migrate` (migration 006 применена к локальной БД);
domain test с replay hash/relative delta; codec test schema 5/6 round-trip;
`pnpm test:integration` 31/31 на настоящем PostgreSQL/WebSocket;
`pnpm test:e2e` 24/24 на built Axum assets при DPR 1/2;
`pnpm test:recovery` 1/1 с hard restart после formula + insertion;
`just check` pass после форматирования; `just sqlx-check` pass.
Дополнительный browser test изменённого A1 в formula bar после вставки и
reload прошёл 2/2 при DPR 1/2; полный browser suite после его добавления не
повторялся. Pure domain test проверил lookup cross-sheet target ID; реальной
многолистовой книги в MVP пока нет. Последний `just check` включал 57/57
Vitest tests; `pnpm format:check`, `git diff --check` и presentation check
после документационных правок прошли.
После коммита усилен этот browser test: значение 7 осталось результатом
`=A2` после вставки и reload (2/2 DPR). Integration scenario теперь проверяет
не только A1-текст `=A5`, но и canonical calculation result 100 на версии 7;
целевой прогон прошёл 1/1 на живом сервере. Остальной full suite после этого
усиления не повторялся.
Первый `just check` остановился только на Prettier для двух изменённых JS/TS
файлов; после `prettier --write` прошёл. Первый полный browser suite выявил,
что редактор показывал canonical text вместо исходной формулы после reload;
выбор `current_text` ограничен случаями изменённого A1-представления, повторный
suite прошёл. Тесты не удалялись, assertion не ослаблялись.

Criterion harness `insert_middle_10000_formulas`: 3 s warm-up, 10 s target,
10 samples/825 timed iterations; 95% estimate 12.078–13.443 ms, point
12.681 ms для domain candidate на Windows/Ultra 9 185H. Без БД, сети и UI;
[полные условия](docs/benchmarks/2026-09-29-formula-insertion-local.md).
`pnpm --filter @pxel/web presentation:check` прошёл 10/10 слайдов на
1440×900 и 390×844 после изменения текста.
Ограничения: multi-sheet structural fixture, удаление/колонки, copy/move,
undo, snapshots и XLSX не реализованы; phase-4 gate остаётся открытым.
Следующий минимальный срез — typed DeleteRows с `#REF!` для удалённых targets
и тестами convergence/recovery, затем copy/paste/undo.
Перед коммитом просмотрены тесты и diff по пяти осям: корректность (versioned
replay и preserved stable target), читаемость, crate boundaries, авторизация/
данные логов и стоимость scan. Блокирующих замечаний после исправления
регрессии authored source не осталось; известные границы указаны выше.

---

## 2026-09-29 — read-only A1 rendering from stable formula references

Для следующего среза фазы 4 прочитаны AGENTS, roadmap, ADR-020 и код
domain/formula binder, formatter и projection. `spreadsheet-formula::bound` теперь
умеет получать текущий A1-текст из bound AST и актуальных осей, не разбирая
старый пользовательский source. Тест строит новую проекцию осей со строкой
внутри диапазона: видимый
`A1:A2` становится `A1:A3`, повторный bind сохраняет stable targets, а
относительный row delta меняется с -2 на -3. Это только чистый read-only
компонент: `InsertRows` при формулах по-прежнему отклоняется; серверный DTO и
UI ещё не используют новый текст. Прежде чем снять защиту, нужен versioned
persisted transform deltas с replay и browser tests.

Выполнены `cargo fmt --all`, `cargo test --locked -p spreadsheet-formula
--test binder` (3/3), `cargo clippy --locked -p spreadsheet-formula
--all-targets -- -D warnings` (pass). Полный `just check` для этого среза
не запускался; другие тесты не заявляются.

---

## 2026-09-29 — статус промежуточной презентации

По замечанию пользователя обложка и последний слайд `presentation/index.html`
теперь прямо называют презентацию промежуточным отчётом до завершения MVP.
Слайды должны обновляться только после проверенных срезов; фазы 3–4 открыты,
фазы 5–6 не реализованы. Изменение только текстовое, продуктовый код не затронут.
`pnpm --filter @pxel/web presentation:check` прошёл 10/10 слайдов на 1440×900
и 390×844 после загрузки локального toolchain; `git diff --check` прошёл.

---

## 2026-09-29 — baseline вставки перед 99,999 строками

В `crates/spreadsheet-domain/benches/workbook.rs` добавлен timed case
`insert_front_99999_rows`. Формулы отсутствуют, candidate создаётся из
предварительно подготовленной книги. Запущен optimized Criterion с 3 s warm-up,
10 samples и 935 timed iterations. Интервал 95% estimate — 6.6972–7.2624 ms,
point estimate 6.9241 ms на текущем Windows/Ultra 9 185H. Это не p95 commit
или UI, не измерение памяти. Полные условия записаны в
[отчёте](docs/benchmarks/2026-09-29-row-insertion-local.md). Production-код
после предыдущего `just check` не менялся; benchmark и property test прошли
отдельно.
После добавления harness `cargo clippy --locked -p spreadsheet-domain
--all-targets --all-features -- -D warnings`, `pnpm format:check` и
`git diff --check` завершились успешно.

---

## 2026-09-29 — вставка строки перед стабильным ID

Перед этапом прочитаны AGENTS, roadmap, ADR-002/019, collaboration contract,
domain/formula/codec/actor/frontend и RUNBOOK. Цель — вставить строку внутри
листа с сохранением identity заполненных ячеек. Инварианты: server order,
candidate перед CAS, typed отказ вместо молчаливой порчи формул, versioned
persisted event, idempotent retry, confirmed axes отдельно от pending. Риск:
формулы хранят authored source и relative deltas; без трансформации текущий A1
текст после вставки может быть неверен. ADR-020 вводит временный workbook-wide
запрет вставки при любой формуле до проверенного transformation slice.

Добавлены `InsertRows` с stable `before` anchor и batch 1–1,000 UUIDv7 IDs,
persisted schema 5, additive wire v1 field, миграция principal constraint,
типизированный rejection 12. Domain обновляет axis order/index, хранит клетки
на прежних ID, event codec считает отдельный semantic hash. Calculation worker
перестраивает projection после structural change. В web появились команда и
кнопка «Строка ↑», canonical refresh после ACK/hint и фактический размер осей
в footer. Изменены domain, formula, collab, storage-codec, storage, server,
wire/storage protobuf, generated TS, web store/UI и tests; документация и
презентация обновлены.

Первый integration test неверно ожидал обратный порядок двух insert-before:
фактический server order — ранняя вставка перед поздней, обе перед anchor.
Ожидание исправлено; production-код не менялся. PostgreSQL migration выполнена
локально (`migrations_complete`), `/readyz` вернул 200. Targeted domain 2/2,
codec 1/1, frontend Vitest 56/56, integration 31/31, Chromium E2E 24/24
при DPR 1/2 прошли. `pnpm test:recovery` прошёл 1/1 с hard restart после
`InsertRows`. Полный `just check` завершился exit 0: Rust fmt/Clippy/tests/build,
wasm32, protobuf drift, WASM build, Prettier, ESLint, TypeScript, 56 Vitest и
Vite build. Проверка презентации прошла 10 слайдов на 1440×900 и 390×844;
`git diff --check` чист. Linux/Compose/remote CI, reference
transforms, delete/columns, copy/paste и undo не проверены и не объявлены
готовыми.
`just sqlx-check` прошёл с текущей PostgreSQL после миграции; query metadata
не менялись.
После полного quality gate добавлен и отдельно выполнен property-based test
вставки перед случайным anchor (1–63 исходные строки): позиции всех stable IDs
и значение существующей клетки сохранились. Повтор полного `just check` после
этого дополнительного теста не выполнялся; production-код не менялся.

---

## 2026-09-29 — аудит десяти знаков PXEL

Проверены `design/logos/reference.svg`, десять файлов 01–10 и автономная
галерея. Все 11 SVG разобраны XML-парсером; у новых вариантов viewBox 64×64,
у исходного знака 37×37. В HTML проверена 61 ссылка на SVG, отсутствующих
файлов нет. Осмотрены desktop-галерея и снимок редактора; в верхней панели
редактора остаётся название таблицы без знака и wordmark. Исправлено только
неточное описание viewBox в `design/logos/README.md`. Runtime-код не менялся;
новый браузерный E2E-прогон в рамках этого аудита не запускался.

---

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

## 2026-09-29 — фаза 4, атомарная вставка обычных значений

Добавлен `SetCellValues`: 1–1,000 различных stable addresses сохраняются одной schema-10 durable operation и одной версией через candidate/CAS flow. Весь batch проверяется до COMMIT; неверный адрес, дубликат и превышение 512 KiB суммарного текста отклоняются целиком. Wire и storage protobuf версии разделены. Canvas принимает plain TSV, сразу показывает pending значения и после подтверждения загружает canonical projection. Формулы в массовом TSV отклоняются видимым сообщением; relative formula copy, rich clipboard, cut/move и undo не реализованы. Решение: [ADR-025](docs/adr/025-atomic-value-paste.md).

Изменены domain, formula calculation, collab, wire/storage codecs, серверный декодер, миграция 010, web store/grid, generated protocol, integration/recovery/browser tests, benchmark, roadmap, презентация и RUNBOOK. Выполнены `cargo fmt --all`, `cargo check --workspace`, `pnpm proto:generate`, `cargo run --locked -p pxel-server --features dev-auth -- migrate`. Domain и codec unit tests прошли. `pnpm test:integration`: 37/37 после добавления теста пересчёта зависимой формулы. `pnpm --filter @pxel/web test`: 61/61. `pnpm --filter @pxel/web test:e2e`: 34/34 при DPR 1/2. `pnpm test:recovery`: 1/1 после добавления batch restart/retry. `pnpm --filter @pxel/web presentation:check`: 10 слайдов на 1440×900 и 390×844. Первый `just check` дошёл до Prettier и остановился на новых неформатированных файлах; они исправлены `pnpm exec prettier --write ...`, повторный gate указан ниже.

Criterion: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook candidate_1000_value_batch -- --sample-size 10 --measurement-time 1` дал 567.59–631.31 µs (95% estimate), point 604.84 µs, 2,200 timed iterations; только доменный candidate без БД, сети и UI. [Условия](docs/benchmarks/2026-09-29-value-batch-local.md). После Prettier повторный `just check` завершился успешно: fmt, clippy, Rust workspace tests/build, wasm target/build, protobuf drift check, Prettier, ESLint, TypeScript, Vitest 61/61 и production web build. Phase 3 network gate и Phase 4 formula copy/undo остаются открытыми, затем Phase 5 snapshots и Phase 6 XLSX.

## 2026-09-29 — фаза 4, копирование одной формулы

Добавлен pure Rust `copy_formula_text` для A1-ссылок, `$`-режимов, диапазонов, имён листов и `#REF!` при выходе за адресное пространство. Тонкий WASM export работает в отдельном Worker. Внутренний clipboard переносит текущий видимый текст формулы и версию документа; paste отказывает при устаревшей версии, преобразует текст в Worker и отправляет обычный `SetCellFormula`. Сервер заново parse/bind-ит текст, авторизует и сохраняет его по прежнему CAS flow; clipboard AST не принимается. Пользовательский срез — одна формула в пределах одного документа, не `CopyRange` для прямоугольника. [ADR-026](docs/adr/026-single-formula-copy.md).

Затронуты `spreadsheet-formula`, thin WASM adapter/generated bindings, отдельный browser Worker, document store и Canvas clipboard, native/Vitest/browser tests, roadmap, benchmark script, презентация и RUNBOOK. Native copy tests 2/2 проверяют mixed `$`, ranges, quoted sheets, negative/out-of-bounds offsets. Browser targeted DPR 1/2 подтвердил Worker, server-bound commit, расчет и reload. Первый полный browser suite обнаружил тестовую гонку после reload: навигация шла до загрузки projection; тест стал ждать document value. Второй полный suite выявил настоящую гонку: WebSocket canonical formula event мог прийти до HTTP refresh без `current_text`, из-за чего clipboard был пуст. Исправлено заполнением current text из canonical formula при apply. Финальный browser suite с настоящими Ctrl+C/Ctrl+V прошёл 36/36 при DPR 1/2. `pnpm --filter @pxel/web presentation:check`: 10 слайдов, 1440×900 и 390×844. `just check` сначала остановился на форматировании ранее изменённой `docs/collaboration.md`; после Prettier повторный `just check` завершился успешно: Rust fmt/clippy/tests/build, WASM target/build, protocol check, Prettier, ESLint, TypeScript, Vitest 62/62 и production web build. PostgreSQL integration/recovery для этого среза повторно не запускались: server command/journal не менялись, browser E2E проверил их живой путь.

Локальный Worker benchmark `node apps/web/scripts/bench-formula-copy.mjs`: Chromium 153.0.8010.12, Windows 10.0.26100, Intel Core Ultra 9 185H, 15.6 GiB RAM, 5 warm-ups + 50 samples, fresh Worker + cached WASM init + copy + JS reply: p50 12.50 ms, p95 16.10 ms, max 16.90 ms. Это не commit/UI editing latency; [условия](docs/benchmarks/2026-09-29-formula-copy-worker-local.md). Открыто: rich multi-cell clipboard, cut/move, undo, Phase 3 network gate, snapshots, XLSX.

## 2026-09-29 — фаза 4, прямоугольное выделение и копирование значений

В Canvas добавлены anchor/focus выделения, Shift+стрелки и pointer drag с коалесцированием pointer move через animation frame. Выделение и активная ячейка при вставке/удалении осей следуют за стабильными ID; при удалении endpoint выбирается ближайшая допустимая позиция. Presence передает разные anchor/focus. Доступный диапазон произносится через ограниченный DOM output без создания DOM-ячеек. Ctrl+C переносит до 1000 обычных значений как TSV, внутренний MIME сохраняет завершающую пустую строку; Ctrl+V вставляет от левого верхнего угла выделения через существующий атомарный `SetCellValues` (одна версия документа). Формулы в многоячеечном диапазоне и текст, который текущий TSV не может перенести без потерь, получают видимую ошибку. Это локальный снимок скопированных значений, не server-side `CopyRange`; cut/move/undo и богатый формат остаются открытыми. [ADR-027](docs/adr/027-rectangular-selection-and-value-clipboard.md).

Изменены `apps/web/src/document/store.ts`, `apps/web/src/grid/CanvasGrid.tsx`, `apps/web/src/grid/renderer.ts`, Vitest и Playwright tests, `docs/roadmap.md`, презентация и этот RUNBOOK. Первая полная браузерная проверка дала 36/40: старые тесты дополнительно перемещали курсор после структурной вставки, хотя новая селекция уже следовала за stable ID. Проверки исправлены с явным утверждением нового положения выделенной ячейки и видимого адреса формулы; адресный повтор прошел 4/4. Финальный полный browser suite прошел 40/40 при DPR 1/2, включая настоящий clipboard, один durable batch, reload и отсутствие DOM-ячеек. Первый `just check` остановился на форматировании презентации; после Prettier повторный gate завершился успешно: Rust fmt/clippy/tests/build, WASM target/build, protobuf drift check, Prettier, ESLint, TypeScript, Vitest 65/65 и production web build. `pnpm --filter @pxel/web presentation:check` подтвердил 10 слайдов на 1440×900 и 390×844. PostgreSQL integration/recovery повторно не запускались в этом срезе: Rust server/domain и persisted protocol не менялись, а браузер проверил живой durable path. Новых latency-измерений нет; ранее измеренный доменный batch benchmark не является временем UI или commit.

## 2026-09-29 — фаза 4, защищённый undo одной записи ячейки

Реализован узкий [ADR-028](docs/adr/028-guarded-single-cell-undo.md): новая одиночная `SetCellValue`/`ClearCell` сохраняет server-derived preimage в immutable schema-11 event; `UndoCell` является клиентским намерением с новым operation ID. Server actor до COMMIT готовит schema-12 компенсацию с прежним содержимым, target ID и target version. PostgreSQL внутри той же транзакции повторно проверяет ACL, владение оригиналом, окно 10 000 версий и любой последующий write на тот же stable address (включая одинаковое значение, batch, formula, compensation и удаление оси), затем выполняет CAS. Повтор того же undo ID возвращает receipt; тот же ID с другой целью получает IdempotencyConflict. Domain hash зависит только от итогового состояния; preimage и ссылка на отменяемое событие остаются в журнале, не в кэше расчёта. Старые schema 1–10 сохранены без переписывания и не обещают retroactive undo.

В UI кнопка и Ctrl+Z отменяют только последнюю подтвержденную собственную одиночную запись обычного значения в текущей сессии. Intent остаётся pending без optimistic inverse; подтверждённая компенсация применяется в server order, typed conflict виден пользователю. После reload кнопка недоступна, хотя серверный повтор уже отправленного ID работает. Формульный preimage записывается, но его восстановление, batch/структурный undo, redo и общий per-user стек остаются вне этого среза. Схема wire и долговременный codec развиваются раздельно: клиент посылает только UndoCell intent, а серверная компенсация транслируется прежним wire SetCellValue/ClearCell для совместимости v1.

Изменены domain/collab/application/storage/storage-codec/formula crates, server wire adapter, `proto/pxel/{wire,storage}`, generated TS/descriptor, PostgreSQL migration 011, Web store/UI/tests, recovery helper/test, `docs/roadmap.md`, `docs/collaboration.md`, ADR, benchmark note и презентация. Миграция 011 понадобилась после первых journal-тестов: существующий PostgreSQL constraint разрешал только schema 1–10 и отклонял 11/12. До исправления два новых теста завершались `Unavailable`; после миграции `cargo run --locked -p pxel-server --features dev-auth -- migrate` прошёл. Первый `pnpm test:integration` был запущен без сервера и дал только ECONNREFUSED; повтор с реально работающим server прошёл 37/37. Первый `just check` остановился на rustfmt в server adapter; после `cargo fmt --all` повторный gate полностью прошёл.

Реально прошли: domain tests 39 (плюс один долгий ignored), storage codec 17/17, journal tests 20/20 (один benchmark ignored в обычном запуске), server wire unit test, полный `just check` (fmt, Clippy all features, Rust workspace tests/build, WASM target/build, proto drift, Prettier, ESLint, TypeScript, Vitest 68/68, production web build), Playwright 44/44 при DPR 1/2, `pnpm test:integration` 37/37, `pnpm test:recovery` 1/1 с новой двухкратной hard-restart/undo/retry проверкой и `pnpm --filter @pxel/web presentation:check` на 1440×900 и 390×844. Review нашёл, что `fetch_all` мог выделить память пропорционально всем payloads окна; tail scan переведён на SQLx streaming, journal subset 15/15 повторён. Ещё review выявил несоответствие: decoder отвергал event >4 MiB, а encoder не проверял размер. Отдельный canonical CellCompensation в wire v1 мог сломать старого клиента, поэтому сервер отправляет совместимый SetCellValue/ClearCell, сохраняя тип компенсации только в versioned persisted event. Encoder теперь измеряет `encoded_len` до INSERT, возвращает typed `PayloadTooLarge`; адресный codec test, workspace `cargo check` и TypeScript typecheck после этой правки прошли; повторный полный `just check` и `presentation:check` также прошли. Ignored real-PostgreSQL timing harness для финального streaming-кода был запущен отдельно: один commit с 1000 unrelated events в tail занял 22.461 ms; прежний `fetch_all` prototype дал один 19.055 ms sample и не является итоговым показателем. Это не p95 и не latency UI или fsync-only. [Условия](docs/benchmarks/2026-09-29-undo-tail-local.md). Не запускались повторно cross-host сетевой gate, Linux/Compose/remote CI и полный 100 000-command convergence stress. Следующий срез фазы 4: server-side `CopyRange` для диапазона с формулами либо отдельный compatibility-gated cut/move; general undo остаётся открытым.

## 2026-09-29 — фаза 4, восстановление формульного preimage при undo

Следующий срез [ADR-029](docs/adr/029-formula-preimage-undo.md) расширяет `UndoCell`: если одиночная собственная запись value/clear затёрла формулу, компенсация может восстановить сохранённый bound AST. Actor разрешает это только при `last_axis_change_version <= target.version`; после вставки/удаления/расширения осей возвращается typed `NotUndoable`, поскольку отсутствовавшая в projection историческая формула не проходила structural rebase. Журнал по-прежнему защищает stable address от любой поздней записи внутри CAS-транзакции. Новая persisted schema 13 хранит формулу в компенсации; schema 12 остаётся для value/blank компенсаций, wire v1 отправляет существующий `SetCellFormula`, расчёт выполняется сервером на новой версии. Старые event bytes не менялись. General formula undo, batch/structural undo и redo открыты.

Изменены domain, collab, storage и storage-codec, domain/codec tests, живой PostgreSQL/WebSocket integration test, browser и hard-restart recovery tests, docs/collaboration, roadmap, ADR и презентация. Первый живой test вернул `Unavailable`: codec ещё запрещал формулу в компенсации; после code review введены schema 13, migration 012 и round-trip/forged-old-schema tests, чтобы прежний schema-12 reader не получил неразборчивое событие. Адресный тест после исправления прошёл, включая расчёт `=A1+1` → `8` и отказ после вставки строки. Полный `pnpm test:integration` прошёл 38/38; Playwright 46/46 при DPR 1/2; `pnpm test:recovery` 1/1 с новым перезапуском после восстановления формулы и проверкой hash/result/idempotent retry. `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 68/68 и production web build. `pnpm --filter @pxel/web presentation:check` проверил 10 слайдов при 1440×900 и 390×844. Новых latency benchmark нет; прежние показатели не распространяются на эту операцию. Linux/Compose/remote CI, cross-host slow-client tests и полный network convergence gate не запускались в этом срезе.

После выделения schema 13 реально выполнены `pnpm proto:generate`, `cargo run --locked -p pxel-server --features dev-auth -- migrate`, адресный codec test, живой PostgreSQL/WebSocket integration test и Playwright 2/2 при DPR 1/2. Повторный полный `just check`, `pnpm test:recovery` 1/1 и `presentation:check` прошли уже на schema 13. После schema 13 полный integration suite повторён 38/38. Первый повтор полного Playwright suite дал 45/46: старый сценарий обычного undo на DPR 2 упал, но точный assertion не был сохранён. Этот сценарий затем прошёл отдельно 10/10, следующий полный suite прошёл 46/46. Причина единичного падения не установлена; критерий закрытия этого test-stability вопроса — сохранить trace следующего сбоя и исправить подтверждённую гонку в тесте либо приложении, не ослабляя проверку.

## 2026-09-29 — фаза 4, server-authoritative CopyRange смешанного диапазона

Реализован ограниченный [ADR-030](docs/adr/030-authoritative-copy-range.md): клиент отправляет source/destination stable anchors, размеры и точную `base_version`; actor берёт до 1,000 исходных ячеек из одного текущего head, смещает формульные ссылки общим Rust-кодом, повторно bind-ит формулы на сервере и сохраняет значения, формулы и пустоты как frozen schema-14 event. Перекрывающаяся копия читает весь прежний source до любой записи. Один candidate проходит доменные лимиты и CAS, операция получает одну версию; retry того же ID возвращает receipt, другой intent с тем же ID получает conflict. Старый source получает новый non-retryable `SourceVersionChanged` с `resync_required` вместо вводящего в заблуждение retryable conflict. Wire v1 не умеет прямой canonical patch для CopyRange: ACK содержит version/hash, подписчик и replay используют HTTP projection refresh. Это временный дополнительный read, не измеренный latency target.

Изменены domain/collab/formula/storage-codec/storage, protobuf wire и persisted schemas, generated TS и descriptor, server adapter, frontend clipboard/store/Canvas, миграция 013, domain/codec/frontend/integration/recovery/browser tests, `docs/roadmap.md`, `docs/collaboration.md`, ADR-030 и слайды. `cargo run --locked -p pxel-server --features dev-auth -- migrate` успешно применил миграцию 013 к локальной PostgreSQL. Первый targeted integration test выявил только несоответствие ожидаемого canonical текста (движок печатает `=(C3*2)`); assertion исправлен по фактическому нормализованному формату. Первый двухклиентский browser test выявил настоящий дефект: `DocumentHeadChanged` с event без wire canonical operation не запускал refresh у удалённой вкладки. Исправлен guard и добавлен Vitest regression. Review также выявил, что stale source ошибочно маркировался retryable; добавлен отдельный код 17 и тест его флагов. Добавлен тест replay barrier, который при CopyRange требует projection resync.

Реально выполнено: `pnpm proto:generate`, `cargo fmt --all`, `just check` после окончательного code-17 изменения — прошёл Rust fmt, Clippy all features/default, workspace tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. Первый `just check` остановился на rustfmt, второй на Prettier трёх файлов; оба замечания исправлены до финального успешного запуска. До code-17 изменения полный `pnpm test:integration` прошёл 39/39, полный Playwright 48/48 при DPR 1/2 и `pnpm test:recovery` 1/1 с hard restart, state hash, формульным результатом и idempotent retry. После code-17 изменения адресный живой CopyRange test с новым rejection и replay fallback прошёл 1/1, браузерный адресный тест прошёл 2/2 при DPR 1/2. `pnpm --filter @pxel/web presentation:check` проверил 10 слайдов на 1440×900 и 390×844 до code-17 изменения; презентация после этого не менялась. `git diff --check` прошёл, кроме информационного предупреждения о CRLF→LF в двух proto-файлах. Не запускались повторно после code-17 изменения полный integration/Playwright/recovery suites; не запускались Linux/Compose/remote CI, cross-host stress и полный сетевой convergence gate. Новых latency benchmark нет, поэтому показатели прежних срезов не приписываются CopyRange. Фазы 3–4 остаются частично открыты, фазы 5–6 не реализованы.

Следующий минимальный этап: проверить корректность и стоимость direct canonical range patch в новой wire-версии либо завершить оставшиеся Phase-3 network gates; cut/move и batch undo требуют отдельных контрактов. Перед snapshots (фаза 5) нужен план state machine `UPLOADING → STORED → VERIFIED → ACTIVE`, fault injection и contract suite BlobStore. Презентация остаётся живым промежуточным отчётом и обновляется только по реально проверенным срезам.

## 2026-09-29 — phase-3 convergence harness со структурой и CopyRange

Расширен существующий `crates/spreadsheet-domain/tests/convergence.rs`: у 3 виртуальных реплик по-прежнему есть задержки, дубликаты, доставка событий не по порядку, интервалы offline/replay и многократное восстановление серверной projection из журнала. Детерминированные сценарии теперь включают `InsertRows`, `DeleteRows`, `InsertColumnsRebased`, `DeleteColumns` и frozen `CopyRange` обычных значений и пустот наряду с value/clear writes. Адреса выбираются по живым stable IDs. Для каждого seed проверяется, что все пять новых ветвей были исполнены, а финальные версии и domain hashes всех реплик равны каноническому replay. Никакие production paths, persisted bytes или wire DTO этим изменением не менялись.

Изменены domain convergence test, `docs/testing.md`, `docs/roadmap.md`, презентация и этот RUNBOOK. Реально выполнены `cargo fmt --all` и smoke `cargo test --locked -p spreadsheet-domain --test convergence seeded_convergence_smoke -- --nocapture`: 5 seeds × 300 commands × 3 replicas, прошёл за 0.43 s. `just convergence` после добавления строк и CopyRange прошёл 100 × 1000 × 3 за 81.02 s; после добавления столбцов и явной проверки operation mix полный прогон повторён и прошёл за 82.67 s. `cargo fmt --all --check` и `cargo clippy --locked -p spreadsheet-domain --test convergence -- -D warnings` прошли. Первый `pnpm format:check` остановился на разметке презентации; после Prettier повторный `pnpm format:check`, `presentation:check` на 1440×900 и 390×844 и `git diff --check` прошли. Полный `just check` повторно не запускался: production-код не менялся. Это wall-clock длительность теста, не latency одной пользовательской операции и не benchmark production code. Реальный WebSocket/PostgreSQL, ACL revoke, формулы в CopyRange, calculation patches и серверный restart этим доменным harness не проверяются; соответствующие network/recovery gates остаются открытыми.

Продолжение этого этапа: добавлен `tests/integration/document.test.mjs` сценарий на пяти документах через живой PostgreSQL/WebSocket. Два клиента делают конкурентные записи; затем последовательно вставляют строку и столбец, пишут в новую ячейку, копируют frozen 2×2 диапазон, повторяют тот же copy ID, удаляют исходные строку и столбец. Reconnect с версии 1 получает события 2–6 и барьерную подсказку head 9 с `resync_required=true`, поскольку schema-14 CopyRange не представим прямым wire v1 patch. Reconnect с версии 7 получает непрерывные версии 8–9 и финальный hash, равный HTTP projection и последнему commit. Адресный прогон прошёл 1/1; полный `pnpm test:integration` на локальной PostgreSQL прошёл 40/40. Это проверяет границу replay/projection fallback, но не случайные задержки, ACL-гонки, server restart в том же сценарии или 100×1000 сетевых операций. Обновлены roadmap, testing docs и презентация; новые latency замеры не заявлены.

## 2026-09-29 — фаза 5, внутренний snapshot image и replay хвоста

Начат первый ограниченный срез [ADR-031](docs/adr/031-snapshot-image-and-tail-replay.md). `Workbook::snapshot_image` выдаёт авторизуемый внутренний образ текущей одно-листовой sparse projection: stable axes, authored cells включая bound formulas, retired row/column IDs, `last_axis_change_version` и optional CalculationContext. `Workbook::restore_snapshot` заново выполняет bounded hydrate, проверяет duplicate/live tombstones, диапазон версии изменения осей и ожидаемый domain hash. Тест строит книгу с вставкой/удалением строк и столбцов, формулой и persisted context, восстанавливает образ, применяет следующее committed event и сравнивает с direct replay. Он также проверяет запрет повторного использования удалённого ID и обнаружение неверного hash. Рассчитанные значения в образ не входят.

Изменены `crates/spreadsheet-domain/src/workbook.rs`, новый `tests/snapshot_image.rs`, ADR-031, `docs/roadmap.md`, `docs/persistence.md`, презентация и этот RUNBOOK. Адресный `cargo test --locked -p spreadsheet-domain --test snapshot_image` прошёл 1/1; при добавлении формулы первый compile остановился из-за отсутствующего `map_err` у `FormulaValidationError`, после исправления тест повторно прошёл 1/1. Финальный повтор `just check` после формульного уточнения прошёл Rust fmt/Clippy/tests/build, WASM target/build, proto drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. `pnpm --filter @pxel/web presentation:check` проверил 10 слайдов на 1440×900 и 390×844; `git diff --check` прошёл. Не реализованы binary snapshot schema, PostgreSQL metadata/state machine, BlobStore запись/активация, corruption fallback, S3 contract или server startup from snapshot. `SnapshotImage` сам по себе не является durable snapshot и не включён в runtime recovery.

## 2026-09-29 — фаза 5, отдельный бинарный snapshot codec

Второй срез [ADR-032](docs/adr/032-versioned-binary-snapshot-codec.md) добавляет `proto/pxel/storage/v1/snapshot.proto` с самостоятельным `snapshot_schema_version=1`; event envelope и wire protocol не становятся snapshot format. `spreadsheet-storage-codec` кодирует внутренний `SnapshotImage`, проверяет длину до 32 MiB перед decode/encode, ожидаемые document/version/hash/engine/profile, согласование CalculationContext с writer labels, границы осей/ячеек/tombstones и восстановленный domain hash. Неизвестная snapshot schema отвергается typed `UnsupportedSchema`; большие файлы — typed `SnapshotTooLarge`. Контракт готов для будущего ACTIVE metadata, но сам по себе не публикует объект. BlobStore byte checksum и PostgreSQL metadata/state machine ещё предстоят.

Изменены snapshot proto, descriptor, storage codec и его unit test, ADR-032, roadmap/persistence docs, презентация и этот RUNBOOK. Реально выполнены `pnpm proto:generate`, `cargo fmt --all`, `cargo check --locked -p spreadsheet-storage-codec`, адресный `cargo test --locked -p spreadsheet-storage-codec snapshot_codec` 1/1 и `cargo clippy --locked -p spreadsheet-storage-codec --all-targets -- -D warnings`. Тест проходит формулу, context и tombstone через encode/decode, затем применяет tail event и сравнивает hash с direct replay; проверяет неизвестную schema, duplicate retired ID, mismatch метаданных/context и 32 MiB cap. Финальный `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. `pnpm --filter @pxel/web presentation:check` проверил 10 слайдов на 1440×900 и 390×844; `git diff --check` прошёл. Автоматический snapshot scheduler, durable metadata, activation/fallback, S3 и benchmark восстановления не реализованы; текущий server restart по-прежнему использует полный журнал.

## 2026-09-28 — side task: iOS squircle для вариантов логотипа

По отдельному запросу пользователя изменены только design/logos/01–10 SVG, README этой галереи и gallery/gallery-mobile screenshots. Внешние rect с rx заменены настоящими SVG paths с cornerRadius = width × 0.2237 и cornerSmoothing = 0.6. Для варианта 07 применено к четырем отдельным плиткам. Внутренние glyphs и исходный reference сохранены. Генератор figma-squircle 1.1.0 (MIT) использован offline в ignored .local/squircle-edit, архив проверен по registry SHA-512; зависимости приложения/lockfiles не менялись. Chromium проверил 1280/390 px: 10 cards, 0 broken images, 0 page errors, нет horizontal overflow; screenshots обновлены. Основная разработка, git state и субагенты не затрагивались.

## 2026-09-29 — фаза 6, ограниченный разбор XLSX package metadata

Начат первый срез [ADR-043](docs/adr/043-bounded-xlsx-package-inspection.md). Новый используемый crate `spreadsheet-xlsx` инспектирует ZIP/OOXML и разрешает workbook/worksheet relationships в заголовки листов. `pxel-server xlsx-inspect <path>` запускает эту проверку до конфигурации БД; выводит только счетчики, без содержимого файла. Проверяются фактические распакованные bytes и лимиты ZIP/XML, запрещены обход пути, неоднозначные имена, внешние связи, DTD/entities, шифрование и распознаваемые активные части. Изменены workspace manifest/lock, серверный CLI, новый crate, compatibility/roadmap, ADR-043, презентация и RUNBOOK. `zip 8.6.0` используется только с deflate; `quick-xml 0.42.0` — streaming XML parser. Оба crate не являются spreadsheet engine.

Адресный `cargo test --locked -p spreadsheet-xlsx` прошёл 3/3: два листа, traversal/external relationship и DTD/duplicate sheetId. `cargo clippy --locked -p spreadsheet-xlsx --all-targets -- -D warnings` прошёл. Первый `just check` остановился только на Prettier для roadmap и презентации; после форматирования повторный `just check` прошёл целиком: Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 71/71, production web build. `cargo deny check` и `cargo audit` завершились успешно; deny оставил прежние предупреждения о дублирующихся транзитивных версиях. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов на 1440×900 и 390×844. Новых performance benchmarks нет.

Ограничение: это inspection metadata, а не импорт ячеек, normalized IR, экспорт или изолированный worker. CLI пока не имеет жесткого OS-level timeout; пятсекундный бюджет кооперативный. На следующем срезе нужны cell/shared-string parsing в bounded IR, тестовый корпус и parent/child process boundary до включения upload API. Production Excel round-trip и phase-6 acceptance не заявлены.

## 2026-09-29 — фаза 6, scalar IR без публикации документа

Второй срез [ADR-044](docs/adr/044-normalized-xlsx-scalar-ir.md) добавил `normalize_reader`/`normalize_file` и отдельную XLSX-specific `NormalizedWorkbook`: листы, one-based координаты, finite numbers, boolean, простые shared/inline strings и исходный текст формулы. Серверный CLI `xlsx-preview <path>` использует этот путь и выводит только счетчики. Формульный XLSX cache намеренно игнорируется; серверное связывание формул и domain IDs не выполняются. `&amp;` и числовые XML character references обрабатываются, неизвестные сущности отвергаются. Дубликат адреса, неверный shared index, unsupported rich text и malformed structure отклоняют весь разбор. В IR ограничены 100 000 cell elements (включая пустые), 32 MiB суммарного развернутого текста, размеры одной строки/формулы и координаты Excel sheet. Лимит 32 MiB предотвращает раздувание памяти при множественных ссылках на одну большую shared string.

Добавлен воспроизводимый синтетический [fixture](fixtures/xlsx/minimal_scalars.xlsx) с [генератором](fixtures/xlsx/generate_minimal.py) и provenance: два листа, пять значений, неверный cached formula result. Он не заявлен как Excel-verified. Изменены XLSX crate, серверный CLI, README, compatibility docs, ADR-043/044, презентация и RUNBOOK. Команда `python fixtures/xlsx/generate_minimal.py` реально создала fixture; `cargo run --locked -q -p pxel-server -- xlsx-preview fixtures/xlsx/minimal_scalars.xlsx` вывела `2 sheets, 5 cells`. Адресные `cargo test --locked -p spreadsheet-xlsx` прошли 7/7, включая IR expansion cap; Clippy crate прошёл. Первый `just check` остановился на незапущенном Rust fmt после добавления теста; форматирование выполнено и повторный `just check` прошёл целиком: Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов desktop/mobile. Новых performance benchmarks нет.

Открыто: отдельный sandboxed child process и framed parent protocol, parent-side schema validation, compatibility report, стили/даты/merge/frozen panes, публикация документа, export/reimport и ручная Excel validation. Этот срез не включает upload endpoint и не закрывает gate фазы 6.

## 2026-09-29 — фаза 6, процессная граница XLSX preview

Срез [ADR-045](docs/adr/045-xlsx-worker-framed-process.md) перевёл локальный `xlsx-preview` на дочерний режим того же binary `xlsx-worker`. Родитель передаёт только canonical UTF-8 путь в фиксированном version-1 frame длиной до 4096 bytes; ответ содержит только status, число листов и ячеек. Inherited environment очищается (на Windows оставляется SystemRoot для запуска), stderr дочернего процесса не пересылается, лишние stdout bytes и неизвестная версия отклоняются. Вызов ограничен 10 секундами; при timeout/ошибке parent убивает и reap-ит child. Парсер остаётся в `spreadsheet-xlsx`, в основной серверный процесс XLSX bytes не поступают. Прямой `xlsx-inspect` CLI entry удалён. `tokio/process` включён в существующую закреплённую зависимость; `tempfile` добавлен только как dev dependency для интеграционного теста.

Первый реальный CLI-прогон завис до deadline: worker ожидал EOF на stdin, а parent удерживал pipe handle. После явного `drop(stdin)` команда на fixture вернула `XLSX preview: 2 sheets, 5 cells`. Повреждённый ZIP дал typed generic rejection и exit code 1 без вывода содержимого. Интеграционный тест `cargo test --locked -p pxel-server --test xlsx_worker_cli` прошёл 3/3: нормальный файл, поврежденный файл с private-marker без утечки и неизвестная версия запроса. `cargo clippy --locked -p pxel-server --all-targets -- -D warnings` прошёл; presentation check — 10/10 на desktop/mobile. Финальный `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. Linux cgroups/Windows Job Objects и S3 contract в этом срезе не запускались.

Это **процессная**, но ещё не OS security sandbox: worker имеет права родительского OS user. Нет cgroups/Job Objects, process-group kill, private temp/read-only input, FD limit или parent-side IR validation. Результат пока только счетчики; import API и публикация документа не подключены. Следующий срез обязан передавать bounded versioned IR и валидировать его в parent, после чего внедрить ограничения ОС до приема пользовательских файлов.

## 2026-09-29 — четыре сглаженные плитки в логотипе «01 Квартет»

По отдельному запросу пользователя изменён `design/logos/01-quartet.svg`: четыре внутренних прямоугольника 14×14 заменены SVG-контурами, построенными тем же offline-генератором `figma-squircle 1.1.0`, что и внешний зелёный контур. Для каждой плитки `cornerRadius = 14 × 0.2237 = 3.1318`, `cornerSmoothing = 0.6`; координаты и цвета сохранены. Обновлён `design/logos/README.md`. В Chromium SVG успешно отрисован: пять path, четыре внутренние фигуры с указанным радиусом, ноль rect. Просмотрен локальный preview. Полные проверки приложения не запускались: изменение ограничено отдельным SVG галереи.

## 2026-09-29 — «Квартет»: плитки 15×15

По просьбе пользователя четыре внутренние плитки design/logos/01-quartet.svg увеличены с 14×14 до 15×15 без смещения центров (22/42 по обеим осям). Контуры повторно построены offline через figma-squircle 1.1.0 с cornerRadius = 15 × 0.2237 = 3.3555 и cornerSmoothing = 0.6; цвета и внешний контур сохранены. Обновлён design/logos/README.md. Проверка в Chromium: пять path, ноль rect, у четырёх плиток bbox 15×15 и ожидаемые translate; локальный preview просмотрен. Первая проверка ошибочно сравнила координаты через viewport-scaled getCTM() и была заменена проверкой SVG bbox/transform; повторная проверка прошла. Полные тесты приложения не запускались для этого изолированного SVG-изменения.

## 2026-09-29 — сетевой convergence gate и recovery после перезапуска

Добавлен `tests/integration/network-convergence.test.mjs`: два реальных WebSocket-писателя выполняют детерминированные конкурентные записи, включая коллизии одной ячейки, повтор с тем же operation ID и reconnect. Подписчик отключается на 20 версий и догоняет replay; поздний подписчик получает bounded replay. Тест сверяет непрерывную серверную версию, порядок событий, победителя каждой конкурентной записи и итоговый canonical hash с HTTP projection из PostgreSQL. Параметры `PXEL_NETWORK_SEEDS` (1–100, по умолчанию 3) и `PXEL_NETWORK_COMMANDS` (чётное 2–1000, по умолчанию 100). Добавлен `tests/recovery/convergence-restart.test.mjs`: после 50 конкурирующих commit сервер принудительно останавливается, новая сессия проверяет replay, hash, идемпотентный retry и последующие конкурентные записи. Обновлены `package.json`, `scripts/smoke.mjs`, `scripts/recovery.mjs`, README, testing/roadmap и презентация; production-код не менялся.

Первый адресный сетевой тест вернул HTTP 503: тестовый dev-auth account превышал лимит 32 символа. Исправлен только test fixture. Затем прошли default 3×100 (около 4 с), 1×1000 (около 12,5 с) и полный профиль 100×1000: 100 000 подтверждённых записей, 1 310 127 ms выполнения теста, 0 ошибок. Отдельный restart test прошёл дважды (около 1,9 с каждый). `just smoke` прошёл: integration 42/42, recovery 2/2, Chromium Playwright 48/48 при DPR 1/2. Это функциональный stress, **не** измерение p95 commit/ACK. Стенд: Windows 11 Pro 10.0.26100, Intel Core Ultra 9 185H, RAM 15,61 GiB, PostgreSQL 18.6 на loopback 127.0.0.1:55432, `synchronous_commit=on`, `fsync=on`, `full_page_writes=on`, сервер debug build. База содержит тестовые операции этого прогона. Cross-host, Linux/Compose, комплексные ACL+formula stress и production latency gate пока не выполнялись; фаза 3 полностью не закрыта.

## 2026-09-29 — bounded XLSX IR через дочерний процесс

Следующий срез фазы 6 ([ADR-046](docs/adr/046-bounded-xlsx-worker-ir.md)) заменил counts-only ответ worker v1 на version-2 frame с XLSX-specific scalar IR и лимитом payload 64 MiB. Родитель десериализует и повторно проверяет число листов, имена, уникальный возрастающий порядок координат, Excel bounds, число ячеек, размер текста/формул и finite нормализованные числа. Неизвестные поля и версии отвергаются. `xlsx-preview` остаётся локальным CLI и печатает только счётчики; upload API, stable IDs и запись в PostgreSQL не добавлены. Изменены XLSX crate, server worker, Cargo.lock, README, roadmap, ADR-045/046 и презентация. Сырой JSON здесь только одноразовый локальный transport, не canonical hash и не persisted format.

Первый `cargo test --locked` после новой зависимости `serde` не запустился из-за устаревшего Cargo.lock; `cargo check --offline -p pxel-server` обновил lock и успешно проверил код. После этого прошли `cargo test --locked -p pxel-server --test xlsx_worker_cli` 3/3, адресные server worker unit tests 1/1, затем 2/2 после тестов forged IR, и `cargo test --locked -p spreadsheet-xlsx` 7/7. Родительский decoder тестирует дубликат адреса, отрицательный ноль и неизвестное поле. OS-level sandbox, parent memory limit, compatibility report и import/export остаются открытыми.

После кодового review parent validation приведена в соответствие с parser: имена листов ограничены 31 Unicode-символом и уникальны без учёта регистра; добавлен forged IR test для `Data`/`data`. Первая правка этого условия пропустила открывающую фигурную скобку и дала синтаксическую ошибку во всех адресных Rust проверках; ошибка исправлена, затем `cargo test --locked -p pxel-server xlsx_worker::tests` прошёл 2/2, CLI integration — 3/3, Clippy для XLSX/server — без замечаний. До этой правки полный `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, proto drift, Prettier, ESLint, TypeScript, Vitest 71/71 и production web build. После правки полная цепочка не повторялась, потому что изменение ограничено validator и его адресным тестом. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов на 1440×900 и 390×844; `cargo deny check` и `cargo audit` завершились успешно, deny оставил прежние duplicate-transitive предупреждения. `git diff --check` прошёл.

## 2026-09-29 — совместный тест формулы и ACL revoke; восстановление после HTTP 429

В `tests/integration/document.test.mjs` добавлен живой PostgreSQL/WebSocket сценарий: владелец сохраняет `=A1+1`, viewer после grant получает серверный расчёт и обновление после правки A1, затем revoke закрывает его подписку. После нового commit viewer не может получить calculation ни по HTTP, ни через WebSocket; владелец видит результат новой версии. Это один локальный сценарий, не масштабный ACL+formula convergence stress и не cross-host gate. Production backend и контракт ACL не менялись.

Первый `just smoke`: integration 43/43 и recovery 2/2, но браузер 46/48. Сохранённые Playwright trace обоих сбоев показали HTTP 429 на GET projection при DPR 1; клиент сразу показывал «Не удалось открыть таблицу». В `apps/web/src/document/store.ts` добавлены до четырёх попыток только для идемпотентного GET при 429 с задержками 200/400/800 ms, с проверкой поколения синхронизации между попытками. Добавлен Vitest, подтверждающий 429 → 200 без потери документа; 403 и другие ответы не получают этот retry. После изменения `pnpm test` прошёл 72/72, `pnpm typecheck` и `pnpm lint` прошли. Повторный `just smoke` прошёл целиком: integration 43/43, recovery 2/2, Chromium Playwright 48/48 при DPR 1/2. Docs/roadmap и слайд обновлены. Показатели p95 не измерялись; Linux/Compose/remote CI и совмещённый масштабный стресс остаются открытыми.

## 2026-09-29 — scalar XLSX writer и библиотечный round-trip

В `spreadsheet-xlsx` добавлен [ADR-047](docs/adr/047-scalar-xlsx-export.md) и публичный `export_scalar_xlsx(&NormalizedWorkbook)`: вручную создаются OOXML content types, relationships, workbook и sheets внутри ZIP. Записываются finite numbers, boolean, inline strings с сохранением пробелов и исходный текст формулы без недоверенного cached result. Выход проходит те же ограничения 8 MiB на part и 32 MiB на package, что собственный reader; ZIP entries сохраняются без compression, чтобы не нарушать anti-bomb ratio читателя. Перед выдачей bytes проверяются XML characters, Excel sheet names, координаты, размеры и порядок ячеек. Изменены XLSX crate, docs/compatibility, roadmap, презентация и RUNBOOK. Domain model и HTTP export пока не подключены.

Первый `cargo test --locked -p spreadsheet-xlsx` после добавления XML escaping прошёл 10/10. После расширения fixture на значимые пробелы тест обратного чтения упал с `InvalidPackage`: тестовое имя листа заканчивалось апострофом, который новый validator правильно отвергал. Имя изменено на допустимое, адресный round-trip test прошёл. Затем добавлен тест на запрещённые XML characters и Excel sheet names. Финальный `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, proto drift, Prettier, ESLint, TypeScript, Vitest 72/72 и production web build. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов на 1440×900 и 390×844, `git diff --check` — без ошибок. Отдельный browser/DB smoke после library-only изменения не запускался; предыдущий smoke 43/43 + 2/2 + 48/48 к нему не относится. Собственный reader не доказывает открытие файла Excel; ручной Excel corpus и документный import/export остаются открытыми.

## 2026-09-29 — owner-only HTTP-экспорт подтверждённой scalar-книги

Шестой срез фазы 6 ([ADR-048](docs/adr/048-owner-audited-document-xlsx-export.md)) добавил `GET /api/documents/{id}/export.xlsx`. Сервер берёт подтверждённую projection, переводит stable row/column IDs в текущие координаты, переносит значения и server-rendered formula source в отдельную `NormalizedWorkbook` IR и записывает bounded XLSX в `spawn_blocking`. Перед занятием единственного слота конвертации PostgreSQL проверяет owner; после подготовки файла повторяет owner/tenant/state-проверку и транзакционно добавляет append-only `export_audit_events` с версией, SHA-256 и размером. Ответ не отправляется при ошибке аудита. Миграция `202609290016_export_audit.sql` применена локально; SQLx checked-query metadata обновлены. Изменены server route, storage adapter, интеграционные и браузерный тесты, README, compatibility/roadmap, презентация и RUNBOOK. В аудит не попадает содержимое документа.

Storage test проверил отказ viewer, разрешение owner, записанные metadata и запрет удаления audit record. Живой PostgreSQL/WebSocket/HTTP test создал документ, сохранил число и формулу, проверил ZIP и исходные OOXML значения, а затем отказ viewer и постороннему tenant. Это проверка нашей scalar-схемы, **не** ручная проверка файла в Excel. Экспорт пока доступен по HTTP владельцу без кнопки в UI. Повторный запрос создаёт новую audit-запись: аудит фиксирует подготовку ответа, а не факт получения клиентом всех байтов.

Первый полный `just smoke` после HTTP-маршрута прошёл integration 44/44 и recovery 2/2, но Chromium 47/48: TSV paste test иногда принимал старую метку «Сохранено» за подтверждение новой версии. Тест теперь ждёт version 2 через HTTP; production paste-код не менялся. Затем найдено, что viewer мог занимать CPU-слот до транзакционного отказа. Добавлена ранняя owner-проверка с повторной проверкой в audit transaction. Первый `just check` остановился на Prettier для нового integration test и презентации; файлы отформатированы. После финальной правки `cargo sqlx prepare --workspace -- --all-targets --all-features`, `just sqlx-check`, `just check` и `pnpm --filter @pxel/web presentation:check` прошли. Финальный `just smoke` прошёл integration 44/44, recovery 2/2, Chromium 48/48 при DPR 1/2; `git diff --check` прошёл. В `just check` прошли Rust fmt/Clippy/tests/build, WASM target/build, protobuf drift, Prettier, ESLint, TypeScript, Vitest 72/72 и production web build. Отдельные latency/throughput benchmark этого экспорта не запускались; нельзя переносить на него показатели других срезов.

Ограничения: только scalar values/formula source; стили, даты, merges, frozen panes и import/reimport документа отсутствуют. Excel golden fixture ещё не проверен. Отзыв ACL во время передачи HTTP body не покрыт отдельным send-gate test. Следующий минимальный срез — безопасная публикация импортированного scalar IR в новый документ через проверяемую атомарную транзакцию; OS sandbox worker и полный compatibility report остаются отдельными обязательными gates до приёма произвольных пользовательских файлов.

## 2026-09-29 — скачивание XLSX из интерфейса

Седьмой узкий срез фазы 6 добавил в меню документа кнопку «Скачать XLSX». Она вызывает ранее проверенный owner-only HTTP export, сохраняет полученный Blob как `PXEL.xlsx`, показывает понятную ошибку при отказе viewer/неподдерживаемом профиле и не доступна, пока есть pending operations или соединение не готово. Изменены `apps/web/src/App.tsx`, `apps/web/src/document/store.ts`, browser E2E, README, roadmap, презентация и RUNBOOK. Backend, схема журнала и XLSX writer не менялись. Кнопка сохраняет подтверждённую сервером версию; локальный незавершённый ввод в файл не включается.

Первый вариант browser test читал скачанный файл через `node:fs`/`Buffer`; web TypeScript-проект намеренно не включает Node-типы, поэтому `pnpm typecheck` завершился с TS2591. Удалена эта дублирующая проверка байтов: ZIP и OOXML уже проверяет реальный HTTP integration test, браузерный тест проверяет событие download, имя и наличие файла. Следующий полный `just smoke` завершился integration 44/44, recovery 2/2 и Chromium 49/50: старый TSV paste test отправлял синтетический paste до появления sheet projection после создания документа. Ожидание версии **после** paste не могло это исправить. Тест теперь ждёт появления save-status/листа **до** paste; production paste-код не менялся.

Финальный `just smoke` прошёл integration 44/44, recovery 2/2, Chromium 50/50 при DPR 1/2. Первый `just check` после изменения слайда остановился на Prettier для `presentation/index.html`; после форматирования повторный `just check` прошёл Rust fmt/Clippy/tests/build, WASM target/build, proto drift, Prettier, ESLint, TypeScript, Vitest 72/72 и production web build. `pnpm --filter @pxel/web presentation:check` прошёл 10 слайдов на 1440×900 и 390×844. Excel golden fixture, import/reimport, ACL-send-gate и отдельный benchmark экспорта не запускались и остаются открытыми. Анализ следующего среза предполагал, что нужен отдельный import genesis event. Это предположение пересмотрено в ADR-049: для одного листа существующие события можно записать в одной транзакции.

## 2026-09-29 — атомарный локальный импорт scalar XLSX

Восьмой ограниченный срез фазы 6 ([ADR-049](docs/adr/049-atomic-local-xlsx-document-import.md)) добавил dev-auth CLI `xlsx-import <path> <workspace_id> <actor_id>`. Существующий дочерний XLSX worker отдаёт normalized IR; parent повторно валидирует её и создаёт candidate projection из `CreateWorkbook`, одного batch `SetCellValues` и по одному `SetCellFormula` на формулу. Сервер связывает A1-ссылки со stable IDs до публикации. `PgDatabase::create_imported` применяет все события к candidate, проверяет hash/лимиты/автора/версии и в **одной** транзакции пишет `documents`, owner permission и весь append-only journal. Неудачный insert откатывает всю книгу. Точный повтор prepared events для того же document ID сверяется по operation ID и canonical event hash. Команда генерирует новый document ID при каждом запуске, поэтому повторный запуск CLI создаёт новый импорт, а не retry прежнего намерения.

Добавлен синтетический одно-листовой fixture и живой integration test: CLI импортирует четыре заполненные ячейки, включая формулу из worker, HTTP читает документ версии 3, затем owner экспортирует XLSX; двухлистовой fixture отклоняется до публикации. Storage test проверил exact retry, конфликт при подмене содержимого и откат после искусственного сбоя второго insert. Изменены `apps/server/src/main.rs`, `xlsx_import.rs`, `crates/spreadsheet-storage/src/journal.rs`, SQLx metadata, fixture generator и XLSX-файл, integration test/smoke script, README, compatibility/roadmap, ADR-049, презентация и RUNBOOK. Новых миграций и persisted schema нет.

Реально выполнены `cargo test --locked -p spreadsheet-storage journal::tests::imported_events_publish_atomically_and_retry_exactly` (1/1), `cargo test --locked -p pxel-server xlsx_import --no-default-features` (2/2), `cargo test --locked -p pxel-server --features dev-auth xlsx_import` (2/2), `cargo sqlx prepare --workspace -- --all-targets --all-features`, `just check` и `just smoke`. `just check` прошёл Rust fmt/Clippy/tests/build, WASM, proto drift, Prettier/ESLint/TypeScript, Vitest 72/72 и web build. Его domain test на 100 000 sparse-ячеек занял 1762.09 s на этом Windows-стенде; это не benchmark import и не latency. `just smoke` прошёл integration 46/46, recovery 2/2, Chromium 50/50 при DPR 1/2. После полного `just check` добавлена только явная ошибка CLI без `dev-auth`; эта ветка проверяется отдельным targeted compile/check, а не повторным 30-минутным domain suite.

Ограничения: только один лист, максимум 1000 ячеек и 32 MiB encoded events; стили/merges/dates/форматирование не переносятся. Нет публичной загрузки, OS sandbox, malware scan, import job/audit, compatibility report, Excel golden corpus и полного edit/export/reimport gate. Результаты производительности импорта отдельно не измерялись. Фазы 3–5 и общий phase-6 gate остаются открытыми.

## 2026-09-29 — живой scalar XLSX round-trip после правки

Добавлен `tests/integration/xlsx-import.test.mjs` сценарий: локальный worker/CLI импортирует файл, владелец меняет A1 через WebSocket (версия 4), скачивает owner-only XLSX по HTTP, затем импортирует этот файл как вторую книгу. Тест сравнивает текущие координаты, типы и значения четырёх заполненных ячеек между книгами. Первая проверка намеренно требовала побайтного совпадения текста формулы и провалилась: export рендерит bound AST с дополнительными скобками, поэтому `=A1+B1` становится `=(A1+B1)`. Это эквивалентное выражение для fixture; тест теперь отдельно фиксирует оба ожидаемых текста и сравнивает положение/тип формулы. Production-код из-за этого не менялся, assertion на значения остальных ячеек не ослаблялся.

Первый `just smoke` после нового теста завершился integration 46/47 и не запускал recovery/browser из-за описанного несовпадения текста. После уточнения теста финальный `just smoke` прошёл integration 47/47, recovery 2/2 и Chromium 50/50 при DPR 1/2. Изменены integration test, roadmap, compatibility docs и RUNBOOK. Это поддержанный scalar round-trip нашего reader/writer, не доказательство совместимости с Microsoft Excel и не полный phase-6 gate.

## 2026-09-29 — XLSX compatibility issues и явное согласие на потери

Девятый срез фазы 6 ([ADR-050](docs/adr/050-bounded-xlsx-compatibility-issues.md)) закрывает тихую потерю части неподдержанных свойств. `NormalizedWorkbook` теперь содержит bounded/sorted typed issues без содержимого документа: дополнительные части пакета, workbook/worksheet metadata, форматирование строк и ячеек. OOXML reader собирает их при извлечении scalar cells. Дочерний worker передаёт отчёт в version-3 frame, parent валидирует коды, порядок, уникальность и sheet indices. `xlsx-import` по умолчанию отказывает до создания документа при непустом отчёте. Локальный `--allow-loss` явно принимает перечисленные потери; CLI всё равно печатает только категории и индексы листов. Библиотечный экспорт отвергает IR с issues. Persisted event schema и документный hash не меняются.

Добавлены синтетический fixture с merge и живой тест обоих режимов. Без флага получены `WorksheetMetadata` и отказ до публикации; с флагом опубликованы четыре поддержанные ячейки без merge. Отдельный тест reader проверил также dropped package part, workbook metadata и row/cell formatting. Parent decoder отвергает неизвестный код, неверный индекс и duplicate issues. Изменены XLSX crate, worker/CLI, normalized IR constructors, generator/fixture, integration test, README/compatibility/roadmap, ADR-050, презентация и RUNBOOK.

Первый адресный `cargo test --locked -p pxel-server --features dev-auth xlsx_worker` после смены frame version 2→3 провалился: тест помечал версию 3 как неизвестную. Проверка обновлена на версию 4 и повторно прошла 2/2; production-код не менялся ради теста. `cargo test --locked -p spreadsheet-xlsx` прошёл 12/12. `cargo test --locked -p pxel-server --features dev-auth xlsx_import` прошёл 2/2, адресный Clippy для XLSX/server прошёл. `just smoke` прошёл integration 48/48, recovery 2/2, Chromium 50/50 при DPR 1/2. После smoke дополнительно усилена parent validation для global/sheet issue codes; после неё `cargo test --locked -p spreadsheet-xlsx` прошёл 12/12, `cargo test --locked -p pxel-server --features dev-auth` прошёл 16/16 unit и 3/3 CLI, адресный Clippy, `pnpm format:check`, проверка 10 слайдов на 1440×900 и 390×844 и `git diff --check` прошли. Полный smoke после этой валидационной правки не повторялся. Ни один новый benchmark скорости импорта не выполнялся.

Ограничения: отчёт агрегирован по категории/листу и не является полной XLSX compatibility matrix; некоторые редкие XML extension points и содержимое `[Content_Types].xml` пока не классифицируются. `--allow-loss` доступен только локальному dev operator, а его решение не сохраняется как durable import job/audit. Не выполнены OS sandbox, malware scan, публичная загрузка, multi-sheet/domain styles, Excel golden проверка и полный gate фазы 6.

## 2026-09-29 — durable scalar XLSX import с несколькими листами

Десятый срез фазы 6 ([ADR-051](docs/adr/051-durable-multiple-sheets.md)) сохраняет 1–64 листа с суммарно 1000 заполненных scalar-ячеек. В domain добавлена `AddSheet` с проверкой глобальной уникальности stable sheet/row/column IDs и имён листов; кандидат книги строится до публикации. Local `xlsx-import` сначала создаёт все листы, затем значения и серверно связанные формулы; `create_imported` записывает всю последовательность одной транзакцией. Persisted event schema 15 отделена от wire v1; при replay через `AddSheet` подписчик получает projection resync. Snapshot schema 2 хранит ordered sheets и sparse authored cells; decoder всё ещё читает schema 1. Миграции 017/018 расширяют разрешённые версии snapshot/event. Обновлены domain, formula/collab, storage codec/journal/snapshots, server import/wire adapter, protobuf descriptor, SQLx metadata, тесты, compatibility/roadmap, презентация и этот RUNBOOK.

Проверки: `cargo check --locked --workspace --all-features` прошёл; `cargo test --locked -p spreadsheet-domain --test multi_sheet` 1/1; `cargo test --locked -p spreadsheet-storage-codec --lib snapshot_codec` 2/2, включая legacy schema 1; `cargo test --locked -p pxel-server --features dev-auth xlsx_import` 2/2; `cargo sqlx prepare --workspace -- --all-targets --all-features` прошёл после обеих миграций. Первый `just smoke` дал integration 47/48: в новом тесте ожидалось имя второго листа `Aux`, хотя fixture содержит `Plan`; исправлен только тестовый литерал. Повторный `just smoke` прошёл integration 48/48, recovery 2/2, Chromium 50/50. Первый quality pass обнаружил форматирование нового Rust-теста и двух web-файлов; `cargo fmt` и Prettier применены. Проверка презентации затем выявила race при загрузке URL `#s06`: IntersectionObserver запускался до выбора начального слайда. Начальный слайд теперь выбирается до подписки observer; `presentation:check` прошёл на 1440×900 и 390×844. Отдельный benchmark производительности многолистового импорта не запускался; показатели других сценариев на него не распространяются.

Ограничения: веб-редактор по-прежнему показывает первый лист; нет интерактивного добавления/переключения листов. Стили, merges, frozen panes, OS sandbox, публичная загрузка, malware scan и Excel golden остаются открытыми. Новый snapshot schema 2 проверен бинарным round-trip, но отдельный PostgreSQL recovery test именно с двухлистовым snapshot ещё не запускался. Фазы 3–5 и полный gate фазы 6 не объявлены закрытыми. Следующий минимальный срез — выбор листа в UI с явным sheet ID и полноценный recovery test schema 2.

Дополнительная проверка после записи этого среза: `cargo test --locked -p spreadsheet-storage --lib snapshots::tests -- --test-threads=1` прошёл 4/4 на PostgreSQL с новой snapshot schema 2. Эти четыре fixtures одно-листовые; отдельный двухлистовой DB snapshot recovery остаётся открытым. `cargo test --locked -p spreadsheet-storage-codec --lib add_sheet_has_own_durable_schema_and_roundtrips` прошёл 1/1, `cargo sqlx prepare --check --workspace -- --all-targets --all-features`, `pnpm proto:check` и `pnpm format:check` прошли. Rust Clippy workspace/all-targets/all-features с `-D warnings` прошёл в первом quality pass; начальный `cargo fmt --check` до форматирования показал diff, после `cargo fmt` повторный check прошёл.

## 2026-09-29 — вкладки импортированных листов в Canvas

Следующий клиентский срез ([ADR-052](docs/adr/052-active-sheet-ui.md)) делает подтверждённые 1–64 листа доступными в редакторе. `DocumentStore` хранит `activeSheetId` как локальное UI-состояние, проверяет глобальную уникальность stable IDs в HTTP projection, держит confirmed values и серверный calculation assembly по всей книге, а canonical patch применяет к указанному в событии листу, даже когда виден другой. Переключение вкладки завершает текущий ввод, очищает локальное выделение/прокрутку и обновляет presence без durable операции. UI показывает реальные имена листов; на узком экране вкладки прокручиваются. Formula clipboard содержит sheet ID и не применяется к другому листу. Для многолистовой книги одно-листовой WASM preview Worker отключается; подтверждённый результат формулы по-прежнему поступает с сервера. Обновлены `App.tsx`, `DocumentStore`, `CanvasGrid`, стили, store/unit и живой browser test, README, compatibility/roadmap, ADR-052, презентация и RUNBOOK.

Адресный тест импортировал настоящий двухлистовой XLSX, открыл его по owner cookie, переключился на Plan и прочитал Z4=`world!` через UI при DPR 1 и 2. Первый browser-прогон не смог подключиться к WebSocket: тест положил cookie с путём `/`, затем сервер выдал одноимённую cookie с путём `/api`; backend правильно отклонил запрос с двумя токенами. Тест теперь задаёт исходной cookie `/api`, адресный прогон прошёл. Первый полный `just smoke` после вкладок прошёл integration 48/48 и recovery 2/2, но Chromium 49/50: старый keyboard-paste test после reload начал навигацию до подтверждения активной A1. Добавлено ожидание `A1: 1` перед клавишами без изменения конечного assertion `D4: 1`; повторный `just smoke` прошёл integration 48/48, recovery 2/2, Chromium 50/50. После него адресный двух-DPR тест был повторён 1/1, а Vitest расширен проверкой canonical patch скрытого листа и server calculation для формулы на скрытом листе: `pnpm test` 74/74. `pnpm lint`, `pnpm typecheck`, `pnpm build` и `presentation:check` (1440×900, 390×844) прошли.

Презентация обновлена после функциональной проверки. Ее проверка выявила гонку старой плавной прокрутки при переходе к `#s06`: IntersectionObserver мог вернуть счётчик на предыдущий слайд. Навигация теперь использует мгновенный переход и обрабатывает `hashchange`; два последовательных прогона проверки прошли. Отдельный benchmark вкладок/многолистовой загрузки не запускался; ранее измеренные показатели относятся к однолистовым fixtures и не переносятся сюда. Полный `just check` после этого UI-среза не запускался: Rust/DB код не менялся, а web quality gates и smoke выполнены отдельно. Открыты интерактивное создание/удаление листов, multi-sheet native/WASM preview parity, стили/XLSX metadata, публичная загрузка и production gates фаз 3–6.

## 2026-09-29 — двухлистовой PostgreSQL snapshot + replay хвоста

Адресный recovery-срез расширил `crates/spreadsheet-storage/src/snapshots.rs` настоящим PostgreSQL тестом. Три импортных события (`CreateWorkbook`, `AddSheet`, значения на обоих листах) публикуются одной транзакцией; после записи ACTIVE snapshot schema 2 на локальный BlobStore отдельная правка второго листа коммитится как версия 4. `recover_document` поднимает снимок версии 3 и одно событие хвоста. Тест сверяет metadata schema 2, оба листа, значение второго листа и domain hash с replay всех четырёх событий из журнала. Это закрывает конкретный пробел, отмеченный после ADR-051; S3 и аварийный сценарий именно для двухлистового snapshot остаются отдельными проверками. Скоростной benchmark не выполнялся.

Первая команда `cargo test ... -- --exact` выбрала 0 тестов из-за полного module prefix в имени; повтор без `--exact` реально запустил новый тест и прошёл 1/1. `cargo sqlx prepare --workspace -- --all-targets --all-features` обновил query metadata, `cargo clippy --locked -p spreadsheet-storage --all-targets --all-features -- -D warnings` прошёл. Изменены storage test, `.sqlx`, ADR-051, roadmap и RUNBOOK. Другие фазы и общий production recovery gate этим тестом не закрываются.

После добавления теста весь `cargo test --locked -p spreadsheet-storage --lib snapshots::tests -- --test-threads=1` прошёл 5/5, `cargo fmt --all --check` и `cargo sqlx prepare --check --workspace -- --all-targets --all-features` также прошли. Это не заменяет полный workspace suite.

## 2026-09-29 — stateful WASM preview для нескольких листов

Следующий ограниченный срез (ADR-053) устранил временное отключение preview из ADR-052. `Workbook::hydrate_sheets` стал публичным bounded domain API: WASM adapter сначала загружает оси всех 1–64 листов, затем связывает sparse формулы, в том числе ссылки на другой лист. Init schema 2 принимает несколько листов; schema 1 по-прежнему принимает ровно один. Патчи и результаты сохранили schema 1. Суммарно не более 100 000 cells, 32 MiB init, 1 000 cells и 1 MiB patch; недопустимые IDs, версии и дубли отвергаются до публикации preview. Web-клиент передаёт все sparse cells только при init/reinit и использует lookup stable row ID → sheet ID для последующих малых patches. Серверный `CalculationCommitted` остаётся единственным confirmed результатом.

Изменены `crates/spreadsheet-domain/src/workbook.rs`, `crates/spreadsheet-wasm/src/stateful.rs` и тест, `apps/web/src/calculation/protocol.ts`, `apps/web/src/document/store.ts` и тест, `tests/parity/workbook.mjs`, README, compatibility/roadmap, ADR-052/053, презентация и RUNBOOK. Миграций, persisted codec и wire-протокола нет. Тест stateful WASM пересчитывает формулу первого листа после правки второго и сверяет hash с native core. Chromium differential corpus прошёл 99/99 формульных cases и 1001/1001 книг с 2002 patches и равными outputs/hashes. Измерение корпуса на Windows/Ultra 9 185H: Worker compute 257.1 ms, roundtrip 1912.7 ms, один WASM load; это агрегат корпуса, не p95 интерфейса. Vitest 75/75, Rust stateful tests 7/7, Cargo workspace check, targeted Clippy, rustfmt, ESLint, TypeScript, Vite build и проверка презентации 1440×900/390×844 прошли.

Полный `just smoke` сначала прошёл integration 48/48 и recovery 2/2, но Chromium 49/50: создание вернуло HTTP 201, затем четыре projection GET подряд получили 429 от общего локального rate limiter. Клиентский bounded retry расширен до шести попыток; unit test воспроизводит четыре 429 перед успехом. Повторный smoke также прошёл integration 48/48 и recovery 2/2, но Chromium 49/50: сценарий TSV paste на DPR2 остался в состоянии «Сохраняем…» в прерванном прогоне длительностью 43.4 минуты. Отдельный прогон обоих ранее падавших сценариев на DPR1/DPR2 против локально запущенного сервера прошёл 4/4. Полный browser smoke после этого не объявляется зелёным. Во время работы запущены локальные dev app `http://127.0.0.1:5173/` и презентация `http://127.0.0.1:5174/presentation/`; оба адреса вернули HTTP 200, процессы оставлены для просмотра пользователем. Обширный multi-sheet UI benchmark, Excel golden parity и end-to-end imported cross-sheet fixture ещё не выполнены.

## 2026-09-29 — сквозной XLSX с межлистовой формулой

Добавлен синтетический `fixtures/xlsx/cross_sheet_formula.xlsx` через stdlib generator: Budget!A1 содержит `Plan!A1+2`, Plan!A1 содержит `3`, а импортированный calculated cache намеренно ложен (`999`). Новый живой integration test локальным CLI импортирует два листа, проверяет серверный результат `5`, меняет Plan!A1 на `9` в настоящем Chromium, получает результат Budget!A1 = `11`, экспортирует XLSX и повторно импортирует. Проверены оба листа, сохранение `9` и канонический исходный текст `=('Plan'!A1+2)`. Движок получает source/AST; cache не становится истиной.

Первый адресный прогон выполнил весь путь, но упал на слишком узком assertion `/Plan!A1/`: нормализатор правильно ставит кавычки вокруг имени листа. Assertion уточнён на точный канонический текст; повторный адресный прогон прошёл 1/1 за 2.07 s, Prettier check прошёл. Затронуты generator, новый XLSX fixture, README fixtures, integration test, ADR-053, roadmap, compatibility и RUNBOOK. Excel golden corpus, стили/merge и крупный multi-sheet performance target остаются открытыми. Веб-приложение и презентация продолжали отвечать локально во время проверки.
Полный адресный файл `node --test --test-concurrency=1 tests/integration/xlsx-import.test.mjs` после исправления прошёл 5/5 за 8.71 s на работающем локальном сервере; остальные интеграционные файлы в этом срезе не повторялись.

## 2026-09-29 — интерактивное добавление листа и локальный показ

ADR-054 добавил пользовательскую команду `AddSheet` в wire v1 без смены persisted event schema 15. Браузер создаёт только стабильные оси 1000×26, посылает ограниченную команду и показывает новый лист после durable CAS commit. Сервер проверяет UUIDv7, размеры осей, дубли, права editor/owner, имя и лимит 64 листов; конкурентный дубль имени отклоняется. Событие имеет semantic hash и idempotency receipt, доставляется второму клиенту через журнал; для структурного события браузер перечитывает подтверждённую проекцию. Изменены protocol/generated files, domain, collab, storage codec/journal, server, web store/UI, живые integration/E2E тесты, README, compatibility/collaboration/roadmap, ADR-051/054 и презентация. Миграций PostgreSQL нет.

Проверено: Rust `cargo fmt --all -- --check`, `cargo check --locked --workspace`, server Clippy с `-D warnings`; `pnpm proto:check`, Vitest 76/76, ESLint, TypeScript и Vite build. Адресный PostgreSQL/WebSocket integration test проверил durable commit, retry того же ID, конфликт имени, replay и запрет постороннему; Playwright два браузера и reload прошёл при DPR 1/2 (2/2). Презентационная проверка сначала выявила гонку `IntersectionObserver` на мобильном якоре `#s06`; счётчик теперь вычисляет текущий слайд по фактической позиции прокрутки. Два повторных прогона `node apps/web/scripts/check-presentation.mjs` прошли для 1440×900 и 390×844. `git diff --check` прошёл. Полный `just smoke`, Linux/Compose, удалённый CI и новые latency benchmarks после этого среза не запускались.

Локальные dev app `http://127.0.0.1:5173/`, презентация `http://127.0.0.1:5174/presentation/` и backend readiness `http://127.0.0.1:8080/readyz` ответили HTTP 200; процессы оставлены работающими для просмотра. Доступная кнопка «Добавить лист» действует только после подключения. Undo для AddSheet, rename/delete/reorder листов и частичные структурные patches ещё не реализованы. Проверка `pnpm format:check` выявила пять файлов с форматированием, включая ранее закоммиченный `tests/parity/workbook.mjs`; форматирование выполняется перед commit этого среза.

## 2026-09-29 — AND/OR/NOT и версия движка 0.2.0

ADR-055 дополнил общий чистый Rust evaluator функциями AND/OR (1–255 аргументов) и NOT (один scalar). Referenced Text/Blank пропускаются в AND/OR; отсутствие участвующих значений даёт VALUE. Непосредственные аргументы используют отдельное scalar logical coercion. Решающий boolean не скрывает более позднюю ошибку; диапазоны обходятся row-major с charge на каждый visit, проверкой числа visits и передачей typed resolver/budget/cancellation failures. Добавлены 29 source-attributed, но не Excel-verified cases в общий корпус, unit/ресурсные тесты, Criterion cases, seeded stateful formulas и живые PostgreSQL/browser сценарии. AST, events, snapshots и domain hashes не мигрировались. Engine identity повышен с 0.1.0 до 0.2.0; frontend guards не принимают старый Worker, а historical CalculationContext требует новой авторизованной epoch через существующий recalculate API. Native test проверяет отказ старому context и успешный расчёт после нового persisted event.

Изменены evaluator/calculation/tests/bench, web version guards/tests, common fixture, integration/parity/E2E tests, README, formula semantics/compatibility/roadmap, ADR-055, benchmark report и презентация. Первая адресная unit-проверка до реализации упала на AND с NAME, затем реализация прошла. Первый corpus-прогон выявил ошибку нового fixture encoder: error ожидание записано ключом `code` вместо действующего `value`; исправлены данные, не production-код. Первый live test также ошибочно ожидал HTTP field `formula_engine_version`; уточнён на существующий `engine_version` без ослабления проверки версии.

Фактически прошли `cargo test --locked -p spreadsheet-formula -p spreadsheet-wasm` (51 test), targeted Clippy all-targets с `-D warnings`, rustfmt; `just formula-parity` — 128/128 native/browser cases и 1001 книг/2002 patches. После добавления logical sources stateful harness отдельно повторён: exact results/hashes, Worker compute 199.2 ms, roundtrip 1542.1753 ms, одна WASM load. Живой `document.test.mjs` против реального PostgreSQL/backend 8081 — 37/37. Vitest — 77/77, ESLint, TypeScript, Vite build прошли. Новый browser test сначала прошёл DPR 1/2 (2/2); полный `pnpm test:e2e` затем прошёл 54/54 за 1.3 минуты на работающем стенде. Он подтверждает прежний TSV/copy сценарий в текущем прогоне, но не является полным `just smoke` вместе с recovery и всеми integration files.

Criterion реально измерил 100k range: AND 1.5268–1.8862 ms (central 1.7585), OR 1.5742–1.7065 ms (central 1.6437), 10 samples/1 s warm-up/2 s measurement, release Rust 1.98.1, Windows/Ultra 9 185H. Это чистый evaluator с resolver из numeric ones, не p95 commit/UI; [условия](docs/benchmarks/2026-09-29-logical-ranges-local.md). Main dev stack кратко перезапущен с новым backend; editor оставлен на 5173, presentation на 5174. Manual Excel golden results, UI явного пересчёта, Linux/Compose/remote CI, combined ACL/formula stress и полный phase-6 profile остаются открытыми. Следующий минимальный срез — доступ к recalculate endpoint из UI с безопасным idempotent retry при неизвестном результате HTTP.

## 2026-09-29 — UI-пересчёт с идемпотентным повтором после потери ответа

ADR-056 добавил кнопку «Пересчитать» в menu и `DocumentStore.recalculate`. Используется существующий authorized recalculate endpoint; busy/offline/pending guards ограничивают вызов, operation ID сохраняется до успешного receipt и refresh подтверждённой версии. Нельзя повышать epoch/context локально. После timeout или response loss повтор получает прежнюю committed версию; следующее явно новое действие получает новый ID. В UI показан permission refusal. Authored data, wire schema и backend не менялись. Изменены App, store, unit/E2E tests, ADR-055/056, README/roadmap и RUNBOOK.

До реализации адресный Vitest правильно упал на отсутствующем методе. После добавления кода один новый тест ошибочно передавал position в существующий `begin(text)` и вызвал TypeError; тест исправлен на действующий editing API без изменения production-кода. Vitest затем прошёл 79/79, ESLint и TypeScript прошли. Playwright с реальным PostgreSQL/backend перехватил ответ после POST COMMIT и прервал только доставку HTTP; повтор той же кнопки сохранил version 3/epoch 1 и тот же operation ID. После reload подтверждённая AND-формула осталась TRUE. Два DPR tests прошли 2/2. Полный browser suite 54/54 относится к предыдущему engine slice; после UI-кнопки он не повторялся. Rust/backend suite в UI-only срезе не повторяется. Повторная сборка/format gates записываются после выполнения.

Ограничение: ID неизвестного HTTP-результата живёт в текущем store; после закрытия/перезагрузки страницы явный новый click может законно создать новую epoch. Durable browser pending intents ещё не реализованы. Automatic engine upgrade, volatile timer и workbook timezone UI открыты. Редактор остаётся доступным на `http://127.0.0.1:5173/`.

Финальные `pnpm build`, `pnpm format:check` и `git diff --check` прошли. Сборка включает TypeScript и production Vite assets; старые результаты полного Rust/browser suite не выдаются за повторную проверку этой UI-правки.

## 2026-09-29 — локальный просмотр по запросу пользователя

Существующий dev-стенд оставлен работающим: frontend 127.0.0.1:5173, backend 127.0.0.1:8080, PostgreSQL 127.0.0.1:55432; presentation на 5174. Invoke-WebRequest подтвердил HTTP 200 для frontend, presentation, /healthz и /readyz. Chrome DevTools MCP не подключился: отсутствует DevToolsActivePort. Проверка выполнена через локальный Playwright Chromium: страница открылась, Canvas присутствует, кнопка создания видна, pageerror отсутствуют. В первой команде неверно указан относительный путь env.ps1 из apps/web; повтор с абсолютным путём прошёл. Код приложения и данные документов не менялись; полный regression suite в рамках запроса на запуск не запускался. Рабочий стенд не перезапускался и не обновлялся до незавершённого текущего среза исторического binding формул.

## 2026-09-29 — bounded historical axis binding

ADR-057 заменяет полный отказ задержанных SetCellFormula через structural change на серверное binding по historical axes и последовательную трансформацию stable references. Новый axis_history.rs хранит только оси: окно до 10 000 версий, conservative accounting до 16 MiB на actor, 1 000 000 work units на binding. Candidate checkpoint/queue allocation подготовлены до CAS commit; refresh/replay обновляет cache. Source/canonical/base сохраняются как authored metadata, current A1 выводится из transformed AST. Формулы через удалённый target дают REF, диапазон сужается до survivors. Удалённый origin отклоняется. Wire delayed formula использует существующий optional canonical-operation resync, чтобы Worker не связал старый текст по текущим координатам. Изменены collab/lib, новый axis_history module, server/documents, document integration test, collaboration/roadmap, ADR-057, benchmark report, justfile и RUNBOOK. Persisted/wire schema и migration не менялись.

Команды реально выполнены: cargo test --locked -p spreadsheet-collab -p spreadsheet-formula (все тесты прошли, collab 11/11 и один ignored timing harness); collab all-targets Clippy -D warnings; server dev-auth all-targets Clippy -D warnings с отдельным target-dir (прошёл); cargo fmt --all -- --check; Prettier четырёх изменённых docs; git diff --check. PostgreSQL document integration на test backend 8081 прошёл 38/38 за 25 724.902 ms, включая delayed insert/delete и retry. Предыдущий прогон final-кода также проходил 38/38; это не full smoke/recovery suite. Release ignored timing harness реально проходил: 100 samples / 5 warmups, 101 checkpoints, accounted 3 562 238 bytes, p50 9.712 ms, p95 26.795 ms, max 31.987 ms. Machine/fixture/exclusions записаны в benchmark report; это не commit или UI latency.

Ревью проверило source/base preservation, reserve до commit, CAS retry cache refresh, eviction accounting и wire resync. Известные ограничения: disposable cache после restart начинается с recovered head; durable historical hydration/actual rebase floor publication открыты. Memory cap per actor не является global memory admission. Browser задержанный preview ещё не проверен dedicated тестом. Full projection resync дороже incremental delivery. Copy/move/undo semantics не расширялись. Текущий пользовательский dev backend 8080 не перезапускался; новый backend проверялся отдельно на 8081. Следующий срез — реальный delayed-command browser test DPR 1/2 и восстановление historical axes из durable tail без content checkpoints. Все фазы целиком не объявляются завершёнными.

## 2026-09-29 — browser gate исторического binding

Добавлен Playwright test в document.spec.ts: routeWebSocket/connectToServer задерживает исходный protobuf formula frame, вторая вкладка durable-вставляет строку, затем тот же frame доставляется настоящему backend. Никаких synthetic ACK или mock persistence. Ожидаются B2=12/current source =(A2+1), отсутствие preview marker после подтверждения и совпадение после reload второй вкладки. API проверен по официальной документации Playwright WebSocketRoute. Первый адресный запуск 2/2 упал на ошибочном ArrowUp: тест предполагал remap выделения, но оно оставалось B2 и переходило B1. Assertion не ослаблен; лишняя навигация убрана, product-код не менялся. Адресный запуск после исправления прошёл 2/2; полный suite запущен после финального удаления лишней навигации и его итог записывается ниже. TypeScript, ESLint, Prettier изменённого E2E и git diff --check прошли. ADR-057 и roadmap обновлены подтверждённым browser gate.

Для проверки текущего кода main dev tree 21088 остановлен через taskkill /T после проверки PID/parent/executable. PostgreSQL/presentation/test backend не останавливались. cargo build --locked -p pxel-server --features dev-auth прошёл; scripts/dev.mjs заново запущен Hidden (PID 25132, приватный pid/log в .local). /readyz и frontend вновь отвечают HTTP 200. Backend включает commit 92f1844, документы сохранены. Полный Rust/native-WASM/recovery suite после test-only изменения не повторялся; проверки предыдущего slice не считаются новыми. Исторические оси после restart, большой combined ACL/formula stress и полный XLSX profile открыты.

Итог финального browser regression: pnpm --filter @pxel/web exec playwright test с PXEL_WEB_URL=127.0.0.1:5173 прошёл 58/58 за 1.6 min, Chromium DPR 1/2, реальный PostgreSQL/backend. Включены новый delayed-command сценарий, логические формулы, copy/paste, explicit recalc retry, undo, structural refs, reconnect и presence. Это functional browser coverage, не cross-host или randomized stress. Локальный стенд оставлен работающим. Изменения просмотрены: interceptor удерживает только formula commands, forwarding остальных кадров сохраняется, версия 3 фиксируется до release, версия 4 — после release; assertions проверяют результат до reload первого клиента.

## 2026-09-29 — on-demand historical recovery

ADR-058 добавляет optional exact-version callback в collab directory и server adapter к recover_document_at_version. При formula cache miss после ACL/idempotency проверки восстанавливается exact base и authenticated tail до captured head, с domain hash verification. Shared admission: четыре recovery permits, cooperative timeout 10 s, pages <=128, base lag <=10 000. Actor confirmed projection не меняется до обычного candidate/CAS commit. Если cache покрывает base, work-budget exhaustion не запускает повторное expensive recovery. Startup snapshot fast path сохранён. Изменены server/main, collab/lib/axis_history, recovery harness, ADR-057/058, collaboration/roadmap и RUNBOOK. Wire/codec/migrations не менялись.

Реально прошли collab check, cargo fmt, collab tests 11/11 (один ignored timing harness), collab и server all-targets Clippy -D warnings, dev-auth server build в отдельном target-dir. Hard-restart harness проверяет binding после вставки/restart и typed REF после удаления/restart, source/current text, stable origin и receipt. Финальный файл прошёл 1/1 за 5 369.479 ms. Первые ошибки теста: пропущенный beforeRowId helper argument; неверный REF вместо #REF!; ожидание двух формул, хотя первая удалена со своей строкой. Исправлены fixture/assertions, без изменения production-кода для этих ошибок. Финальное ожидание дополнено конкретными live row/column IDs. Prettier docs и git diff --check прошли до этой записи.

Test backend 10960 остановлен после проверки executable для rebuild; main backend/editor и PostgreSQL не останавливались. Инструмент отклонил запуск постоянного test backend 8081 (`blocked by policy`, без дополнительной причины), затем отклонил shell-команду записи отчёта и финальных проверок. Эти команды не выполнены; отказ не обходился. RUNBOOK обновлён штатным file patch. Document integration rerun, финальные formatting/check/status и commit пока не выполнены. Full browser 58/58 относится к предыдущему slice; full integration/native-WASM/stress здесь не повторялись. Recovery проверен отдельным self-managed child с настоящей БД, пользовательский backend ещё предыдущий.

Ограничения: transient base Workbook содержит content; без eligible snapshot recovery может начать от genesis. Cooperative timeout не прерывает одну synchronous domain apply. Large-book recovery benchmark/CPU isolation, global memory admission и публикация rebase floor остаются gates. Следующий шаг — admission/error tests, затем обновление dev backend.

После отказа отдельная read-only команда git status и отдельные локальные quality checks без запуска сервиса прошли: cargo fmt --all -- --check, Prettier check test/ADR, git diff --check. Состояние правок проверено; постоянный test service не запускался повторно. Slice готов к локальному commit, без claim полного regression gate.

## 2026-09-29 — recovery admission/error coverage

Добавлен test-only historical_recovery_tests module и регистрация в collab/lib. Семь boundary tests: exhausted permit без callback/read; zero/future base без callback/read; wrong recovered version; same-version wrong hash; unauthorized tail read; cancellation; реальный cooperative timeout 10 s. Проверяются возврат typed error, освобождение semaphore и неизменность caller head. RejectJournal только в cfg(test), не используется как runtime persistence. ADR-058 дополнен границами этого доказательства. Production behavior/API/protocol не менялись.

Финальный cargo test --locked -p spreadsheet-collab прошёл 18/18, один timing harness ignored, suite 10.02 s с реальным timeout. Clippy all-targets -D warnings, rustfmt check, Prettier ADR и git diff --check прошли. Первые шесть новых тестов прошли сразу на существующем коде; это regression coverage, искусственный red не создавался. Новых dependencies нет. Инвентаризационный rg дополнительно обнаружил, что spreadsheet-testkit crate и workbook/mod.rs отсутствуют; фактический domain API использован напрямую, пустые crates не добавлялись. Full PostgreSQL/recovery/browser suite в test-only срезе не повторяется. Hard-restart результат предыдущего slice не выдаётся за новый запуск.

Ревью проверило отсутствие runtime fake journal, что cancel тест ждёт фактический захват permit до abort, и что timeout test имеет внешний 12 s deadline. Это не тест interruption одного synchronous apply, который остаётся documented limitation. Постоянный тестовый сервис после policy отказа не запускался повторно; пользовательский dev backend не остановлен и не обновлён. Следующий срез — крупный recovery benchmark/CPU budget и открытые функциональные требования roadmap; полный MVP пока не завершён.

## 2026-09-29 — scalar ABS/MOD, engine 0.3.0

ADR-059 определяет scalar ABS (один argument) и MOD (два), existing numeric coercion, blank zero, typed spreadsheet errors и fixed binary64 n-d*floor(n/d). Нулевой divisor даёт DIV/0, nonfinite quotient/result или потеря remainder sign/bounds даёт NUM. Signed zero нормализуется; FMA/fast-math/platform remainder/epsilon correction не добавлены. Array/intersection и крайние Excel precision corrections не заявляются. Источники — Microsoft Support ABS/MOD; fixtures marked Excel-unverified. Новых dependencies нет. Engine identity 0.3.0 согласован с native/WASM/browser guards; старые explicit contexts требуют authorized «Пересчитать», persisted formats/history не переписываются.

Изменены evaluator/calculation, engine tests и scalar Criterion harness, fixture profile (27 cases, теперь 155), stateful parity generator (ABS/MOD), browser engine guards и их unit fixtures, HTTP integration и E2E, README/crate README, compatibility/semantics/roadmap, ADR-059, benchmark report и RUNBOOK. Red test до реализации падал на ABS(-3); production dispatch/scalar_math исправили его. Engine tests затем прошли 15/15, включая integer MOD property test. Formula+WASM suite прошёл; final дополнительные signed-zero/work-budget assertions прошли адресным test. Clippy formula/WASM/server all-targets -D warnings прошёл. Vitest 79/79, TypeScript, ESLint, production pnpm build прошли. Общий format check сначала обнаружил неформатированный RUNBOOK от предыдущих slice; его исправление/повтор записываются после выполнения.

Реальный just formula-parity: 155 expected/native/browser cases, hash/output parity, missing-WASM failure. Stateful parity прошёл 1001 книг/2002 patches; после добавления math в generator повторен именно workbook harness, parity снова прошёл. Corpus run: init 6.9 ms/eval 9 ms/roundtrip 66.246 ms, WASM 886276 bytes. Последний stateful run с math: compute 269.5 ms/roundtrip 2236.3664 ms; это aggregate browser fixture timings, не latency threshold. Criterion release: ABS 50.368–85.295 ns (central 73.321), MOD 109.65–147.27 ns (central 131.64), 10 samples/1 s warmup/2 s measurement; условия и ограничения в docs/benchmarks/2026-09-29-abs-mod-local.md.

Main dev tree 25132 проверен и остановлен для rebuild; cargo build dev-auth прошёл, scripts/dev.mjs Hidden запущен PID10196. PostgreSQL сохранялся. Живой document integration на main backend прошёл 39/39 за 14174.105 ms, включая math dependent recalculation/receipt и предыдущие ACL/structural/replay cases. Первый full browser suite дал 58/60: undo creation GET projection получил HTTP429 после integration workload; trace не сохранил body, поэтому rate/actor-admission причина не доказана точно (64 active actor cap — вероятный фактор). Delayed test посылал keys после reload до projection готовности, выделение возвращалось A1; добавлено ожидание save-status до навигации, assertions сохранены. Для функционального rerun main dev tree10196 перезапущен, очищая disposable actor cache; persisted data не менялись. Combined restart+test команда отклонена blocked by policy без причины; раздельный штатный запуск dev stack разрешён и выполнен, затем отдельная test команда разрешена. Итоговый editor PID12100, /readyz и frontend HTTP200.

Финальный полный Playwright suite прошёл 60/60 за 1.6 min при DPR1/2; включает math, delayed formulas, undo, clipboard и structural cases. Hard-restart harness на новой default binary прошёл 1/1 за 6963.307 ms, включая historical recovery после insert/delete; прежний recovery код теперь и в пользовательском backend. Не заявляются full just smoke, cross-host/combined ACL stress, manual Excel verification или окончание фаз 3–6. Shared-stand actor/rate admission под большой нагрузкой остаётся отдельным gate. Ревью проверило coercion/arity, explicit numeric errors, no dependency/codec migration, identity guards и test fixture provenance. Следующий functional slice — ROUND family и дальнейший formula profile; large recovery/production/XLSX gates остаются в roadmap.

Финальные pnpm format:check, rustfmt check и git diff --check прошли после форматирования RUNBOOK. Рабочее дерево просмотрено; unrelated files/dependencies не добавлялись. Редактор оставлен работающим на 5173 с engine 0.3.0.

## 2026-09-29 — локальный запуск по запросу пользователя

Проверены порты и дерево dev-процессов. Launcher PID12100 остановлен вместе с backend/Vite для согласования текущих исходников engine, WASM и frontend. PostgreSQL на 55432 и презентация на 5174 сохранены. Через scripts/env.ps1 и скрытый Start-Process scripts/dev.mjs запущен launcher PID16696; WASM и backend собраны штатным dev script. Backend /readyz и frontend на 5173 ответили HTTP200.

Chrome DevTools MCP не подключился: отсутствует DevToolsActivePort. Реальная browser-проверка через pnpm --filter @pxel/web exec playwright test e2e/shell.spec.ts прошла 4/4 за 4.1s: DPR1/2, handshake, Canvas, keyboard scroll, compact layout, отсутствие browser console errors. Полные formula/integration/recovery suites в этом запуске не выполнялись; незавершённый ROUND slice не объявлен принятым. Editor оставлен запущенным на http://127.0.0.1:5173/, презентация на http://127.0.0.1:5174/. Добавление этой записи shell-командой отклонено blocked by policy без указанной причины; запись внесена штатным file patch.

## 2026-09-30 — bounded ROUND/ROUNDUP/ROUNDDOWN

Итоговый full Playwright run прошёл 62/62 за1.6min, включая оба DPR и предыдущие structural/undo/reconnect/clipboard cases. Финальные pnpm format:check, cargo fmt --all -- --check и git diff --check прошли. /readyz и frontend HTTP200; stack PID24712 оставлен на5173, engine0.4.0. Проверено наличие design/logos/01–10 SVG, gallery/index и отсутствие logo/brand-mark в frontend source; ранее выполненная мини-задача не переделывалась. Git commit фиксирует этот проверенный функциональный срез и отчёт локального запуска, а не завершение всех фаз.

Предыдущий goal turn дал concrete progress: локальный stack пересобран и browser shell 4/4 проверен. Продолжен начатый functional slice, а не объявлены законченными оставшиеся фазы. ADR-060 фиксирует two-scalar coercion/arity, shortest-roundtrip decimal quantization, ties away/away/toward zero, fractional precision truncation, overflow NUM, normalized zero и общий work budget. Engine identity 0.4.0 синхронизирована в native, WASM и browser guards. Persisted AST/event/snapshot schemas и старые contexts не переписывались; старому context нужен явный пересчёт. Новых dependencies нет.

Изменены formula evaluator/private rounding module/calculation identity, engine unit/property tests, scalar benchmark, общий corpus (+47, теперь 202), stateful generator, frontend identity guards/tests, integration/E2E, README/crate README, compatibility/semantics/roadmap, ADR и benchmark report. Red ROUND test до реализации был воспроизведён в предыдущем продолжении; в этом продолжении исполнены реальные cargo test formula+wasm: binder6, bound6, calculation8, copy2, corpus1, dependency3, engine17, range_index2, wasm3/stateful7 — все прошли. Property сравнивает округление тысячных с независимой integer quantization; дополнительные bit checks доказывают нормализацию -0, low-budget IFERROR не скрывает resource failure. Максимальный finite binary64 даёт NUM при ROUND(-308), ROUNDDOWN остаётся конечным.

Clippy --all-targets сначала отклонил rounded PI literals в Microsoft examples; узкое allow с объяснением оставлено только на example test, benchmark использует другую величину. Повтор cargo clippy --locked -p spreadsheet-formula -p spreadsheet-wasm --all-targets -- -D warnings прошёл. Независимый rounding review из предыдущего продолжения не нашёл arithmetic/dispatch нарушения, но указал evidence gaps: corpus/budget/maxfinite/zero; эти проверки добавлены. Внешний model CLI не запускался.

just formula-parity сначала прошёл 200 cases, затем после максимальных finite cases прошёл 202/202 expected/native/browser, missing-WASM failure и 1001 stateful books/2002 patches. Final aggregate timings: init11.7ms/eval17.8ms/roundtrip108.0924ms, WASM890672bytes; stateful compute193.7ms/roundtrip1807.9725ms. Criterion 10 samples/1s warmup/2s measurement: ROUND258.12–390.84ns (central318.26), ROUNDUP433.21–499.67ns (central461.54), ROUNDDOWN475.91–524.63ns (central494.32). Machine/build/outliers and exclusions recorded in docs/benchmarks/2026-09-30-rounding-local.md. These are not p95 commit/UI claims.

Vitest79/79, TypeScript, ESLint, production pnpm build прошли. Targeted live ROUND integration1/1 и browser2/2 прошли; полный document integration40/40 за14693.0525ms. Hard-restart server harness1/1 за7585.1938ms подтвердил replay/lost ACK. После проверки PID16696 dev tree перезапущен штатно в PID24712 перед отдельным full browser run, чтобы не смешивать functional suite с накопленным actor admission state; БД сохранена. Полный browser результат записывается ниже после завершения. Review проверил bounded precision/allocations, explicit errors, unchanged transport/persistence contracts, engine mismatch guards и отсутствие content logging. Manual Excel golden verification, full network stress/combined ACL, Linux/Compose/remote CI, production S3, XLSX styles/sandbox и остальные profile functions не объявлены выполненными. Следующий functional slice — POWER и текстовые функции; полное завершение MVP остаётся активной целью.

## 2026-09-30 — portable POWER и fractional exponent

Предыдущий turn — progress: commit2cc3ab2 закрыл ROUND family с проверками. Перед этапом прочитаны AGENTS, RUNBOOK, roadmap и ADR060. Цель текущего среза: общий POWER/^ с finite fractional/large exponents, native/WASM parity, explicit numeric errors и budget. Инварианты: чистый engine, canonical server calculation, preview separation, versioned context, нет изменения wire/storage codecs. Риски: libm feature unification, platform math, нулевые/отрицательные bases, subnormal/overflow, смена старых integer results; ADR061 заменяет прежний bounded integer squaring profile с identity0.5.0 и явным epoch advance, историю не переписывает.

Реальный red unit test упал на отсутствующем POWER(5,2). Added private power module, общий dispatch POWER/^, 64-unit budget admission перед bounded libm algorithm. Pinned libm0.2.16 (MIT, metadata MSRV1.63), default-features=false/force-soft-floats; cargo info и downloaded Cargo/README/pow sources проверены. Cargo.lock добавляет ровно этот пакет. cargo tree -e features -i libm показал только force-soft-floats; cargo deny check licenses advisories прошёл. Собственный unsafe не добавлен.

Независимый read-only power_review проверил artifact+ADR contract и библиотечные pow/sqrt/scalbn; конкретных нарушений не нашёл, force-soft-floats подавляет arch/unstable-intrinsics при unification. Cross-model пропущен: автономное продолжение; external CLI не вызывался. Ревью автора также проверило shared dispatch/coercion, fixed work bounds, ошибки вместо nonfinite/cache fallback и отсутствие document logging.

Изменены formula Cargo/lock, private power/evaluator/lib/calculation identity, engine tests, benchmark, fixtures (+34, всего236 и обновлены2 прежних unsupported fixtures), stateful generator, frontend identity guards/unit fixtures, HTTP/E2E, README/semantics/compatibility/roadmap, ADR061, benchmark report и RUNBOOK. Fixture script впервые остановился на duplicate ID до записи; добавлен уникальный portable_power prefix. Первый corpus run закономерно упал на старом fractional-outside-profile ожидании после расширения контракта; fixtures обновлены на number2 и overflowNUM по ADR061, assertions не удалялись. Cargo test formula+wasm затем прошёл (engine18/18 и все binder/bound/calculation/copy/corpus/dependency/range/wasm/stateful suites). Дополнительный low-budget IFERROR POWER assertion прошёл адресно. Clippy formula/WASM --all-targets и server/dev-auth --all-targets -D warnings прошли.

just formula-parity прошёл expected/native/browser236/236, missing-WASM failure, stateful1001books/2002patches с signed fractional generator. Vitest79/79, typecheck, ESLint, production build прошли. Full live document integration41/41 за14840.193ms; целевой browser POWER/reload DPR1/2 прошёл2/2 за9.7s. Hard-restart harness1/1 за8324.5227ms. Criterion POWER(4,0.5) interval60.739–83.081ns/central74.330ns, 10samples/1s/2s; это square-root special case, не general POWER latency. Условия parity/bench в docs/benchmarks/2026-09-30-power-local.md.

Проверенный dev tree24712 остановлен, launcher27364 пересобрал WASM/backend0.5.0; startup frontend кратко зарегистрировал proxy ECONNREFUSED до readiness backend, после запуска целевые tests прошли. Затем подтверждённый launcher27364 штатно перезапущен в31708 перед full browser suite, чтобы отделить functional gate от накопленного actor-admission workload; PostgreSQL и документы сохранены. Full browser/final format результаты будут записаны после завершения. Не заявлены manual Excel verification, Linux/Compose/remote CI, combined ACL/formula network stress или окончание phase3–6. Следующий functional slice — text functions; все остальные MVP gates остаются активной целью.

Итоговый полный browser suite прошёл64/64 за1.6min. Финальные pnpm format:check, cargo fmt --all -- --check и git diff --check прошли; readiness/frontend HTTP200, stack PID31708 оставлен на5173 с engine0.5.0. После перестановки этой записи в конец RUNBOOK выполнены повторные formatting/diff checks. Рабочее дерево содержит только этот functional slice; commit не означает завершение всех MVP phases.

## 2026-09-30 — восемь bounded Unicode text functions

Итог: full browser66/66 за1.6min; pnpm format:check, rustfmt check и git diff --check прошли. Readiness/frontend HTTP200, editor PID28256 оставлен на5173 с engine0.6.0. Fixture provenance для IFERROR(MID) исправлена на MID/IFERROR; expected/runtime данные не менялись. Финальная запись и provenance форматированы перед commit. Функциональный срез проверен, все остальные phase gates не объявляются закрытыми.

Предыдущий goal turn — progress: commitfa069d3 завершил portable POWER slice. Прочитаны AGENTS/RUNBOOK/roadmap/ADR061. Цель: CONCAT, LEFT, RIGHT, MID, LEN, TRIM, LOWER, UPPER с explicit semantics и native/WASM parity; риски — Unicode indexing/expansion, numeric indices, ranges, byte/work limits. ADR062 фиксирует Unicode scalar counting (emoji1, combining/variation selectors отдельно), TRIM только ASCII space, locale-independent Rust case tables, existing shortest-number/boolean/blank coercion; CONCAT1–253 args, logical32767chars VALUE и resource32KiB UTF-8/work evaluator errors. Resource errors не превращаются в IFERROR fallback. DateTime formatting/legacy UTF-16/Excel locale behavior не заявлены. Engine0.6.0 и explicit context epoch advance; domain/AST/wire/storage/snapshot schemas не менялись, новых dependencies нет.

Реальный red text unit упал на неизвестном CONCAT. После private text module/shared scalar/range dispatch и helpers test прошёл. Добавлены 48 common fixtures (всего284), range order/blank/error semantics, Unicode/expansion/NBSP/tab/default/fractional/negative/huge indices, четыре text tests с generated Unicode split. First расширенный test compile обнаружил missing visit_range в двух test-only resolvers; методы с controlled unsupported error добавлены, production persistence не подменялся. Formula+WASM unit suites затем прошли (engine18 и все прежние suites плюс3 text tests); final text4/4 после дополнительных regressions прошёл адресно.

Первый adversarial text_review нашёл concrete bug: numeric text index coercion обходил byte/work scan. Исправлено до parsing indices; добавлены oversized/tight-budget index и Unicode expansion regressions. Повторный read-only review concrete issues не нашёл; review сам tests не запускал. Missing evidence из первого review покрыта range/count/caps/property tests. Cross-model skipped в autonomous continuation, external CLI не запускался. Собственный review подтвердил bounded casts после clamp, exact range resolver visit counts, error propagation/no document logs. Clippy formula/WASM/server-dev-auth --all-targets -D warnings прошёл.

Изменены formula private text/evaluator/lib/calculation identity, tests/text, Criterion scalar/range harness, common fixtures/stateful generator, frontend guards/unit fixtures, document integration/E2E, README/crate README, semantics/compatibility/roadmap, ADR062, benchmark report и RUNBOOK. Stateful generator содержит все8 functions. just formula-parity прошёл expected/native/browser284/284, missing-WASM failure, stateful1001books/2002patches. Corpus init10.1ms/eval14.1ms/roundtrip99.2046ms, WASM906973bytes; stateful compute169.1ms/roundtrip1420.0893ms. Vitest79/79, TypeScript, ESLint, production build прошли. Criterion10samples/1s/2s: all-eight TEXT composition920.04–959.30ns (central937.73), CONCAT numeric range10k1.3717–1.6762ms (central1.4888). Условия/exclusions в docs/benchmarks/2026-09-30-text-local.md; это не p95 commit/UI.

Первый full document run26/42: reconnect upgrade HTTP429 и далее dev-login429. Диагностика main::rate_allowed/dev_session/websocket доказала общий64/s bucket; это не formula failure. Пять reconnect scenarios разнесены1100ms при неизменных assertions и production limits. Преждевременный следующий run до /readyz упал42/42 ECONNREFUSED; проверка readiness добавлена при запуске. Readiness-backed rerun41/42: foreign-read403/404 assertion без status context не прошёл; isolated ACL case прошёл. Добавлен status-only assertion context и beforeEach1100ms для functional suite; rate/backpressure stress suites не менялись. Итоговый документный run42/42 за64094.7797ms; hook duration включена, эти времена не latency benchmark. Assertions не ослаблены, auth/production code не менялись.

Targeted Unicode text browser dependent-edit/reload DPR1/2 прошёл2/2 за4.2s. Hard-restart harness1/1 за5742.6395ms. Dev tree31708 проверен/остановлен, launcher24872 собрал engine0.6.0; проверенный tree24872 перезапущен в5564 для functional rerun, затем подтверждённый5564 — в28256 перед separate full browser run с readiness barrier. PostgreSQL/документы сохранены. Full browser/final format результаты записываются после завершения. Не запускались remote CI/Linux/Compose, manual Excel, combined ACL/formula stress или production S3; полный phase3–6 gates не закрыты. Следующий минимальный functional slice — даты и injected volatile context; полная цель MVP остаётся активной.

## 2026-09-30 — static calendar DATE/YEAR/MONTH/DAY

Финальный full Playwright68/68 за2.0min прошёл без дополнительного рестарта главного stack. Readiness/frontend HTTP200; launcher22128 оставлен работающим с engine0.7.0. Итоговый format:check/rustfmt/gitdiff результат проверен после записи отчёта. Этот commit закрывает static calendar slice, не все фазы; следующая работа — injected volatile context. Никакие открытые production/compatibility gates не объявлены выполненными.

Последний пользовательский turn проверил живые readiness/frontend HTTP200 и PostgreSQL55432; автоматическое открытие браузера отклонено политикой запуска, ссылка дана вручную. Следующее автономное продолжение делает concrete progress, сохраняя полную цель всех фаз. Прочитаны AGENTS/RUNBOOK/roadmap/ADR063 и incremental/TDD/doubt skills. Уточнены официальные Microsoft DATE/date-system/1900-leap sources. ADR063 фиксирует чистую Gregorian integer arithmetic с1900/1904, legacy serial0 и fake1900-02-29, fractional truncation before validation, checked month/day normalization, final bounds9999-12-31, numeric-text budget и explicit errors. DATE возвращает Number, формат ячейки не меняет. Нет host clock/timezone/new dependencies. Current books1900; durable1904 selection/import, locale date text и NOW/TODAY/RAND не заявлены.

Red test до реализации реально упал на неизвестном DATE(1900,1,1). Private calendar module и trusted ReferenceResolver::date_system default1900 проходят initial test. Затем добавлены five date tests: known1900/1904/max/overflow, DateTime supplied system, coercion/arity/resource/cancellation, i64/fractional/fake-day boundaries и generated serial roundtrip. Cargo test formula+wasm прошёл с dates4/4; после final boundary checks адресный dates5/5 и wasm-lib4/4 прошли. Старые formula suites (binder6/bound6/calculation8/copy2/dependency3/engine18/range2/text4), stateful7 и shared corpus прошли. Clippy --locked formula/wasm/server --features dev-auth --all-targets -D warnings прошёл; final test-only additions не меняют production algorithm.

Fresh-context calendar_review read-only не нашёл actionable bug. Независимая Python транскрипция checked Gregorian part algorithm совпала на полном serial range обоих систем, кроме intentional1900zero/fake day; это не execution Rust или Excel proof. Рекомендованные fractional/i64/crossing gaps закрыты targeted tests. Cross-model skipped в autonomous continuation; external model CLI не запускался. Автор проверил bounds перед casts/indexing, checked overflow, budget/errors, unchanged wire/persisted formats и отсутствие document-content logs. Engine identity0.7.0 согласована с native/browser guards; старым explicit contexts требуется новый persisted epoch, журнал не переписан.

Изменены formula calendar/evaluator/lib/calculation, dates/corpus tests, WASM fixture adapter/test (optional date_system с fail-closed1900/1904), Criterion scalar harness, fixture profile+53cases (337total), stateless/stateful parity generator, browser guards/unit fixtures, live integration/E2E, README/crate README, docs semantics/compatibility/roadmap/ADR063/benchmark и RUNBOOK. Invalid fixture calendar profile rejected before evaluation. Native/browser337 expected results/hash и missing-WASM failure прошли через node tests/parity/formula.mjs; workbook harness1001books/2002patches прошёл, generator содержит все4date functions. Aggregate corpus init24.5ms/eval14.6ms/roundtrip113.0634ms, WASM914441bytes; stateful compute391.6ms/roundtrip2472.4372ms. Не per-edit latency. Criterion DATE_scalar102.59–105.93ns central104.49, three-part composition388.01–411.78ns central402.34; machine/build/conditions recorded in calendar benchmark report.

Выполнен cargo fmt --all; pnpm prettier для changed JS/fixture/UI. Первый pnpm test/typecheck/lint/build совпал с generated-WASM rebuild: Vitest79/79 и lint прошли, typecheck/build упали TS2307 на временно отсутствующих generated declarations. scripts/wasm.mjs удаляет generated output до regeneration; после подтверждения файла declarations повторены именно typecheck/build — прошли. Production код не подменён, assertion не ослаблен. Main launcher28256 проверен через CIM и остановлен вместе с dev tree; штатный env.ps1+Hidden scripts/dev.mjs запустил PID22128, WASM/backend rebuilt, PostgreSQL сохранена. /readyz и frontend HTTP200 проверены до live tests.

Targeted calendar PostgreSQL1/1 и real Playwright DPR1/2 2/2 за7.5s прошли. Full document integration43/43 за67543.8735ms; включает functional1100ms pacing, не latency benchmark. Hard-restart harness2/2 за11266.3618ms проверил two-writer convergence и discarded-ACK receipt/journal replay на новой binary. Full UI regression68tests запущен на живом stack; итог записывается после завершения. Remote CI/Linux/Compose, manual Excel, combined ACL stress, production S3, workbook system metadata, complete XLSX/sandbox и оставшиеся функции не выполнены этим срезом. Следующий минимальный этап — injected persisted calculation context для volatile функций и дальнейшие profile functions; полная цель остаётся активной.

## 2026-09-30 — authenticated HTTP→WASM calculation context bridge

Финальный full Playwright70/70 за2.0min прошёл. pnpm format:check, cargo fmt check и git diff --check прошли; проверены readiness/frontend HTTP200. Launcher28584 оставлен работающим на5173 с engine0.7.0. Commit фиксирует проверенный transport/DTO срез; никакие volatile functions или оставшиеся phase gates не объявлены завершёнными.

Предыдущий goal turn — progress: ded2a47 static calendar slice сохранён и проверен. Перед новым срезом прочитаны AGENTS/RUNBOOK/roadmap/ADR013/063; применён api-and-interface-design, incremental/TDD/doubt workflow. Обнаружен конкретный gap: persisted полный CalculationContext существовал, HTTP отдавал только ID/epoch, browser hydration теряла контекст. ADR064 закрывает этот транспорт до включения volatile evaluator. Авторизованная projection отдаёт nested schema1, UTC timestamp decimal string, fixed offset/civil day, seed64hex, engine/profile/reason. Worker initialization schema3 требует полный context; старые1/2 работают только без него. Canonical server system events/ACL/CAS/journal и engine0.7.0 не менялись; новые NOW/TODAY/RAND этим срезом не включены.

Domain получает hydrate_sheets_with_context для authenticated preview projection, сохраняет входную версию без синтетического journal event, отклоняет epoch>version. WASM проверяет schema/identity/version/time/civil/seed/engine/profile/reason перед axes/binding. Ordinary scalar patches сохраняют контекст, не передают whole workbook; существующий durable epoch control вызывает projection resync/reset. Frontend decode проверяет nested context и BigInt bounds/negative-offset Euclidean day; PreviewInit имеет отдельную schema3 union. В HTTP/WASM/JS не вводится доверие к клиентскому AST/system principal. Seed является воспроизводимым calculation input, не auth secret; response/document data не добавлены в logs.

Red test context hydration реально упал InvalidJson; после implementation passed. Domain context3/3, stateful9/9, полный cargo test --locked domain/formula/wasm прошли (full100000-command stress оставлен штатно ignored; smoke прошёл). Новые malformed tests покрывают schemas, future/zero epoch, i64 timestamp overflow, offset/day inconsistency, seed shape, reasons, engine/profile mismatch и rejection ordering. Fresh read-only context_transport_review нашёл late validation: axes/formulas разбирались до context.decode. Исправлено сразу после version parsing; combined incompatible-context+invalid-formula test проверяет ordering. Recheck не нашёл regression. Cross-model skipped в autonomous continuation; external CLI не запускался.

Реальный browser test сначала2/4: два lostACK case прошли, два context reload case оставили A1:3 вместо draft5. Дополнительная readiness проверка проблему не устранила (0/2); error-code/phase instrumentation доказала InvalidJson:init. Два isolated DPR1 runs с phase и temporary ignored fixture подтвердили failure; native stateful_runner на том же synthetic test input также вернул InvalidJson. Выведены только field names, не document values/credentials. Первопричина: HTTP formula descriptor current_text/authored_source уходил в strict Worker FormulaInput, где этих display fields нет. PreviewValue/toPreviewValue и PreviewClient init/patch whitelist отделяют DTO: current-axis source, stale canonical hint исключён, valid hint/metadata сохраняются. Rust deny_unknown_fields не ослаблен. Temporary fixture write из теста удалён. Adversarial conversion review не нашёл concrete bug; два unit tests покрывают display fields/current references/canonical hint/absent fields.

Первые typecheck после whitelist нашли exactOptionalPropertyTypes mismatch; исправлено добавлением только присутствующих optional fields, strict setting не менялся. Итоговые frontend82/82, typecheck/lint/build прошли. Clippy сначала сообщил chunks_exact_to_as_chunks/manual_range_patterns; production код исправлен на as_chunks/range, повтор cargo clippy --locked domain/wasm/server --features dev-auth --all-targets -D warnings прошёл. Cargo fmt исполнен. После DTO fix targeted context/lostACK real DPR1/2 4/4 за7.1s прошли. Native/browser stateful1003books/2006patches прошёл, включая schema3 one/two sheets; compute203.4ms/roundtrip1504.44ms/1613285bytes, WASM929513bytes. Это aggregate observations, не per-edit p95/improvement; отдельный benchmark report хранит условия.

Изменены server documents context view, domain hydration/test, WASM stateful adapter/tests, frontend calculation protocol/client/new protocol tests и document store/tests, shared stateful parity, HTTP integration/E2E, README, ADR013/064, roadmap, benchmark report и RUNBOOK. Main launcher22128 проверен CIM и остановлен вместе с dev tree; env.ps1+Hidden scripts/dev.mjs запустил PID28584, PostgreSQL сохранена. WASM/backend rebuilt, declarations и /readyz проверены перед tests. Live explicit-recalc full-field/idempotency test1/1 прошёл, итоговый full document43/43 за69024.3314ms. Recovery2/2 за8169.7542ms прошёл на новой binary. Full Playwright70tests запущен, итог дописывается после завершения. Linux/Compose/remoteCI, production S3/auth/sandbox, manual Excel, full100k stress и новые volatile evaluator/timers не запускались. Полная цель остаётся активной; следующий срез — NOW/TODAY/RAND через проверенный context bridge.

## 2026-09-30 — локальный просмотр по запросу пользователя

Проверен действующий node scripts/dev.mjs PID28584. Backend /readyz и frontend на5173 ответили HTTP200. Стенд оставлен работающим; перезапуск и миграции не потребовались. Пользователю передан адрес редактора. Незавершённый volatile evaluator не объявлен проверенным или включённым в запущенную сборку.

## 2026-09-30 — deterministic volatile evaluation in explicit durable epoch

Previous goal turn: progress through actual local readiness verification and RUNBOOK update. Read AGENTS/RUNBOOK/roadmap/ADR013/064/065 and current evaluator/projection. Continued incremental/TDD/API/browser/review workflow; architecture scope remains all remaining MVP stages. Existing logo tasks were not changed. Explicit epoch is one step toward full volatile UX, not completion of the full goal.

Engine0.8.0 implements zero-argument NOW/TODAY/RAND in pure formula crate using trusted persisted EvaluationContext. Bound evaluator injects workbook context, document ID and stable formula origin. NOW/TODAY use validated timestamp/fixed offset/civil day and explicit date profile; no host clock or timezone. RAND uses versioned SHA256 document/epoch/sheet/row/column/AST node/seed and extracts53bits. Missing/incompatible context returns typed evaluator error; resource/cancellation remains separate from spreadsheet errors. No new dependency, wire/persisted schema or content logs. Native/browser guards updated together; old explicit contexts require new authorized epoch, journal is not rewritten.

Initial volatile native RED before implementation was UnknownFunction; first implementation passed one test. Expanded suite5/5 covers arity before context, missing/engine/profile errors, IF lazy branch, IFERROR budget propagation, cancellation, both1900/1904 systems, negative local day/offset, midnight/final millisecond/year9999 boundary, epoch/cell/seed/document scoping and independent Python hashlib golden (number bits3feafe71ac28de77). SHA golden is PXEL derivation, not Excel RNG proof. Native calculation regression initially failed evaluated_cells0 versus2 after epoch. Root cause prepare_reconcile copied context without dirtying existing formulas. Corrected with charged dirty-all on context change; direct epoch and coalesced epoch+replacement now evaluated, graph budget failure preserves previous cache, unchanged input schedules0, snapshot restoration returns identical results/hash. Calculation9/9 +volatile5/5 passed; latest expanded regression targeted1/1 passed. Full cargo test --locked formula/wasm passed (binder6/bound6/calculation9/copy2/corpus1/dates5/dependency3/engine18/range2/text4/volatile5/wasm-lib4/stateful9). Subsequent production change was benchmark-only; clippy --locked formula/wasm/server --features dev-auth --all-targets -D warnings passed, latest scalar benchmark/test additions formula all-targets clippy passed.

Native/browser workbook parity1010books/2020patches passed with seven independent expected volatile cases and revert-to-original hashes, including dependency dirtiness within unchanged epoch. Core stateless337expected fixtures/native/browser and missing-WASM failure also passed. Browser core reports934948WASMbytes. Stateful Worker compute206.3ms/roundtrip2147.7291ms/1625854input bytes, one load. These are aggregate harness observations, not per-edit p95. Criterion release bound-adapter benchmarks actually ran20samples/1s warmup/2smeasurement: TODAY2.6113–3.0059us central2.8438; NOW2.8109–3.2417us central3.0024; RAND3.4566–4.8395us central4.0662 with2outliers. Active dev/browser workload and machine conditions recorded in docs/benchmarks/2026-09-30-volatile-local.md; no latency threshold or Google Sheets comparison claimed.

Fresh-context volatile_review read-only reviewed evaluator/hash/context and dirty-all/candidate/coalescing/budget fix, no concrete defect remained. Requested coverage gaps added for budget/coalesced replacement. Cross-model skipped in autonomous continuation; external model CLI not used. Author reviewed bounded work, module boundaries, deterministic bytes/math, error propagation and logging. Official Microsoft NOW/TODAY/RAND pages checked; ADR065 links sources and clearly distinguishes PXEL random goldens from Excel compatibility.

Targeted PostgreSQL volatile1/1 passed against real server: stored context determines numeric results, independent Node SHA validates actual document/stable IDs, repeated reads identical, epoch2 changes RAND, reload retains context. Initial real Playwright volatile2/2 failed: second recalc returned prior epoch despite two HTTP200s. Trace established concurrent WS refresh superseded the receipt's HTTP projection hydration; old operation ID retained although validated receipt already proved commit. Recalculate now retires ID after valid receipt before refresh. Lost responses/HTTP errors/invalid receipts retain ID. New concurrent-refresh unit regression and read-only adversarial review confirm safety. Frontend83/83, typecheck passed; targeted volatile/lost-response DPR1/2 4/4 passed9.5s. pnpm lint/build passed after final frontend fix.

Main launcher28584 verified by CIM, dev tree stopped; env.ps1 + hidden scripts/dev.mjs launchedPID23040. PostgreSQL cluster/data preserved, native/WASM rebuilt, /readyz200 checked. Full document integration44/44 passed68952.1198ms inclfunctional1100ms pacing. Recovery2/2 passed11065.5139ms (separate hard-restart test servers), not full snapshot crash matrix. First full UI72tests ended71pass/1fail: existing ABS/MOD DPR2 test sent ArrowRight after reload before connected projection ready, leavingA1:4. Added explicit connected/saved/A1 loaded preconditions; expected B1:0 unchanged. Targeted ABS/MOD DPR1/2 2/2 passed5.8s. Full UI rerun is tracked live by exec session26976; final result to be appended.

Changed formula volatile module/resolver injection/dispatch/calculation identity+dirty context/calendar visibility, native volatile/calculation tests, scalar benchmark, browser guards/store receipt retirement/unit/E2E, integration/parity corpus generator, README/formulaREADME/semantics/compatibility/roadmap/ADR013/065/benchmark/RUNBOOK. Before accepting final slice, formatting and git diff checks are run. Automatic open/timer/volatile dependency epochs, workbook timezone/date-system UI/import, manual Excel verification, full phase3 stress, production auth/S3/snapshot crash matrix, OS XLSX sandbox/full XLSX profile and Linux/Compose/remote CI gates remain open. Next minimal stage: bounded/coalesced server automatic epoch admission. Full original objective remains active.

Финальный full Playwright rerun72/72 passed1.8min. pnpm format:check, cargo fmt --all --check и git diff --check прошли до финальной записи; локальный launcher23040 оставлен работающим, frontend/readiness200. Commit фиксирует explicit-epoch срез0.8.0, не automatic volatile triggers и не завершение всего MVP.

## 2026-09-30 — automatic first volatile context, ADR-066

Прочитаны AGENTS, roadmap, ADR013/065 и текущие actor/storage contracts. Первый контекст volatile-книги теперь создаётся после durable пользовательского commit или при authorized read старой книги. Compatible context переиспользуется. Trusted server sampler получает время/seed только на границе приложения; повтор не пересэмплирует pending request. User ACK остаётся подтверждением пользовательского события, system event имеет отдельную версию. Engine identity0.8.0 и wire/storage schemas не менялись. Полная цель всех фаз остаётся активной; timer/dependency/open-renewal ещё впереди.

SystemCommitRequest теперь несёт authenticated identity и expected ACL epochs. PostgreSQL system commit использует transactional access_locked: tenant/membership/document authorization проверяется под блокировками; Explicit требует editor, bootstrap Open допускает авторизованное чтение. Новая запись проверяет head и epochs, сохраняет CAS/durability. После отзыва доступа даже повтор receipt отклоняется. OperationJournal::system_receipt разрешает uncertain outcome по настоящему persisted fingerprint. Storage codec выделяет прежний неизменённый fingerprint algorithm в typed helper, без фиктивного события или миграции.

Fresh read-only volatile review нашёл две гонки. Pending bootstrap мог создавать лишний epoch после внешнего контекста или удаления последней volatile-формулы; теперь каждый retry сначала проверяет receipt и затем актуальную необходимость. Commit того же ID между refresh и receipt lookup мог возвращать старую projection; receipt с более новым head вызывает повторный refresh. Actual PostgreSQL fault tests покрывают все три pending cases и отдельную receipt race. Reviewer после исправления дополнительных concrete bugs не нашёл; внешняя model CLI не использовалась. Test-only forwarding journal пишет через настоящий PostgreSQL, production persistence не подменена.

Изменены server sampling/main/documents, application journal port, collab bootstrap/actor/options/test journal, storage commit/receipt/tests, storage fingerprint helper, live integration/E2E, README/semantics/compatibility/roadmap/ADR013/066 и RUNBOOK. Bootstrap AST scan отличает вызовы от текста, учитывает lazy branches консервативно, кеширует проверенную версию и не сканирует весь документ после обычного scalar edit.

Реально прошли cargo check server dev-auth; codec22/22 (включая прежние hash goldens); targeted PostgreSQL system4/4, pending bootstrap3/3, receipt race1/1; collab20 passed/1 ignored historical release benchmark; clippy application/storage-codec/collab/storage/server dev-auth all-targets -D warnings; just sqlx-check. Полный document integration45/45 за71246.0024ms с functional pacing; recovery2/2 за8289.6152ms на отдельных hard-restart серверах; typecheck/lint; targeted real-browser automatic/manual/lost-response6/6 DPR1/2 за10.7s. Full Playwright74/74 за2.1min, exec90114 подтверждён завершённым exit0. Это functional результаты, не latency benchmark. Native/WASM corpus и Criterion в этом срезе не повторялись: kernel не менялся, предыдущие результаты относятся к explicit-epoch срезу.

Ошибки при работе: fmt один раз получил mapped-file OS1224, после завершения compiler повтор прошёл; clippy обнаружил run с8 arguments, исправлено ActorOptions без suppression. Test helper ошибочно получил sqlx::test и FormulaSource::from_str, исправлены attribute/constructor. Cargo test с двумя positional filters не запускал suite, затем выполнены отдельные команды. HTTP test первоначально ожидал undefined вместо существующего serde null — assertion исправлен на действующий контракт. Попытка удалить якобы лишнюю SQLx metadata обнаружена sqlx-check: snapshots.rs также использует query; cargo sqlx prepare --workspace -- --all-targets --all-features восстановил её, tracked metadata diff отсутствует, повторная проверка прошла. Assertions production correctness не ослаблялись.

Проверенные dev launcher trees перезапущены для сборки изменённого сервера; текущий PID24868 оставлен работающим. PostgreSQL и документы сохранены. Пользовательскому запросу локального просмотра соответствуют реальные frontend5173/healthz/readyz HTTP200. Ошибочный probe /health вернул404, правильные /healthz и /readyz проверены отдельно. Automatic bootstrap load/latency benchmark, full phase3 stress, полный snapshot crash matrix, production auth/S3, XLSX OS sandbox/full profile, manual Excel, Linux/Compose/remote CI не запускались и остаются gates. Следующий срез — bounded automatic epoch admission при дальнейших изменениях и timer для clock consumers.

## 2026-09-30 — automatic volatile edit epochs, ADR-067

Previous goal turn was progress: verified full74-test browser result, corrected current docs, recorded ADR066 changes and committed03bb6f6. Current slice read AGENTS/RUNBOOK/roadmap/ADR066/013 and actor/storage contracts; incremental/TDD/API and fresh adversarial-review skills used. Full original MVP goal remains active; logo work is already separate and unchanged.

New live HTTP RED test failed on version4 instead of5 after another cell edit in a RAND book. Content/copy/undo and explicit structural operations now request a separate VolatileDependency epoch after durable user ACK. Only newly committed user events trigger it; receipts don't. Empty-axis ExtendRows caused by scrolling is intentionally excluded, and static books get no system event. Sampler remains at server boundary, no host clock in formula/domain. No new wire/storage format, engine identity or dependency.

Actor tracks latest required edit watermark and retains sampled operation ID/context across uncertain failures. Fresh review rejected a context-ID-only supersession contract: an older epoch/receipt can precede a newer edit. Watermark logic and authorized durable-tail recovery fix this. Author found crash gap between user/system commit; new OperationJournal calculation_epoch_version port locates latest system event through recovered head, validates codec/checksum/hash/type, then scans verified tail128events/page, stops at first relevant edit and rejects after4096events rather than hiding work. Two samples/request bound older resolution plus newer admission. Compatible already-covered reads don't resample. No fictitious persisted event/receipt or mocked production persistence.

Second read-only review found a race where external epoch E2/edit B arrive inside advance_context refresh after initial recovery. Post-advancement changed-head recovery now runs before retiring pending/caching inspected head. Real PostgreSQL forwarding journal injects E2 atv5 and B atv6 during receipt lookup, forces CAS retry, and asserts returnedv7/epoch3. Final reviewer found no further actionable defect; reviewer did not run tests, external model CLI was not used. Owned-code review checked ACK/durability/CAS, transactional ACL retained, bounded work, errors and content-free logs.

Actually passed targeted PG volatile_edit3/3 (restart gap, old pending/external context/later edit, refreshed-head race); existing bootstrap4/4 (including receipt race); collab20passed/1ignored release historical benchmark; cargo check server-dev-auth, clippy collab/storage/server dev-auth all-targets -D warnings and just sqlx-check; typecheck/lint. SQLx metadata generated against actual PG; the final new query reads/verifies latest system event, replaces initial MAX prototype, and has a checked offline entry. Live HTTP targeted3/3 passed4223.384ms. Browser targeted8-test run and full46-test HTTP run are observed via exec20329/20738; final outcomes recorded after completion.

Changed application journal port, storage query/forwarding/fault tests and SQLx metadata, collab admission/actor/reject-test adapter, HTTP/E2E tests, README/semantics/compatibility/roadmap/ADR067/RUNBOOK. Existing volatile fixtures expected unchanged epochs across writes; their version/epoch assertions now reflect the deliberate new semantics (three volatile writes each add a separate system event), numeric derivation assertions remain unchanged. Test helper samples fixed inputs; production sampling unchanged.

Development tree24868 was verified by CIM, stopped and rebuilt as5144; after final race fix verified5144 was rebuilt as1456. PostgreSQL/data preserved. First readiness probe before compilation finished got ECONNREFUSED; no restart based on that observation, same launcher subsequently returned200. Final full regression/recovery/format results still being collected. Timer/open-renewal, burst admission profiling/coalescing, calendar/timezone UI, full phase3–6/production/Linux/Compose/CI/manualExcel gates remain open. No new latency benchmark or performance threshold claimed.

Final real-browser targeted8/8 passed12.5s; complete Playwright76/76 passed2.1min (exec92342 exit0). Complete live document46/46 passed72620.0379ms with functional pacing (exec20738 exit0). Hard-restart recovery2/2 passed11393.061ms (exec17786 exit0); these generic restart tests are distinct from the3 actual-PG actor admission fault/race cases. Final collab20/20 plus1ignored passed after the refresh-race fix. Native/WASM corpus and Criterion were not rerun because kernel/schema did not change; production S3/Excel/Linux/Compose/remote CI and full production crash matrix were not run. Final formatting/default server check and clean diff are verified before commit; main launcher1456 remains live at5173/8080.

## 2026-09-30 — active clock timer epochs, ADR-068

Previous goal turn was progress: automatic edit admission committed asf6c73cb after real HTTP/browser/recovery verification. Read AGENTS/RUNBOOK/roadmap/ADR067/013 and current actor/heartbeat/authorization before this slice. Incremental/TDD and fresh adversarial contract review applied; full original goal including all MVP stages remains active, completed logo work untouched.

Existing authenticated WS heartbeat15s now enqueues internal Request::Timer only for active document subscription. Actor queue is bounded, try_send skips full admission and next heartbeat retries; no unconditional background scheduler or stored session principal. Actor reauthorizes/refreshes, resolves edit admission then checks normalized clock calls. NOW is due at60s since saved context, TODAY-only on civil-day change. RAND-only/static don't create timer epochs. UTC0 is current server policy; fresh offset mismatch is typed error, backward clock does not guess a result, checked subtraction avoids overflow. Closing/switching subscription stops its ticks; actor idle lifecycle retained.

Pending timer ID/sample survives uncertain commit failure and resolves receipt before cancellation even after consumers disappear or another context commits. Candidate/CAS/transactional ACL/durable commit unchanged. Multiple subscribers coalesce against persisted time. Changed-head ensure recovers content edits after an external epoch discovered during CAS, rather than losing their required version. Post-commit ensure failure does not undo durable timer or pending edit intent. No engine/schema/protocol/dependency changes; core still has no host clock.

Initial native timer-policy test was RED with missing timer_due_at (E0425). Initial implementation compiled wrong FunctionName/string comparison; fixed to as_str without weakening assertions. Policy table then passed threshold59999/60000, day boundary, nonconsumers/literal, offset mismatch; additional backward-clock/new-baseline regression passed in fullcollab21tests/1ignored historical release benchmark. Clippy initially found nested collapsible_if heartbeat branch; used edition2024 let-chain, rerun all-targets clippy collab/storage/server-dev-auth -D warnings passed. Frontend typecheck/lint passed; changes are E2E only, no frontend state/UI implementation.

Actual PostgreSQL timer6/6 passed2.34s: NOW threshold/coalescing; TODAY midnight; RAND-only sampler not called; commit response deliberately lost after genuine DBcommit then clock advanced120000 while retry retains same ID/sample; clock formula deleted before pending receipt retry; E2+editB injection during timerCAS yieldsv6/epoch3VolatileDependency. Native calculation on injected midnight asserted25570. Tests use genuine PostgreSQL and forwarding journal; production persistence not mocked. Fresh contract/code reviewer required these cases and found no remaining actionable defect; reviewer ran no tests, external model CLI not used.

Verified launcher1456 tree replaced by hidden17588 for timer code; PostgreSQL/data preserved and readiness200 before full browser. FullPlaywright78/78 passed4.3min (exec77615 exit0); real NOW waits production heartbeat/cadence at DPR1/2, records new server context/version and changed confirmed value with no user edit/preview. Observed1.3min percase is functional waiting time, not latency benchmark. Native/kernel parity fixtures and Criterion not rerun because evaluation algorithm unchanged. Final document46-case/recovery/SQLx/default-check runs still being collected via live handles; final outcomes appended after completion.

Changed collab bootstrap/admission/actor, server heartbeat, storage fault/race tests, browser E2E, README/semantics/compatibility/roadmap/ADR013/068/RUNBOOK. Timer classification/admission load benchmark, compatible open-renewal/RAND-on-open, timezone UI/manualExcel, full phase3–6/production/Linux/Compose/remoteCI gates remain open. Full MVP completion is not claimed; next slice is explicit compatible open-renewal policy and measured admission load.

Final timer verification: full document HTTP integration46/46 passed72113.8118ms including functional pacing. Initial recovery attempt did not execute tests: Windows denied replacing the running target/debug/pxel-server.exe (OS error5). Later default server cargo check and just sqlx-check passed in that command, but do not imply recovery passed. Verified launcher17588 by CIM and stopped only its dev tree; PostgreSQL/data remained untouched. Recovery rerun genuinely passed2/2 in7541.7118ms after successful build. Hidden scripts/dev.mjs restarted asPID19476; frontend5173 and backend8080/readyz each returnedHTTP200. Full format check, rustfmt and git diff checks passed before this final record; documentation formatting/diff is checked again before commit. No kernel parity or Criterion rerun claimed. ADR013 current summary now references accepted timer slice while preserving historical notes.

## 2026-09-30 — timer classification measurement

After c63c075, added ignored release timing harness for actual clock_consumers over1k/10k/100k populated scalar cells, static and one boundNOW profiles. No production change or dependency. Uses domain hydrate validation, checks expected classification/due predicate, 100 warmups then1000 timed black_box scans. First compilation failed because from_fixture is feature-gated in domain; switched to public validated hydrate with CellContent, no feature expansion or production workaround. Release harness passed1/1 in5.26s after12.82s build. At100k, static p50/p952182.200/3475.800us and oneNOW2249.400/3365.500us. Full table/environment/command/limitations recorded in docs/benchmarks/2026-09-30-timer-classification-local.md; ADR068 links it.

Read-only reviewer found no concrete defect; noted cache-warm scalar-heavy workload and Instant overhead, no subscriber/AST-heavy/end-to-end coverage. Reviewer did not run benchmark. cargo clippy collab all-targets -D warnings passed; targeted bootstrap tests3 passed/1 ignored. Local frontend/backend both200 on revalidation. Formatting and diff checked before commit. No HTTP/browser/native parity reruns needed for this test-only addition; previous timer evidence retains its own scope. Next admission profiling must include real PostgreSQL and subscriber/AST density before deciding on cached classification. Compatible open-renewal, full MVP gates remain active.

## 2026-09-30 — volatile subscription-open renewal, in progress

Read AGENTS, roadmap, RUNBOOK and ADR066/068 contracts. Apply incremental/test-driven workflow. ADR069 defines actual WS subscription transitions, distinct reconnect/open epochs including shared RAND, no per-read renewal and no duplicate same-active-document renewal. Fresh reviewer required comparison after authorized refresh, reuse first/edit context, receipt-first pending resolution even after supersession/deletion, failed open does not install subscription, and explicit reconnect traffic limitation. Added pure admission RED (missing Open enum E0599); implemented guarded Open, pending sample, internal actor request and server transition before replay barrier. Pure test and server check passed.

Actual PostgreSQL Open6/6 then expanded8/8 passed2.57s: compatible RAND/read retention, bootstrap reuse, static no epoch, lost genuine commit response same sample, deleted consumer retry, viewer allowed/revoked denied, external epoch supersedes candidate and external epoch+edit CAS recovery. Forwarding test journal generalized loss reason without replacing genuine DB commit; timer cases retained. Own review corrected pending Open clearing: keep it through post-commit ensure so failure cannot trigger a second fresh epoch. Browser reload expectations now require renewed context/value; new two-tab RAND equality test and real WS duplicate/switch test added. Frontend typecheck/lint and clippy collab/storage/server-dev-auth all-targets passed before that small pending-lifetime fix; full runtime verification follows. No new dependency/schema/engine changes. Local dev remains on previous verified binary until controlled rebuild; goal not complete.

Fresh code review found invalid Subscribe future cursor could renew context before rejection. Server now parses version and checks authorized head before Open, retaining later refreshed head/replay barrier. Live WS test sends999999 and confirms closed socket, unchanged head3/context; valid Open yieldshead4, duplicate subscription stays4, switch/static/return yields5. Post-commit recovery fault case uses genuine Open DB commit then one unavailable calculation_epoch_version read; retry retains same sample/receipt without another epoch. Expanded PostgreSQL Open11/11 passed3.73s, including recovered edit epoch reuse and cross-tenant denial. Final read-only review found no additional concrete defect; reviewer ran no tests/external CLI.

Controlled dev tree19476 stopped after CIM check; recovery2/2 passed8455.3556ms, then hidden launcher15784. Following final Subscribe ordering fix, verified15784 and rebuilt through hidden launcher3220; PostgreSQL preserved, frontend/readiness200. Final clippy collab/storage/server dev-auth all-targets -D warnings passed3.10s; collab22passed/2ignored10.01s; timer PostgreSQL6/6 regression1.34s; just sqlx-check passed5.86s, default server cargo check4.23s. Typecheck/lint passed. Full document HTTP47/47 passed72427.9263ms including functional pacing. Browser80-case suite is still live on session53092, DPR1 all40 and firstDPR2 renewal case passed; final outcome will be appended. pnpm format:check, rustfmt and git diff checks passed before final record. README, ADR013 current summary, roadmap/semantics/compatibility describe new Open semantics; historical notes remain historical. No engine/parity/Criterion rerun claimed, no admission-load or production gate marked complete.

Final Open outcomes: full Playwright80/80 passed4.4min, session53092 terminalexit0. Verified launcher3220 and stopped only dev tree after browser completion; recovery rerun on final code2/2 passed8106.6368ms after4.81s build. Hidden dev launcher23828 retained; readiness/frontend rechecked before commit. ADR069 accepted for this functional slice only; original MVP remains incomplete. Final docs formatting/rustfmt/diff checked before commit. Next measured slice uses real PG actor admission/drain with bounded producer bursts and separate queue-full accounting, scalar-heavy and AST-heavy workloads, due/non-due contexts. It must not claim WebSocket/subscriber end-to-end latency; calculation completion is a separate metric. Full production snapshots/XLSX profile/lookup/conditional aggregates, timezone/manualExcel/Linux/Compose/CI and broad stress gates remain open.

## 2026-09-30 — real PostgreSQL timer actor admission benchmark, in progress

Read AGENTS/RUNBOOK/roadmap/ADR069 and current journal/session code. Added cfg(test) timer_admission_benchmark module and exposed existing test fixture only within journal parent. No production behavior, dependency, schema or engine changes. Two1000-cell books are created with validated domain candidates, matching commands/events and real durable journal commits, then reconstructed by actor; no injected fake projection. Profiles: oneNOW+999scalars and1000 NOW+SUM formula ASTs. Setup/bootstrap/initial Complete calculation excluded from timing. Durability settings queried and asserted on; no connection credentials printed.

Current-thread runtime Barrier releases1/8/32 producers, each four consecutive bounded try_send attempts. Record accepted/QueueFull separately; unexpected errors fail, head barrier retries bounded.20retained rounds after2warmups, due=previousTimestamp+60000/non-due+59999. Verify actual journal/context/head and same-clock duplicate burst outside measurement. Fresh read-only reviewer found no defect; report must disclose drain includes producer readiness and authorized head request, enqueue combines accepted/full attempts, p95of20 is second maximum, later calculation may compete but completion is not measured. Controlled async-task burst is not parallel subscribers/WS throughput. Release harness is running on session41633; final measurements and verification will be appended. Local backend/frontend200, launcher23828 preserved; full MVP remains active.

Final timer admission release harness1/1 passed16.08s after2m01s compile; session41633 terminalexit0. All12 cases verified actual PG18.6 durability on, due/non-due epochs, same-clock duplicate no-op, persisted context equality and queue accounting.32 producers offer128 attempts per burst:64 accepted/64QueueFull;20retained bursts total2560attempts/1280accepted/1280full. OneNOW due drain p95 with1/8/32producers5.469/22.027/34.503ms;1000formula due11.588/29.194/111.679ms, worst186.800ms. Full results/limits in docs/benchmarks/2026-09-30-timer-admission-local.md, linked ADR068/roadmap. No optimization or end-to-end performance completion claimed; documented stage-level trace/controlled runner follow-up with completion criterion and correct attribution. Next functional slice remains lookup/reference profile.

Actual clippy storage all-targets -D warnings passed5.90s; Open regression11/11 passed3.37s; timer regression6/6 passed4.15s plus ignoredreleaseharness; just sqlx-check passed5.37s. No new production code/kernel/transport/UI, so native/WASM/browser/recovery suites not rerun for this test-only addition; prior evidence keeps its scope. Formatting/rustfmt/diff and local readiness/frontend are checked before commit. Changed test module/helper visibility, benchmark report, ADR068 link, roadmap and RUNBOOK; no secrets/content logs/new dependencies. Full original goal remains active.

## 2026-09-30 — INDEX reference slice, in progress

Previous status-only turn was no progress; revalidated current worktree and resumed
the existing INDEX implementation. Read project contract, roadmap, latest Open ADR
and current calculation tests; incremental-implementation skill applied. ADR070
records intermediate references, conservative dependency graph and unresolved
Excel forms. Kernel identity and current client/context guards advance to0.9.0;
persisted/wire schemas unchanged. Existing INDEX three native tests passed before
this continuation, including bounded source selection and typed resource errors.
Final native/WASM/live/editor verification has not yet completed. No full-MVP
completion or production gate is claimed; existing local dev remains available.

Added30 derived shared INDEX fixtures (367 total), native source/selector/cache and conservative-cycle tests, stateful parity sources, live PostgreSQL test, DPR1/2 dependent-edit/reload scenario and real100k-cell bound evaluator benchmark. Review found selected arrays became spreadsheet VALUE inside SUM and could be hidden by IFERROR; new regression observed RED, visitor now rejects arrays with typed Unsupported before callback, all4 INDEX tests pass. Another test switches selector between calculated source cells and verifies topological results (3 INDEX calculation tests pass). Final reviewer found no further concrete defect; reviewer ran no tests. Calculation rebuild comparison is from current workbook, not durable replay.

Actual full formula/WASM unit tests passed;367 expected/native/browser fixtures and1010books/2020patches parity passed after final production fix. Frontend83/83 tests, typecheck/lint, clippy formula/WASM/collab/storage/server dev-auth all-targets, default server check and SQLx offline metadata check passed. Collab22 passed/2 ignored; targeted actual-PG volatile regression4/4 passed. Targeted INDEX browser2/2 passed8.4s, real HTTP1/1 passed1574.6977ms. Full document48/48 passed78321.7238ms. Full82-case browser suite remains live session69844, including actual clock wait; result pending.

Criterion release INDEX100k fixture passed: selected scalar estimate13.563us (12.064-15.827), selected SUM26.920ms (26.119-27.710), warmup1s/measurement2s/20samples. Laptop had concurrent verification; preliminary results exclude parsing/binding, graph, network/commit/UI and are not p95. Report docs/benchmarks/2026-09-30-index-local.md. Verified dev launcher13972 replaced23828; after array fix verified13972 replaced by28520, both controlled tree stops preserve PG/data. Current native/WASM0.9.0 frontend and readiness200. Full browser/recovery/final format and commit remain pending; production/manualExcel/Linux/Compose/CI/full XLSX and remaining lookup/criteria gates remain open.

Additional final0.9 checks: calculated-source selector transition3/3 targeted tests passed; final formula all-targets clippy and pnpm build passed. Open actual-PG11/11 passed4.03s and Timer6/6 passed3.99s (release admission benchmark intentionally ignored). rg found no remaining0.8 engine guards in apps/crates/tests. Frontend/readiness both200. Formatting/diff checks passed; full browser still live on69844, secondDPR clock case waiting as designed.

Final INDEX slice: full Playwright82/82 passed4.9min (69844 terminalexit0). Expanded real hard-restart recovery2/2 passed7954.9756ms: additive INDEX document survives kill/restart, source/content hash and calculated hash match, duplicate command returns receipt, subsequent source edit recalculates14->26. Existing recovery assertions retained. Verified launcher28520 stopped only after browser finish, recovery command exit captured immediately and finally restarted hidden dev31376; PG/data preserved. ADR070 accepted only for contiguous INDEX slice, not general Excel/full MVP. README current engine guard corrected to0.9 and limitations documented. Changed formula implementation/tests/bench, current engine guards, shared fixtures/parity, HTTP/browser/recovery tests and architecture/compatibility/benchmark/RUNBOOK docs; no new dependencies or wire/storage schema. Final readiness/format/diff checked before commit. Remaining functions, dynamic reference dependencies, manual Excel/full XLSX/sandbox, broad load/fault and Linux/Compose/remote CI remain open. Next minimal functional stage: MATCH, specified against official semantics and shared fixtures before implementation.

## 2026-09-30 — MATCH lookup slice, in progress

Previous turn was verified progress: INDEX committedbea8146. Read AGENTS/RUNBOOK/roadmap/ADR070 and current evaluator/index. Incremental/TDD applied; official Support/Learn MATCH docs read and contract reviewed read-only before coding. ADR071 records both approximate modes, exact wildcard profile and provisional blank/error/coercion/collation rules. Native RED observed NAME instead of2. Bounded lookup module now preserves private INDEX selection, validates1D shape/cardinality/sorted approximate sources, charges folding/DP/comparisons, and keeps errors typed after an exact hit. Initial unit/native tests passed; property tests added. Index Selection/select are crate-private only; no schema/dependency changes. Engine/currentguards advance0.10.0 for upcoming combined tests; local dev remains previous verified0.9 until rebuild. Full MVP remains incomplete; manualExcel/fullprofile/production gates open.

MATCH native implementation now passes2 private unit/property tests and4 public native contract/resource/property tests. Read-only review found no concrete defect/budget bypass; added its decisive cases for mixed approximate ranks, wildcard literals, error precedence, normalized question mark, excess visitor cardinality and oversized mode. Full native formula/WASM tests passed, dependency test proves source/lookup invalidation and independent cache retention. Shared409 expected/native/browser fixtures and1010books/2020patches parity passed. Frontend83/83 tests, typecheck/lint, all-targets clippy formula/WASM/collab/storage/server-dev-auth, SQLx check14.23s, default server5.40s and pnpm build passed.

Verified dev31376 stopped and rebuilt as hidden29612; WASM/native0.10 now match, readiness200. Targeted MATCH browser2/2 passed5.1s and actual PostgreSQL1/1 passed1590.1525ms. Full document49/49 passed80866.2663ms;84-case browser suite still live83246, includes real clock waits. Extended restart fixture retains old INDEX checks and adds separate durable MATCH/receipt/source-recalculation checks; not yet executed.

Release Criterion100k actual bound numeric fixture passed: exact-first42.116ms (36.551-47.998), approximate-last24.918ms (24.323-25.514),20samples/1s warmup/2s measurement. CPU/build/browser verification load uncontrolled; report explicitly disallows interpreting relative ordering or treating estimates as p95. Both cases scan complete source, parsing/graph/DB/WS/render excluded. Report docs/benchmarks/2026-09-30-match-local.md. Native timestamps/context schemas and wire/storage formats unchanged; no new dependencies. Full MVP, manual Excel/locale/array/XLSX/load/Linux/Compose/CI remain open.

Final0.10 admission regressions: collab22/22 plus2ignored passed10.02s, actual-PG volatile4/4 passed1.95s, Open11/11 passed4.95s and Timer6/6 passed3.79s plus ignoredreleasebenchmark. Format/rustfmt/diff passed before documentation additions; current READMEs/compatibility/semantics/roadmap describe MATCH as partial/provisional. No arrays/locale/manualExcel or production deployment gate is claimed. Browser83246 still live, both real clock cases passed; recovery awaits terminal browser so Windows binary replacement cannot interrupt it.

Final MATCH: full Playwright84/84 passed4.9min (83246 terminalexit0). Recovery2/2 passed8746.158ms, including additive separate MATCH book: source/content hash and calculated hash match after hard kill, prior command returnsreceipt, subsequentsource edit changes2->10. Existing INDEX/scalar/ACL/convergence assertions retained. Verified29612 stopped only after fullbrowser, exit captured immediately and finally restarted hidden29072; PG/data preserved.

Added structural MATCH position test after final runtime suite: initial test failedInvalidProjection because it used cell-edit prepare helper for axis operations. Read real calculation lane/source: structural operations deliberately rebuild graph. Fixed test to use that existing production path; assertions2->3->2 remain unchanged. Targeted dependency/structural2/2 and final formula all-targets clippy passed. No production change for this correction, so completed native/WASM/live evidence remains scoped to the same production code. ADR071 accepted for bounded MATCH slice only. Changed purelookup/helpervisibility/dispatch/currentversionguards, tests/fixtures/parity, HTTP/browser/recovery, benchmark and docs; no new dependencies or schemas. Final format/readiness/diff and gitcommit follow. Full original MVP remains active; VLOOKUP/HLOOKUP are next, plus XLOOKUP/criteria/full XLSX/structure/recovery/load/manualExcel/Linux/Compose/CI gates.

## 2026-09-30 — VLOOKUP/HLOOKUP table slice, in progress

Previous goal turn was status-only/no progress. Revalidated current worktree,
AGENTS, roadmap and ADR072. Continued incremental/TDD workflow. Pure table lookup
reuses MATCH search, validates whole table, visits only key axis and selected
return cell, and returns scalar values. No new dependencies or schema changes.
Initial basic RED was NAME; native implementation is now green. Added transpose
property with independent exact/floor oracle, duplicate/scalar/coercion/index/error
tests, instrumented axis/selected-cell reads and malformed visitor cardinality.
Oversized-table test first failed at parser LimitExceeded; corrected it to assert
that existing parser gate explicitly, without weakening the range limit. Runtime
AST boundary coverage is still pending. Clippy found needless borrow and a
redundant test guard, both corrected without suppressions.

Actually passed six table tests, four existing MATCH tests, and native shared
corpus443 fixtures (34 new derived table cases); formula all-targets clippy passed.
Independent read-only review found no concrete defect and requested further
argument/DateTime/runtime-cap/dependency tests. Reviewer ran no tests. Fixtures
remain explicitly not manually verified in Excel. Engine bump, WASM parity,
real PostgreSQL/UI/restart and benchmark verification remain pending; ADR072 is
PROPOSED. Local running binary is still previous verified0.10.0. Full MVP remains
active; no stage or production gate is claimed complete.

VLOOKUP/HLOOKUP verification update: seven native boundary/property/visitor tests,
source/return/index/mode dependency/cache/cycle test and full native formula/WASM
suites passed.443 shared fixtures agree expected/native/browser;1010books and
2020patches converge.83 frontend unit tests passed. Initial typecheck raced the
launcher rebuilding generated WASM files; rerun after generation passed, as did
lint, all-targets clippy formula/WASM/collab/storage/server-dev-auth, SQLx check,
default server check and production frontend build. Targeted live PostgreSQL1/1
and DPR1/2 browser2/2 passed; full HTTP50/50 passed77686.8969ms. Full browser86
suite is still live85168. Engine/current guards0.11.0, launcher24440 verified;
no wire/storage schema changes. Collab22passed/2ignored, actual-PG volatile4,
Open11 and Timer6passed/1benchmarkignored passed.

Initial table benchmark failed after VLOOKUP measurements: HLOOKUP fixture used
10k columns beyond domain MAX_COLUMNS1000. Corrected fixture to100rows x1000cols,
retaining100k total cells and meaningful first-row search. No production limit
changed. Corrected release benchmark runs47758; initial incomplete measurements
are not accepted as final report. Recovery fixture now includes additive real
hard restart/table source/hash/receipt/source-edit checks, not executed yet.

Corrected table benchmark47758 exited0: actual bound100k-cell fixtures, VLOOKUP
50kkeys exact16.642ms(16.030-17.544),approx13.759ms(13.020-14.510);
HLOOKUP1000keys exact145.99us(135.24-161.33),approx114.76us(111.53-118.39).
Concurrent load uncontrolled; Criterion earlier-run comparison is not attributable
regression evidence. Report docs/benchmarks/2026-09-30-table-lookup-local.md records
failed fixture/correction and exclusions. No p95 or performance gate claimed.
Added actual structural rebuild test: V target survives inserted/deleted row;
H numeric return index remains positional, result34->21->34. Both new calculation
tests pass, final formula all-targets clippy passes, frontend/readyz200. Full86
browser still live85168. Recovery waits for terminal browser before stopping
Windows launcher; new recovery assertions not yet executed.

Final table slice: full Playwright86/86 passed5.3min (85168 terminalexit0).
After confirmed terminal browser, verified and stopped only launcher24440 tree.
Real hard-restart recovery2/2 passed8759.6345ms after4.72s build: additive table
book restores source/content hash and calculated hash, receipt retry and source
edit20->28; existing scalar/INDEX/MATCH/ACL/convergence assertions retained.
Finally restarted hidden launcher21204, PostgreSQL/data preserved. ADR072 accepted
for bounded table slice only. Second read-only review found no new concrete defect;
reviewer ran no tests. Changed pure lookup/search/dispatch, engine guards0.11,
fixtures/parity/calculation/boundary tests, live/browser/recovery tests, benchmarks
and docs. No new dependencies or wire/storage/AST schemas. Final formatting,
readiness and git commit follow. Next functional slice XLOOKUP; full original goal
remains active, with manual Excel/full XLSX/structure/load/recovery/production and
Linux/Compose/remote CI gates open.

## 2026-09-30 — XLOOKUP reference/search slice, in progress

Previous goal turn made verified progress: table lookup committedf5ce729, local
readiness200. Read AGENTS/RUNBOOK/roadmap/ADR072/current INDEX/lookup contracts.
Incremental/TDD skills continued; official Microsoft XLOOKUP documentation read.
ADR073 PROPOSED covers contiguous reference returns, forward/reverse and nearest/
wildcard/binary modes, lazy fallback and unresolved manual compatibility/spill/
colon/dynamic dependency gaps. First native RED observed NAME. Linear/reference
implementation passed basic8cases including SUM selected row, nested INDEX and
lazy error fallback; existing native INDEX4/table7/shared443 corpus remained green.
Binary test then RED typed Unsupported; implemented bounded sorted validation and
lower-bound search. Both XLOOKUP tests now pass. Clippy collapse warning fixed
without suppression; further tests/review/fixtures/parity/graph/live/recovery/
benchmarks and engine bump pending. Running app remains last verified0.11.0.
No dependencies or schemas changed, no full-MVP completion claimed.

Review/coverage update: Missing fallback placeholder initially returned0; regression
RED reproduced, filtering Node::Missing now keeps defaultNA and explicit blank-cell
fallback separate. Selected oversized text via COUNT/SUM also observed RED; shared
reference visitor now checks text/array/work boundaries. Nested scalar INDEX(XLOOKUP)
had the same bypass; separate RED closed with shared scalar_result. DateTime-0
bits then failed a targeted test; scalar INDEX now normalizes dates/nonfinite serial
consistently with XLOOKUP. No AST/wire/storage migrations required. Initial nested
reference test had mismatched2-vs3 axis lengths; corrected its lookup range to the
intended matching3-cell row, retaining its expected21. Compiler Result/Value mismatch
and clippy collapse fixed; no assertions deleted/suppressions added.

Nine native XLOOKUP tests now pass, including independent random nearest/binary
oracles, duplicate distinct payloads, horizontal/nested/ref coercion, typed limits,
mode/fallback/DateTime cases. Two graph/structural tests pass: sources/returns/key/
mode/direction invalidate one formula, independent cache retained, conservative cycle,
and insertion/deletion preserves targets.39derived fixtures added,482total native/
browser expected parity passed;1010books/2020patches parity passed. Full native
formula/WASM tests passed; structural test added afterwards and targeted run passed.
83frontend unit/typecheck/lint/build, all-targets clippy formula/WASM/collab/storage/
server-dev-auth and SQLx checks passed. Engine guards0.12, verified launcher21204
stopped and rebuilt as29612; no new dependencies or schemas. Targeted HTTP1/1
passed1437.9743ms, browserDPR1/2 passed2/2 in6.7s. Full HTTP51/51 passed83878.9652ms;
full88browser still live40422. Recovery additive XLOOKUP source/hash/receipt/edit
fixture prepared, not run yet. Native100k release benchmark46969 passed; preliminary
32.011/25.132/23.658/17.490ms forward/reverse/ascending/descending, uncontrolled
load, notp95. Report docs/benchmarks/2026-09-30-xlookup-local.md. Full MVP active.

Final pre-recovery checks: default server check passed2.40s; collab22passed/2ignored
passed10.01s, actual-PG volatile4/Open11/Timer6passed plus one intentional ignored
benchmark passed. Third read-only review found no new concrete defect; reviewer
ran no tests. Added runtime oversized lookup/return AST and wildcard DP work tests;
all10XLOOKUP tests and final all-targets formula clippy passed, rustc actual1.98.1
verified. Production code unchanged by these added tests, so native/browser/live
results retain their scope. Full format:check/rustfmt/diff passed, current engine
0.12guard scan had no remaining0.11 current guards; frontend/readyz200. Full88browser
40422 remains live and recovery awaits its terminal state. No open production/
manual Excel/Linux/Compose/CI/full XLSX/load/structure gate is marked complete.

Final XLOOKUP: full Playwright88/88 passed5.0min (40422 terminalexit0). Verified
launcher29612 and stopped its tree only after browser completion. Hard-restart
recovery2/2 passed8812.3616ms after0.39s build: additive XLOOKUP source/content hash
and calculated hash restore exactly, duplicate command returns receipt, source
edit14->22 recalculates. Existing table/INDEX/MATCH/scalar/ACL/convergence checks
retained. Finally restarted hidden3284; PG/data preserved. Formula README stale
claim that automatic epochs were pending corrected to existing ADR066-069 server
behavior. ADR073 accepted only for bounded XLOOKUP/reference slice. Changed pure
lookup/INDEX materialization/dispatch and engine0.12guards, tests/fixtures/parity,
HTTP/browser/recovery, benchmarks and docs; no new dependencies or schema. Final
format/readiness/diff and commit follow. Next SUMIF/COUNTIF then SUMIFS/COUNTIFS;
full original goal remains active, production/manualExcel/full XLSX/structure/
recovery/load/Linux/Compose/remote CI gates open.

## 2026-09-30 — SUMIF/COUNTIF single-criterion slice, in progress

Previous goal turn made verified progress: XLOOKUP committed9e796fd. Read AGENTS/
RUNBOOK/roadmap/ADR073/current lookup/INDEX/dependency contracts; continued TDD and
incremental skills. Official Microsoft SUMIF and COUNTIF docs read (COUNTIF canonical
URL fetch failed; actual official en-gb article fetched). ADR074 PROPOSED records
explicit provisional coercion/blank/error/255scalar/text/collation rules and static
SUMIF top-left expansion with implicit dependencies. Native RED observed NAME.
Pure criteria module now reuses charged Unicode/wildcard and selection helpers;
streaming COUNTIF/defaultSUMIF, bounded selection mask + sum traversal for separate
range. Numeric/explicit/implicit/nested INDEX cases pass native test. No new
libraries/persisted/wire/AST schema changes.

Graph RED observed evaluated_cells0 instead1 after B2edit with source formula
SUMIF(A1:A3,">5",B1). bound_dependencies now includes derived sum rectangle,
without rewriting durable AST/text. Same assertion passes and result50->70;
independent formula stayscached. Compiler FunctionName/string comparison corrected
using existing typed as_str; formula all-targets clippy passed. Current running
server/WASM remains lastverified0.12.0; engine bump/resource/property/fixtures/
parity/live/recovery/benchmarks still pending. Read-only review requested. Full MVP
remains active; ADR074 not accepted yet.

Continuation after percentage status: preceding turn was status-only, so checked
current worktree and continued the active criteria slice. Seven native criteria
tests pass, including property oracle,2D/reversed/shrunken INDEX and XLOOKUP
references, both visitor cardinalities, typed limits/cancellation, wildcard and
selected errors. A test mistakenly used unsupported compact2D INDEX and a horizontal
XLOOKUP result requiring dynamic expansion; corrected test inputs to explicit
INDEX(...,0,0) and a2-row returned column, preserving the existing profile limits.
Two graph tests pass including implicit summed formula topological order/self-cycle
and inserted/deleted row dimensions/anchor. Added38 derived shared fixtures,
520total native corpus cases passed; no manual Excel verification claimed.
Full formula/WASM native test suites passed before identity bump. Verified launcher
3284 identity and stopped only its process tree, retaining PostgreSQL/data.
Current guards advanced0.13.0 and stateful corpus source pool gained four conditional
aggregate forms. Browser WASM/parity/live/recovery/benchmarks remain pending.

Engine0.13 hidden launcher23780 rebuilt WASM/backend and starts successfully.
520 shared expected/native/browser cases and1010books/2020patches parity passed.
83frontend unit tests, typecheck/lint/production build passed. Full52real-PG HTTP
tests passed79693.3769ms, including implicit B2 edit changing52->62 and receipt.
Full90browser tests live62498; first DPR1 conditional edit/reload passed. Additive
hard-restart SUMIF hash/source/receipt/implicit edit fixture prepared, not run yet.
Full all-targets formula/WASM/collab/storage/server-dev clippy passed; SQLx offline
prepare check and default server check passed; post-bump WASM4unit/9stateful tests
and final formula all-targets clippy passed. Native100k release benchmark passed:
COUNTIF21.002ms, defaultSUMIF15.171ms, anchoredSUMIF58.659ms estimates; concurrent
load, notp95. Actual command/report in docs/benchmarks/2026-09-30-criteria-local.md.
Docs updated to explicit partial criteria profile; ADR074 still PROPOSED pending
full browser/restart/final review. Full MVP remains active.

Final SUMIF/COUNTIF regression: full90Playwright tests passed4.7min (62498terminal
exit0). Second read-only review found no concrete defect and ran no tests.
Collab22passed/2intentional benchmark ignores; real-PG volatile_edit3/Open11/Timer6
passed, one intentional timer benchmark ignored. Improved only formatting in the
new bound dependency block; both implicit graph tests and all-targets formula
clippy passed afterwards. Format:check/rustfmt/diff passed and readiness/web200.
Verified launcher23780 and stopped its tree only after browser terminal success.
Hard-restart2/2 passed9773.1093ms after5.54s server build: SUMIF source/content and
calculated hashes restored exactly, duplicate command returns receipt, implicit B2
edit52->62 recalculates. Finally restarted hidden25956 with PG/data retained.
ADR074 accepted only for bounded criteria slice. Changed formula criteria/dispatch/
shared lookup helpers/reverse dependencies and0.13guards, native tests/fixtures/
stateful parity, HTTP/browser/recovery, benchmarks and docs; no new dependencies
or schemas. Next SUMIFS/COUNTIFS; original full MVP goal remains active and wider
manual Excel/full XLSX/structure/recovery/load/production/Linux/Compose/remote CI
gates remain open. Final formatting/readiness and commit follow.

## 2026-09-30 — SUMIFS/COUNTIFS multiple-criteria slice, in progress

Previous turn made verified progress, SUMIF/COUNTIF commit8c24066. Read AGENTS,
RUNBOOK/roadmap/ADR074 and current criteria/parser contracts; continued incremental
implementation and TDD skills. Official COUNTIFS and alternate en-gb SUMIFS pages
read (canonical en-us SUMIFS fetch failed). ADR075 PROPOSED specifies1..127 pairs,
equal normalized row AND column counts, shared provisional criteria, bounded mask
intersection and separate SUMIFS boolean-sum1/0 rule. Added native specification
test before dispatch/implementation. Current running engine remains verified0.13.
Full original MVP goal remains active.

Native RED observed missing function NAME on first COUNTIFS case; implementation
adds bounded pair admission/equal shape and mask intersection plus fixed-order
SUMIFS traversal. Specification now passes. Clippy found manual modulo/chunks_exact
warnings; fixed via stable is_multiple_of/as_chunks, no suppressions. Four new
native unit/property/resource tests (11criteria total) and two dependency/structural
tests pass; shared34fixtures added,554total native corpus passes. Full formula/WASM
native suites passed before identity bump. Read-only review found no concrete
defects and ran no tests. Verified launcher25956 and stopped its tree, preserving
PG/data; current guards advanced0.14.0. Stateful corpus pool/new HTTP/browser and
100k benchmark cases prepared; combined parity/live/restart/build still pending.

Engine0.14 hidden launcher18192 rebuilt backend/WASM successfully.554shared
expected/native/browser cases and1010books/2020patches parity passed.83frontend
unit tests, typecheck/lint/build passed. All-targets formula/WASM/collab/storage/
server-dev clippy, SQLx prepare check, default server check and post-bump WASM4unit/
9stateful tests passed. Real HTTP new criterion-range edit test passed52->22->0;
full53HTTP/browser92 suite still live55356. Additive hard-restart source/hash/
receipt/second-criterion edit case prepared, not run yet. Native100k release
benchmark passed: COUNTIFS36.439ms/SUMIFS44.664ms estimates under concurrent load,
notp95; report docs/benchmarks/2026-09-30-multi-criteria-local.md. Docs updated for
partial profile, corrected stale architecture claim that executor/worker were
unconnected using existing runtime/parity evidence. No schema/dependency change.
ADR075 remains PROPOSED until full browser/restart and final checks.

Full53HTTP passed81794.4486ms; browser92 live55356, new DPR1 multi-criteria
edit/reload passed. Added third graph regression: deleting only criteria-range
end changes shape and yields VALUE rather than summing misaligned data; all3graph
tests and final formula all-targets clippy passed. Production implementation
unchanged by this added test. Collab22passed/2intentional benchmark ignores,
real-PG volatile_edit3/Open11/Timer6 passed, timer benchmark intentionally ignored.
Format:check/rustfmt/diff and readiness/web200 passed. Recovery remains pending
browser terminal state; no full-MVP/manualExcel/Linux/Compose/remoteCI gate closed.

Final SUMIFS/COUNTIFS: full92Playwright tests passed4.9min (55356terminalexit0),
including new dependent edit/reload at DPR1/2. Verified launcher18192 and stopped
only its tree after browser completion. Hard-restart2/2 passed10878.3049ms after
4.66s server build: new SUMIFS/COUNTIFS source/content and calculated hashes restore
exactly, receipt retry stays idempotent, second-criterion edit52->22 recalculates.
Finally restarted hidden26420, retaining PG/data. ADR075 accepted only for bounded
multi-criteria slice. Changed criteria.rs/evaluator dispatch/0.14current guards,
native criteria/calculation tests, shared corpus/stateful parity, HTTP/browser/
recovery and scalar benchmark, plus README/compatibility/semantics/roadmap/ADR075/
benchmark report and corrected stale architecture worker wording. No new dependency
or schema. Next phase4 gap: authored formula write undo, then broader history/
batch/redo. Manual Excel/full XLSX/structure/recovery/load/production/Linux/Compose/
remoteCI gates remain open and original full goal active. Final readiness/format/
diff check and commit follow.

## 2026-09-30 — Authored formula write undo, in progress

Previous turn progressed and committed892116f, verified SUMIFS/COUNTIFS. Read
AGENTS/RUNBOOK/roadmap/ADR028/029 and current domain/codec/actor/journal/frontend
undo contracts. Continue incremental implementation/TDD skills. ADR076 PROPOSED
extends server-derived CellWrite preimage to SetFormula using schema16/authored_kind3
and independent hash tag17, keeping old event bytes and wire patches unchanged.
Both formula result and formula preimage refuse restoration across axis changes;
ownership/equal-write/tail-window/CAS/ACL/receipt guards stay. Native specification
test added before new enum/API. Running dev remains verified0.14; full MVP active.

Authored formula undo: domain specification first failed to compile without the
new kind, then all five domain undo tests passed. Codec schema16 specification
first rejected the new write; after implementation all23 codec tests passed,
including legacy hash goldens. Frontend eligibility assertion first failed, then
all83 frontend unit tests passed. Full domain/codec suites passed with one
intentional domain stress ignore. Protobuf generation passed. The new HTTP test
was RED against the previous running backend (undo rejected); rebuild/migration,
live PostgreSQL and browser/restart checks remain pending. Removed an unreachable
test-helper arm and clarified the historical-axis guard comment. No full-MVP gate
is closed by these unit results.

Launcher26420 identity verified and only its tree stopped; hidden20888 rebuilt
backend/WASM and applied schema16 migration while preserving PostgreSQL/data.
Live authored-formula restoration test passed (value/formula/blank, dependent
recalculation, exact receipt retry). New real-PG journal test passed, including
schema16 decode, equal rewrite UndoConflict and compensation replay/hash/receipt.
Existing undo guards4passed/1intentional timing ignore. Focused editor DPR1/2
2passed; authored formula after axis change correctly rejected with unchanged
head. Strict all-targets domain/codec/collab/storage/server-dev clippy passed;
protobuf check, frontend typecheck/lint/build, SQLx prepare check and rustfmt passed.
Collab22passed/2intentional benchmark ignores. Full integration/browser session5644
is still live; additive hard-restart source/hash/result/receipt test prepared,
not yet run. No new performance claim or full-phase gate closure.

Full first regression:63 HTTP integration/network/security cases passed (includes
54 document cases),92/94 browser passed. Both failed browser cases were delayed
formula text after row insertion: correct value12 but old authored A1 display.
Root cause: committed_message transformed detection matched only legacy
SetCellFormula, not new authored CellWrite. Extended detection to authored
SetFormula only, preserving unchanged-axis formula compensation wire behavior.
Independent read-only review found two concrete gaps (no tests run by reviewer):
transactional formula-axis guard and codec future-binding encode symmetry. Both
reproduced RED. Encoder now rejects new-schema result/preimage future bindings;
all23 codec tests passed. Shared domain Operation::changes_axes now drives domain
axis version and journal history guard; actual PG inverse after ExtendRows now
fails NotUndoable before head/receipt mutation. Undo tests5passed/1timing ignore.
Stopped verified20888 only after browser terminal failure and restarted hidden
31724 for fixes. New full regression/recovery still pending; failures retained.

Hard restart tests2/2 passed12570.9939ms with new authored-write event recovered
before undo, restored formula result/hash after undo restart and idempotent receipt.
Review found a further display defect: an axis edit BEFORE the overwritten target
can leave a formula preimage whose durable canonical text is historical. New browser
regression initially exposed a harness navigation error (bar Enter does not move
selection); corrected target assertion then RED reproduced '=B1+1' instead of
current '=(B2+1)' at DPR1/2. Formula compensation now also uses the existing
transformed/projection refresh for older binding versions. Updated HTTP contract
assertion explicitly checks transformed/no raw canonical payload, retaining exact
source/result checks. No assertion weakened to tolerate historical display.
Verified18784 stopped after tests terminal; restarted15992. Final focused cases
and full regression remain pending. Engine identity stays0.14; no benchmark claim.

Final focused regressions6browser DPR1/2 and4HTTP passed after compensation
projection-resync fix. Default-feature server check,83frontend unit tests,
protobuf/rustfmt checks passed. Full final integration64/64 passed90047.8298ms
(includes55document tests and9network/security tests);96browser cases remain live
session1669. Presentation updated verified0.14 corpus counts554fixtures and
1010books/2020patches plus bounded function families, keeping explicit incomplete
MVP disclaimer. Real presentation Chromium checker passed1440x900 and390x844,
10slides/navigation/no page errors/no horizontal overflow. No new timing results
or Excel golden claim. Final browser/recovery of last display fix still pending.

Verification command correction: cargo test -p pxel-server --features dev-auth
--lib failed because server is binary-only (no test executed). Correct --bin
pxel-server command passed17/17 unit tests. Final SQLx prepare check and format
check passed. Read-only review follow-up confirmed both first findings fixed and
identified historical compensation display; browser RED/GREEN evidence above
covers that finding. Final browser session1669 remains live; no completion claim.

Final ADR076 regression:96/96 Playwright passed5.3min (1669 terminal exit0).
Verified15992 and stopped only its tree after browser completion. Final hard
restart2/2 passed10863.8576ms after3.62s build, covering original schema16 recovery,
formula compensation source/result/content+calculated hashes and receipt after
restart. ADR076 accepted for authored formula Undo only. Changed domain kind/shared
axis predicate, actor normalization, journal+codec schema/hash/guards and migration,
protobuf enum/descriptor, frontend eligibility, server transformed/resync handling,
unit/live/browser/recovery regressions and persistence/collaboration/roadmap/docs.
Presentation refreshed existing verified corpus and passed its actual browser
checker at two viewport sizes. No dependencies or engine identity change; no new
performance benchmark or manual Excel/whole-phase/production claim. Broader
history/batch/redo, XLSX/recovery/load/production/Linux/Compose/remoteCI remain open.
Restart local development stack and final format/readiness/commit follow.

## 2026-09-30 — Multi-step single-cell undo and redo, in progress

Previous stage e38870d is verified and local launcher16196 serves it. Read
AGENTS/RUNBOOK/roadmap/ADR028/029/076 and current journal/actor/frontend contracts.
ADR077 PROPOSED extends inverse eligibility to own complete compensations and
reduces exact ID/version pairs in bounded own single-cell history. Foreign,
legacy/batch/copy writes remain hard barriers. Frontend bounded100-entry history
will use acknowledged IDs, keeping pending/confirmed state separate. Independent
read-only contract review found no blocking contradictions and specified tests;
reviewer ran none. New real-PG same-cell undo/redo chain test added before changes.
No wire/event-format/engine/dependency change proposed. Full goal remains active.

ADR077 TDD: real-PG chain RED returned UndoConflict before implementation; shared
inverse eligibility and exact ID/version tail reduction made all eight same-cell
write/undo/redo actions plus receipt retries/replay GREEN. Frontend existing undo
spec first failed missing canRedo; after bounded histories/inverse intent changes
85unit tests passed, typecheck/lint passed. Added100-entry undo/redo traversal and
new-edit/failed-top regressions. Foreign equal-value regression now includes the
foreign author's own compensation; owner's earlier inverse must still conflict.
Added live mixed value/formula multi-step and two-browser history/branching test.
Live HTTP is RED against previous backend (second undo rejected), rebuild pending.
No durable schema/engine/dependency change; full MVP goal remains active.

Live mixed value/formula multi-step test and two-tab DPR1/2 chain/branching passed
on rebuilt hidden9612. Native domain/codec/collab suites passed (codec23, collab22
with2intentional benchmarks, domain stress intentionally ignored), SQLx/protobuf
checks passed. PostgreSQL undo6passed/1timing ignore. New inverse-pair timing
harness executed500pairs/1000events with exact replay; explicit durability settings
on verified run measured41.066ms debug wall time, single uncontrolled sample,
notp95. Report docs/benchmarks/2026-09-30-single-cell-history-local.md. Independent
implementation review found overlapping receipt refresh/canonical ACK could push
an authored history ID twice. Test first exposed harness selecting a Subscribe
instead of the last command; corrected harness then RED reproduced remaining
canUndo after one inverse. Re-read pending after refresh before consuming history;
87frontend tests, typecheck/lint passed. No assertion weakened. Added hard-restart
redo of persisted formula compensation test, not run yet. Full regressions pending.

Full final HTTP integration/network/security65/65 passed92153.6855ms;98browser
cases live session28558. Strict all-targets domain/collab/storage/server-dev
clippy, server17unit tests and frontend production build passed. Format/diff
passed. Docs now describe exact inverse-pair reduction, foreign barriers and
session-local bounded histories. No full-stage/MVP acceptance or new p95 claim.
Hard-restart redo case is prepared; run after browser terminal state.

Full98browser passed5.2min; verified9612 stopped only after terminal exit0.
Hard-restart2/2 passed12008.4374ms after3.81s build, including redo targeting a
persisted formula compensation and exact source/content+calculated hash/receipt
recovery. Added wrong target-version PG regression; same-cell chain and storage
all-targets clippy passed. Final contract check exposed pre-existing error-priority
mismatch: deleting a formula target returned NotUndoable before UndoConflict.
Live source/compensation deletion test RED observed14 instead of13. Actor now
checks live address before historical-axis refusal, retaining typed14 for a live
address with changed axes. Hidden26076 rebuilt previous state for reproduction;
rebuild/focused/full HTTP validation of priority fix remains pending.

Final error-priority fix: new deleted source/compensation and live-axis refusal
HTTP2/2 passed, followed by complete66HTTP/network/security suite88936.4796ms.
Collab22passed/2intentional benchmark ignores and strict collab/server-dev
all-targets clippy passed. Earlier98browser/87frontend and hard restart2cases
passed; final recovery/readiness/format/commit follow. ADR077 accepted for bounded
single-cell history only. No wire/storage schema, engine identity or dependency
change. Changed shared inverse eligibility, journal exact ID/version tail
reduction, actor target handling/error priority, bounded UI stacks+ACK guard,
redo toolbar/keyboard, unit/PG/live/browser/recovery tests and timing harness/docs.
Presentation/roadmap reflect guarded single-cell undo/redo while whole MVP,
batch/structural/history-discovery and production/deployment gates remain open.

Final recovery2/2 passed10738.4886ms after0.40s incremental build, including
persisted formula redo/content+calculated hashes/receipt. Real presentation browser
checker again passed1440x900/390x844 (10slides/navigation/layout/no errors).
Final rustfmt/Prettier/diff passed. Hidden development stack will be restarted;
commit/readiness confirmation follows. Full goal remains active; next batch
preimages/atomic guarded range inverses are not implemented in this slice.

Feature commit f4b9ba7 created. Hidden launcher2456 rebuilt current backend/WASM;
readyz8080 and frontend5173 both returned200. Feature worktree was clean at that
check. Local data/PostgreSQL retained. This final verification record is a docs-only
follow-up; the original all-stages objective remains active.

## 2026-09-30 — ADR078 atomic batch final-capacity prerequisite

Previous turn made verified progress (ADR077 feature f4b9ba7 and runtime/docs
bf89427). Goal remains all remaining MVP stages; logo task was previously closed.
Read AGENTS, roadmap, architecture, relevant ADR025/030/077, current candidate,
actor/journal/codec and RUNBOOK. During range undo preparation found an existing
order-dependent capacity rejection in batch candidate application. This is a
necessary correctness prerequisite; range undo itself is not yet implemented.

Applied existing TDD/incremental workflow. New test initially failed to compile
because it used state_hash instead of Workbook::hash; corrected the harness.
The attempted PowerShell Set-Content -NoNewline was unsupported and changed no
file; switched to explicit .NET WriteAllText. Actual RED then reproduced
LimitExceeded in three tests (copy/value/guarded preview) at full text allocation;
original-state overflow rejection already passed. A first extraction script's
line-ending check aborted before writing; completed extraction using actual LF.

Candidate now validates unique destinations and guarded original preimages,
clears all replaced contents in a private clone, then applies final writes.
Shared guarded validator retains metadata, formula/value and preimage checks;
future undo target versions are checked against original head. No event/wire
format, canonical hash, dependency or formula evaluator identity change.
Seven focused capacity/guard tests passed, including100k populated cells,
order-independent hashes, formula origin/future binding and no mutation on error.
Feature-enabled formula_content7passed, including AST-memory saturated exchange.
Readonly reviewer found no concrete defect and requested cell-count/AST/formula
checks; these were added. It did not execute tests or edit files.

Native domain/storage-codec/collab suites passed (codec23; collab22passed,
2intentional timing ignores; full convergence intentionally ignored). Those ran
with the first4new tests; the final7focused tests separately passed. Domain
all-targets clippy with test-support and default server check passed. Frontend
87unit passed. Optimized Criterion1000-value candidate:519.93–652.33us,
central599.42us,10samples; this is domain-only/uncontrolled, not commit p95.
Hardware re-read: Windows10.0.26100/CoreUltra9 185H/16373108KiBvisibleRAM.

Own hidden launcher2456 identity verified and its process tree stopped;
launcher32260 rebuilt WASM release15.41s and backend dev9.94s. Backend readyz200
and frontend200 confirmed after build (first early backend probe refused while
compiler was still running; launcher was not restarted). PostgreSQL/data retained.
Full live HTTP/network/security and native/browser parity are now running;
terminal outcomes, final formatting and commit will be recorded below.
Final ADR078 regression: shared corpus554native/browser cases and1010workbooks/
2020patches converged on rebuilt WASM. Full66HTTP/network/security passed in
88614.0504ms. Focused18Chromium paste/copy/undo/redo tests passed29.9s atDPR1/2;
additional mixed formula/value range2passed7.2s. Initial pnpm-shim regex invocation
failed before tests because Windows interpreted pipe characters; direct Node
Playwright CLI invocation ran the actual tests. No assertion/test was removed.
Rustfmt, selected Prettier and git diff checks passed. Final recovery currently
running after own32260 was verified/stopped only after all browser handles ended.
No full98browser suite, full100k convergence stress, controlled p95, manual Excel,
Linux/Compose or remoteCI run is claimed for this slice. New saturated100k-cell
unit fixture is distinct from a100k-command convergence/load run.
Final hard-restart recovery2/2passed10803.837ms after0.35s build; own launcher restarted, readiness verification follows. ADR078 is a complete candidate correctness fix; persisted range history remains next and the full goal stays active.

Feature commit a8b66c7 saved ADR078. Final hidden launcher17652 built WASM0.16s/backend0.33s; readyz8080 and frontend5173 returned200. Worktree clean before this verification-only entry. PostgreSQL/data retained. Range undo/redo and remaining full MVP gates are open; goal remains active.

## 2026-09-30 — ADR079 atomic range history implementation

Previous turn completed ADR078 candidate capacity fix (a8b66c7/f8bd361), verified
runtime17652. Read AGENTS/RUNBOOK/roadmap/ADR077/078 and current domain, journal,
actor, codecs, calculation and wire mapping. Continuing full remaining-MVP goal.
Agreed shared RangeWrite contract before delegating codec/proto/migration to
existing volatile_review agent, then separate journal/range_history_tests.rs only.
Parent owns domain/collab/journal/server/UI/live/browser/recovery/docs and runtime.

New GuardedContentWrite/RangeWriteKind/RangeWrite stores exact per-address preimages.
Domain test actual RED InvalidAxis after adding unsupported-model match arms;
GREEN after atomic candidate implementation. UI copy-history assertion actual RED
false canUndo; GREEN after registering acknowledged batch/copy with existing stack.
Live range-history test against old running backend actual RED rejected on Undo.
Own17652 verified/stopped, hidden15184 rebuilding actual new backend/WASM.

Codec agent implemented immutable schemas17/18, hash tags18/19/20, protobuf
range messages and generated descriptor, principal migration202609300002; old
schemas retained. Its actual codec29/29, clippy/proto:check/diff checks passed.
Parent read validator/lib integration; strict metadata/formula age/origin,
512KiB incoming+preimage and4MiB envelope constraints retained. No wire TS/newdeps.
Readonly cross-review found ZERO inverse target domain/codec mismatch and a
multiplicative1000×10000 history-work risk. Parent fixed ZERO guard with test,
added64MiB tail-byte and100000 matching-address update budgets returning typed
UndoWindowExceeded beforecommit. Journal now scans one tail and reduces exact
ID/version own inverse pairs per address; scalar/range mixed, foreign/legacy
barriers retained. Exact complete inverse footprint+shape checked inside lock/CAS.
Actor derives inverse, checks all addresses before formula-axis refusal; queues,
confirmed/pending/calculation separation unchanged. Tests and acceptance pending.
Live RangeWrite GREEN: multi-step scalar/batch inverse and frozen mixed copy
2/2passed, then foreign-one-destination equal-value+owncompensation and formula
axis/deleted-destination2/2passed. Complete HTTP suite68/68passed102184.3525ms
beforethose2late additions; those2were run separately, notclaimedfull70one-run.
Domain/storagecodec/collab suites passed (codec29; collab22/two timing ignores),
fullformula suite and serverdev binary17unit passed. Frontend87unit/typecheck/
lint/build passed. SQLx preparecheck and strict domain/collab/storage/server-dev
all-targets clippy passed; defaultserver check and proto generationcheck passed.

Separate-file PG tests actual4/4passed15.32s; exactcomplete footprint rejects
missing/extra/reordered, bothscalar↔range cancellation+late receipts+replay,
foreigntouch+owninverse remainsbarrier,100000touch boundary admitted then101st
1000cellwrite rejectsUndoWindowExceeded unchangedhead/receipt. Firstfixture-only
failurecamefromreissuingdevelopmentworkspace for foreignactor; testcorrected
withgenuine user/membership/ACL records. Additional tail-byte PG1/1passed15.86s:
SQLSUM proves65independent16x32KiB rangeevents cross64MiB,64prefixdoesnot; inverse
rejectsunchangedhead/no receipt evenwithoutmatchingtailaddresses. No combined5
run claimed. Storageclippy passed afteraddition. Agenteditonlyownedtestfile.

Focused Chromium TSV/mixedcopy4/4passed11.8s withatomicundo+redo, two-tabmixed
formula convergence and reload. Full98browser nowrunning; no restartwhilelive.
Updatedtoolbarlabels to coverranges, collaboration/persistence matrices/roadmap
andpresentationphase4line. Addedhard-restart undo/redo ofpersistedrangecomp,
notrunyet. New optimizedCriterion1000-rangeinverse candidate:654.36–693.61us,
central673.93us,10samples/1s warmup/2s measure, excludesDB/historycalc/browser.
Fullstagegate/commitpending. No formulaengine version/newdependency/wireschema
change; persisted storagecodec schemaadds17/18, migration executed bylive startup.
Final ADR079 checks: complete98Chromium DPR1/2 passed5.4min. Onlyafterterminal
exit0 own15184wasidentityverified/stopped for Windows-safe recoverybuild.
Hard-restart2/2passed13906.5711ms after4.73sbuild, nowincluding schema17range
source + schema18undo and redo ofcompensation acrosshardrestarts, exact content/
calculated hashes and late receipts. Rebuiltnative/browser corpus554cases and
1010workbooks/2020patches againconverged. Presentationchecker loadedactualHTML
inChromium at1440x900/390x844:10slides, Home/End navigation, updatedrangehistory
text, nohorizontaloverflow/pageerrors. SelectedPrettier/rustfmt/diff passed.

ADR079accepted forguarded interactive rangehistory; nextstructural/style history,
cut/move anddurablehistorydiscovery remainopen. Storageevent schemas17/18 and
principal migration areadditive; previous schemas1..16/hashgoldens remainvalid.
Changed domainrange model/candidate, separatecodec/proto/descriptor/migration,
actor/journal/incrementalcalculation/wirefallback, localacknowledgedUIhistory,
toolbarlabels, native/PG/live/browser/recoverytests andbenchmark/docs/presentation.
No fullstoragealltests, full100k convergence/load, controlledcommitp95, manual
Excel parity, Linux/Compose/remoteCI orproduction gate claimed. Currentstage
commands and allactualoutcomes recordedabove; fullall-stages goalactive.
Ownhidden devlauncher restarted afterterminalrecovery; finalreadycheck/commitfollows.
ADR079 featurecommit27cea16 saved. Hiddenlauncher24492 rebuilt WASM0.21s/backend0.36s; backendreadyz8080/frontend5173 returned200. Worktree cleanbeforethisverificationentry. PostgreSQL/dataretained. Rangehistory slicecomplete; structural/styleundo, cut/move, durablehistorydiscovery and remainingMVP/deploymentgates stillopen. Fullgoalactive.

## 2026-09-30 — ADR080 pure cut/move planner, integration remains open

Continued authorized phase4 from ADR079 after inspecting current AGENTS/RUNBOOK,
roadmap and history ADR. Previous status-only user response was not a development
slice; revalidated worktree and resumed the existing relocation RED tests. Runtime
24492 retained, backend8080 and frontend5173 both returned200; no restart/DB change.

Added formula::relocation with immutable frozen mapping, same-sheet live geometry,
blank source targets, both overlap directions, cross-sheet inbound references,
formula origin relocation with recomputed relative deltas and absolute flags,
fully contained reversed ranges, partial-range typed refusal, exact preimages and
input version/domain hash. Independent limits:1000sourcecells/10000writes/1M AST
nodes. Added cancellation with complete result discard. No new operation/codec/
wire/migration/UI dependency; this is a planner component, not completed MoveRange.

TDD initial module/API absence failed compile, then3tests passed. Cross-sheet test
fixture first incorrectly reused globally unique axes across sheets (InvalidAxis);
fixed fixture with distinct IDs. Its expected formatter text then omitted existing
canonical sheet quotes; corrected expectation with explicit explanation. Expanded
suite11/11 passed including random frozen-value rectangles, output-limit admission,
preimages restoring content, cancellation after first external scan. Full formula
suite passed before final optimization and wasm32 spreadsheet-wasm build passed.

Read-only review identified clone/discard of unchanged external formulas, fixed
with charged borrowed inspection before cloning only affected formulas. Afterfix
11/11passed. Strict clippy identified collapsible match/if; correction/verification
pending below. No semantic target/overlap bug reported by reviewer. Independent
formula benchmark beforeoptimization3.5623–4.1173ms central3.8243; first inbound
7000write planner benchmark8.3701–9.6218ms central8.9449. Bothrelease Criterion10
samples/warmup1s/measure2s, noDB/calc/network. Finalresultsfollow, notlatencygates.

Changed formula module/tests/bench/Cargo manifest, ADR080/roadmap and benchmark
report. No full browser relocation parity, servercommit/recovery, controlledp95,
manualExcel or fullMVP/deployment gate claimed. Nextslice needs distinct persisted
move/compensation representation, exact-head server command, candidate/CAS/ACK,
reference-aware undo guards, workerpreview and browser cut/paste.
Final ADR080 verification:12/12 relocation tests passed0.29s, including exact AST
relative offsets and untouched independent formula binding epoch. Fullformula
suite passed after borrowed-scan implementation (before those two final test-only
assertions/addition); final focused12/clippy passed afterwards. wasm32 build also
passed after scan change. No claim of browser relocation parity. Strictclippy,
rustfmt, selecteddoc Prettier and gitdiffcheck passed. No newdependency/lockchange.

Bench repeated because firstpostchange inbound regression needed investigation:
inbound9.1744–12.493ms then8.4866–8.8520ms central8.6696; uncontrolled variability
retained in report. Independent formulas before3.5623–4.1173ms; after2.2454–2.3825ms,
repeat2.3450–2.5138ms central2.4028. All10samples/1s/2s Criterion, optimizednative,
noDB/calc/network. Notp95 or general controlledspeedup. Runtime endpoints8080/
5173 returned200, existinglauncher notstopped. Partialrange/crosssheetmove refusal,
fullserver/wire/worker/UI integration and remainingphasegates explicitlyopen.
Read-only finalreview confirms clone/discard issue closed and no additional
concrete semantic defect found. Reviewer did not run tests; actual commandresults
above are from primaryagent. ADR080 remains acceptedforplanneronly.

## 2026-09-30 — ADR081 atomic durable cut/move

Authorized phase4 integration follows ADR080. Coordinated separate domain,
codec and frontend files; primary owns actor/journal/wire/tests/docs. Added
Operation/CommandPayload::MoveRange, frozen guardedsource/destination/inbound
footprint. Domain checksgeometry, area1..1000/writes<=10000, two512KiB textcaps,
exactpreimages/finalcapacity, scalar/blank frozenmapping and formula origins/
source/canonical/ASTshape/nonreference nodes/exactonce-mappedreferences/modes.
Read-only reviewcaughtmissinginboundfootprint; directcandidate RED reproduced,
thenboundedexternalformula validationfixed it. Finaldomainmove9/9passed, planner
12/12passed and domainclippy passed. Nonreference AST/source forgery also first
reproduced andfixed; formerfixture onlycheckedchangedAST and wasstrengthened.

Separate persisted schema19/oneof19/hash21/commandfingerprint17/principal migration
added; schemas1..18/goldensunchanged. Codec35/35 andclippy passed (6newtests,
4MiB envelope); oversizedfixtureinitiallybuiltunreachableAST, correctedreachable
fixturethenGREEN. Wirev1adds move_rangefield10 and dependencycode18, exactlyone
payload; malformedgeometrydecoder RED thenfixed, serverunit18/18passed.
Actor exactheadplan/candidate/CAS/durableACK path reused, late receipt precedes
newsource admission. Journaltouchedaddresses make Move a historybarrier; old
formula inverse acrosslaterMove isNotUndoable. AxisHistory recordscontentfree
relocationfloor; newactorsrestore boundedhistory evenwithoutaxischange before
oldformula binding. Delayedformula refuses17 ratherthanwrongtargetbind.

FrontendCtrlX/MetaX richintent and CtrlV/MetaV same-documentcross-tab MoveRange,
no canonicaloptimisticmove inthisslice. Sourcesremainbeforecommit/refresh; cell
textarea cut staysnative. Terminalstale17/partial18 do notfallbackcopy/TSV ornewID.
Reviewfoundtransientrejection droppinguncertainMove ID afterACKloss; twoREDtests
reproduced codes7/8, fixedretryables preserveexactbytes/id andblocknewcutuntilreceipt.
Fullfrontend98unit/typecheck/lintpassed; buildpassedbeforethatretry-onlycorrection.
Focused8/8ChromiumDPR1/2passed17.5s afterretryfix. Full106/106browserpassed5.8min;
no runtime restartuntilterminalexit0. FirstfocusedHTTPstartedbeforereadycausedone
ECONNREFUSED; thenfixtureframecase/path mistakescorrected (documentHeadChanged;
formula value.value.current_text). FocusedHTTP4/4passed5594.0396ms. Complete
HTTP/security/network74/74passed106532.9433ms onlivePG/server, includingexisting
3x100network writes. Nativecollab22passed/twointentionaltimingignored; newnative
incrementalMove graph testpassed1/1, proves2dirtyformulas/independentcachedvalue.

RealPGseparatejournalMove tests2/2passed1.49s (agent), covering schema19, late
CommitRequest receipt afterlaterhead, pagedreplay/frozenvalue, changedgeometry-ID
conflict, stalecommandbase/systemprincipal nonwrite; rerunafterfinaldomainhardening
pendingbelow. Sharedalltargetsclippy andSQLxpreparecheckpassedbeforelatejournal/
domaininboundfix; finalchecksbelow. Migration appliedthroughactualserverstartup.

Newoptimizedcandidate benchmark1000cells/5000inboundformulas(7000guardedwrites)
45.205–47.698ms central46.411,10samples/1s warmup/requested2s(55iterations,
actualestimated2.5694s). Excludesplanning,hash,calculation,DB/network/browser; developer
machinealso ranChromium,uncontrolledload. Notp95commit. Plannerbenchmarkprevious
8.4866–8.8520ms remainsdifferentmeasurement. AddedADR081, synchronizeddocs/
operationcaps andpresentationphase4line; discoveredstaleproposedcaps corrected
fromactualcode(64actorqueue,128pending,160frames/2MiB outbound,4MiBpersisted).
Unifieddecoded-memoryaccounting explicitlyopen ratherthanclaimed4MiBcap.

Ownedhiddenlauncher6120 identityverified/stoppedonlyafterfull106browserterminal,
forWindows-safe finalbinaryrecoverybuild. Recovery3cases nowrunning; noresult
claimedyet. Runtime/datarestorationrequiredafterterminal. Moveundo,styles/merges,
cross-sheetmove,partialrangeunion,WASMmovepreview/parity,controlledp95/manualExcel,
fullMVP/deploymentgatesremainopen. No fullgoalcompletion claimed.
Final ADR081 outcomes: full106Chromium passed5.8min before stoppingown6120. Final
recoverybinary built15.49s; complete3/3hardrestarttests passed13631.4733ms, including
frozenMove content/calculatedhash, late receipt aftersourceedit, anddelayedformula
rebase refusal afterfreshactor. FinalPGMove2/2rerunpassed1.01s afterinboundhardening.
Hiddenlauncher32120 restored: WASMrelease25.30s/serverdev7.82s, readyz8080 andweb5173
returned200. PostgreSQL18.6/dataretained; no unrelatedprocess stopped.

Final sharedstrictall-targetsclippy andSQLxpreparecheckpassed after lateRustfixes;
proto:checkpassed, defaultservercheckpassed. Full domain allfeatures suitepassed
withoneexisting convergence stressignore; full formula suitepassed includingnew
incrementalMove test. Frontendfinalbuildpassedafterretryfix. Rebuiltordinaryshared
native/browser corpus554cases and1010workbooks/2020patches matchednormalizedhashes;
thisdoesnotproveMove planner/workerpreview parity (notimplemented). FinalHTTPMove
5/5passed6507.5652ms afterruntime rebuild, including deleted-sourcecode9 andzero/
oversizegeometrycode3. FullHTTP74was earlierfour-Move-test invocation; nofull75
one-runclaimed. SelectedPrettier/rustfmt/diff checks passed. Presentationactual
Chromium1440x900/390x84410slides,updatedmove/undo-openline,Home/End/nooverflow/errors
passed; checker initially used inputValue on output element, fixedcheckertextContent.

Finalread-onlyreview confirms omitted-inbound gap closed and noadditionalconcrete
criticalfinding; reviewer didnotruntests. Scope remains exacthead same-sheetcontent
Move with cross-sheetinboundrefs. Partialrangeunion/cross-sheetmove/styles/merges,
Moveundo/referenceguards,workeroptimisticMove/parity,controlledlatency/manualExcel,
largecombinedconvergence/LINUXCompose/remoteCI/RussianS3/fullXLSX remainopen.
Allstagesgoalactive. Nextminimalstage isversionedguardedMovecompensation+newreference
historyguards (schema18limitsmustnotbechanged), then sharedworkerpreview/parity.
