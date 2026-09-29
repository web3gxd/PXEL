# PXEL

Самописанный collaborative spreadsheet. Работают создание документа, sparse
Canvas, ввод значений, durable PostgreSQL CAS commit и восстановление после
reload/restart. Формулы сохраняются как source и серверный bound AST.
Асинхронный серверный пересчет доставляется по WebSocket отдельными
`CalculationCommitted` сообщениями с версией, ограниченными частями и итоговым
хэшем. Stateful WASM Worker показывает отдельно помеченный предварительный расчет,
передавая cell patches после начальной загрузки. CalculationContext теперь
записывается отдельным системным событием и восстанавливается из журнала;
`RAND`/`NOW`/`TODAY` еще не включены до проверки общего native/WASM контекста.
Две открытые вкладки получают подтвержденные события по WebSocket. При
переподключении сервер воспроизводит из PostgreSQL до 128 операций по порядку,
фиксирует `ReplayComplete` и затем доставляет новые версии. Больший разрыв или
системное событие вызывает авторизованный resync. Временное presence показывает
удалённое выделение, не меняя версию документа. Отзыв документного ACL закрывает
старую подписку через durable access epoch и локальный send gate. Полный gate
фазы 3, snapshots и XLSX ещё впереди.

Архитектура принята только для фазы 0. Все решения, проверки и ограничения:
[RUNBOOK.md](RUNBOOK.md), [roadmap](docs/roadmap.md), [architecture](docs/architecture.md).

Листаемая презентация о продукте, архитектуре и фактических замерах:
[presentation/index.html](presentation/index.html). Она открывается локально
без сервера; стрелки и PageUp/PageDown переключают слайды.

Формульный корпус и реальный browser Worker проверяются `just formula-parity`.
Нужны wasm32 target, wasm-bindgen-cli **0.2.129** и установленный Playwright Chromium.
Команда собирает WASM из исходников; generated binaries не хранятся в Git.

## Локальный запуск

Нужны Rust из `rust-toolchain.toml`, Node 24.18+, pnpm 12.6.0, just 1.58.0
и Docker Compose. Linux — основной deployment target; Windows поддерживается
через Docker/WSL или отдельную локальную PostgreSQL. Production authentication
и deployment в российском контуре относятся к следующим этапам.

```sh
pnpm install --frozen-lockfile
just infra-up
just dev
```

`just infra-up` создает случайные локальные credentials в игнорируемой `.local/`,
поднимает PostgreSQL 18.6 на `127.0.0.1:55432`, применяет миграции.
`just dev` запускает backend `127.0.0.1:8080` и frontend `127.0.0.1:5173`.
Откройте <http://127.0.0.1:5173>. Ctrl+C останавливает оба процесса приложения.
PostgreSQL остается запущенным; `docker compose down` останавливает его с сохранением volume.

Локальная проверка совместной работы использует отдельные dev accounts.
`GET /api/dev/identity` показывает opaque actor/workspace IDs текущей сессии.
Владелец книги может вызвать `POST /api/documents/{id}/permissions` с
`{"action":"grant","change_id":"<uuidv7>","target_user_id":"<uuidv7>","role":"viewer"}`
или `action="revoke"` без `role`. Grant приглашает существующего пользователя
в workspace; при следующем dev login он передает `X-PXEL-DEV-WORKSPACE` с ID
workspace владельца. ACL-команда требует cookie, разрешённый Origin и роль
owner; в production dev login отключен. Пользовательского sharing dialog пока нет.

Без Docker: установите PostgreSQL 18.6 binaries и задайте `PXEL_PG_BIN`, затем
вместо `infra-up` выполните `just infra-native`. Команда создает отдельный cluster
в `.local/postgres`; существующие системные службы PostgreSQL не изменяются.
Не запускайте native и Compose database одновременно на одном порту.

На подготовленном Windows-стенде из PowerShell сначала выполните:

```powershell
. .\scripts\env.ps1
just infra-native
just dev
```

`env.ps1` использует изолированные инструменты `.tools/`, если они установлены;
они не входят в репозиторий. Обычная установка toolchain в PATH также поддерживается.
Для Rust Windows GNU нужен совместимый GCC; для Windows MSVC — Visual Studio Build Tools.

## Проверки

```sh
just check       # fmt, clippy (default/dev-auth), tests, native/WASM check, frontend
just test        # nextest + Vitest; PostgreSQL должна работать
just sqlx-check  # sqlx-cli 0.9.0; сверка .sqlx с реальной PostgreSQL
just smoke      # запускает/останавливает приложение; HTTP/WS + Playwright
just audit      # cargo-deny 0.20.2, cargo-audit 0.22.2, pnpm audit
pnpm test:recovery # собирает dev-auth binary и проверяет hard restart
```

До `check/test/sqlx-check` задайте `DATABASE_URL` из `.local/database-url` без вывода
в логи (`env.ps1` делает это локально). Для Playwright установите browser:
`pnpm --filter @pxel/web exec playwright install chromium`;
Linux CI использует `--with-deps`. Порты 8080/5173 перед `just smoke` должны быть свободны.
Cargo-nextest закреплен на 0.9.146. `cargo test --workspace --all-features`
также работает без nextest. SQLx compile-time metadata хранится в `.sqlx/`;
`SQLX_OFFLINE=true cargo build --locked --workspace` не требует БД при компиляции.

GitHub Actions выполняет те же команды на Ubuntu 24.04. Наличие workflow
не означает выполненный remote CI: фактический статус записывается в RUNBOOK.

## Конфигурация и границы

| Переменная                                | Назначение                                                          |
| ----------------------------------------- | ------------------------------------------------------------------- |
| `DATABASE_URL` / `PXEL_DATABASE_URL_FILE` | DB URL из environment или файла; default file `.local/database-url` |
| `PXEL_BIND`                               | Backend bind; default `127.0.0.1:8080`                              |
| `PXEL_ALLOWED_ORIGINS`                    | Список точных Origin через запятую, без wildcard                    |
| `PXEL_DEV_AUTH`                           | `1` работает только с compile feature `dev-auth` и loopback bind    |
| `PXEL_BLOB_ROOT`                          | Private writable directory; default `.local/blobs`                  |
| `PXEL_DB_PORT`                            | Порт только локального bootstrap; default `55432`                   |
| `PXEL_PG_BIN`                             | Каталог binaries для native bootstrap                               |

`GET /healthz` проверяет процесс, `GET /readyz` — подключение, точную версию/checksum
миграций и writable blob root. Dev login требует проверенный Origin, создает
HttpOnly SameSite cookie. `/api/ws` принимает protobuf v1 Hello, выдает Welcome,
принимает document commands и versioned calculation requests. Команды коммитятся
перед подтверждением; результаты формул доставляются отдельным ограниченным
потоком. Лимит handshake 1 KiB, deadline 5 s, 128 connections, heartbeat 15 s /
timeout 45 s. Development session можно адресно отозвать через
`DELETE /api/dev/session`.

Без `dev-auth` login не доступен и WebSocket не авторизуется. Это безопасный skeleton,
а не готовая production auth. В проекте нет обязательной внешней телеметрии.
MinIO/Garage не запускаются; snapshots и S3 adapter относятся к фазе 5.
Ограничения локального blob adapter: [storage README](crates/spreadsheet-storage/README.md).

## Репозиторий

- `apps/server`, `apps/web` — executable adapters и UI.
- `crates/spreadsheet-domain` — workbook, stable IDs, sparse cells и операции.
- `crates/spreadsheet-application` — используемые persistence и blob ports.
- `crates/spreadsheet-formula`, `crates/spreadsheet-wasm` — общий Rust evaluator и браузерный adapter.
- `crates/spreadsheet-collab` — document actor и асинхронный расчет.
- `crates/spreadsheet-storage` — PostgreSQL и filesystem adapters.
- `crates/spreadsheet-storage-codec` — versioned durable event envelope.
- `crates/spreadsheet-wire-protocol`, `proto/` — независимый protobuf wire v1 и генерация Rust/TypeScript.
- `migrations/`, `tests/integration/`, `.sqlx/` — DB schema, live tests, checked query metadata.
- `docs/`, `docs/adr/` — актуальные контракты и решения.

XLSX crate появится вместе с первым использующим его срезом; пустых crates заранее
нет. MSRV policy до production v1 не установлена.
Toolchain и временное lint exception: [web tooling](apps/web/TOOLING.md).
