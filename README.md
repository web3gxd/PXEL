# PXEL

Самописанный collaborative spreadsheet. Работают создание документа, sparse
Canvas, ввод значений, durable PostgreSQL CAS commit и восстановление после
reload/restart. Формулы сохраняются как source и серверный bound AST.
Асинхронный серверный пересчет доставляется по WebSocket отдельными
`CalculationCommitted` сообщениями с версией, ограниченными частями и итоговым
хэшем; при совпавшей клиентской calculation version сервер отправляет только
changed/cleared results, при cache miss — полный snapshot. Stateful WASM Worker показывает отдельно помеченный предварительный расчет,
передавая cell patches после начальной загрузки. CalculationContext теперь
записывается отдельным системным событием и восстанавливается из журнала;
полный контекст теперь передаётся через авторизованную HTTP-проекцию в WASM
Worker (init schema 5; старые readers сохранены). `RAND`/`NOW`/`TODAY` используют этот сохранённый контекст:
при первом сохранении volatile-формулы сервер создаёт его автоматически.
Дальнейшие правки содержимого автоматически создают следующий epoch.
У открытого документа NOW обновляется по heartbeat не чаще раза в минуту,
TODAY — при смене UTC-дня; RAND-only книги не получают таймерных событий.
Новая WebSocket-подписка, переход к другой книге или reconnect обновляет общий
volatile-контекст, включая RAND. Уже открытые вкладки получают тот же результат.
Повтор Subscribe активной книги и обычные HTTP-чтения не запускают новый epoch.
Две открытые вкладки получают подтвержденные события по WebSocket. При
переподключении сервер воспроизводит из PostgreSQL до 128 операций по порядку,
фиксирует `ReplayComplete` и затем доставляет новые версии. Больший разрыв или
системное событие вызывает авторизованный resync. Временное presence показывает
удалённое выделение, не меняя версию документа. Отзыв документного ACL закрывает
старую подписку через durable access epoch и локальный send gate. Полный gate
фазы 3, crash/recovery matrix snapshots и production S3 ещё впереди.
Ограниченный XLSX profile-1 уже проходит import/export/reimport через PostgreSQL;
широкая Excel-совместимость и upload недоверенных файлов остаются отдельными gates.
Локальные
snapshots уже проходят проверенную активацию и используются при запуске actor;
после каждых 1000 операций запускается фоновая запись. У каждого WebSocket есть очередь
отправки с лимитами 160 кадров / 2 MiB; медленный клиент отключается,
а фактическая отправка снова проверяет ACL.

Заголовки строк и столбцов остаются закреплёнными над Canvas. Доступные
пользовательские полосы прокрутки дают больше точности в заполненной области
разреженного листа; wheel, touchpad и клавиатура перемещают лист линейно. См.
[ADR-091](docs/adr/091-adaptive-axis-scrollbars.md).

Строки и столбцы имеют stable IDs; доступны вставка/удаление, TSV, копирование
и атомарный cut/move содержимого внутри листа. Собственная история undo/redo
охватывает значения, формулы, вставки TSV, копии и переносы диапазонов, явные
вставки и удаления строк и столбцов. Отмена
создаёт новую committed version и отклоняет конфликт с чужими изменениями или
новыми зависимостями. Восстановленные IDs имеют durable activation floors:
старая команда не записывает в восстановленную строку. По
[ADR-086](docs/adr/086-durable-author-history.md) реализовано восстановление
общей истории автора в документе после reload/reconnect, включая действия из
других вкладок. Это до 100 целей Undo и 100 целей Redo; возможность выполнить
конкретную отмену сервер проверяет при команде. Этот срез локально проверен на
реальном PostgreSQL, HTTP/WebSocket, в браузере и при аварийных перезапусках;
весь gate фазы 4 не закрыт. Объединённые ячейки реализованы для ограниченного
профиля; полный XLSX и общие нагрузочные/deployment gates остаются открытыми.
Перенос получает отдельный
WASM preview содержимого и формул до подтверждения сервера: shared Rust planner,
геометрический запрос, отдельный pending overlay и сброс при конфликте/resync.
Native/browser corpus переноса: 209 книг / 418 операций, exact results и rendered
formula text. [Контракт](docs/adr/084-optimistic-move-worker.md).

По [ADR-087](docs/adr/087-cell-formatting.md) добавлены 16 свойств оформления:
шрифт, цвет, заливка, границы, выравнивание, перенос текста и ограниченные числовые
форматы. Они хранятся отдельно от содержимого, включая пустые оформленные ячейки;
очистка значения сохраняет стиль. Команда атомарно меняет до 16 свойств у 1000
явно указанных ячеек. Доступны Undo/Redo и восстановление истории после reload,
в том числе отмена оформления исходных ячеек после вставки между ними новой строки.
Удаление строк и столбцов сохраняет оформление в durable preimages с guarded
Undo/Redo ([ADR-092](docs/adr/092-structural-style-preimages.md)).
GFM alignment из Markdown-таблиц импортируется вместе со значениями одной
durable batch operation и одной отменой ([ADR-093](docs/adr/093-atomic-cell-content-style-batches.md)).
Canvas и панель оформления проверены в 8 браузерных сценариях при DPR 1/2.
В WASM передаются точный bound AST и authored metadata; самостоятельно вычисленный
хэш книги сравнивается с серверным отдельно от результатов формул.
[Native/browser corpus: 1027 книг, 2037 patches](docs/benchmarks/2026-09-30-style-worker-parity-local.md).
Общая браузерная регрессия прошла 150/150 сценариев при DPR 1/2; отдельно
прошло восстановление стилей после двух жёстких перезапусков. Затем sparse
formatting добавлен в атомарные durable CopyRange/MoveRange operations с Undo
([ADR-094](docs/adr/094-styled-range-copy.md),
[ADR-095](docs/adr/095-styled-range-move.md)). Новые проверки запускаются
отдельно и записаны в RUNBOOK.
Числовое отображение использует фиксированный профиль en-US, DateIso — систему
1900 с явным serial 60. Неподдерживаемая дата показывает `#FORMAT!`; произвольные
Excel format strings и отображение системы 1904 не заявляются.

Архитектура принята только для фазы 0. Все решения, проверки и ограничения:
[RUNBOOK.md](RUNBOOK.md), [roadmap](docs/roadmap.md), [architecture](docs/architecture.md).

Листаемая презентация о продукте, архитектуре и фактических замерах:
[presentation/index.html](presentation/index.html). Она открывается локально
без сервера; стрелки и PageUp/PageDown переключают слайды.

Синтетический XLSX fixture проверяется без публикации документа:

```sh
cargo run --locked -q -p pxel-server -- xlsx-preview fixtures/xlsx/minimal_scalars.xlsx
```

Команда запускает отдельный дочерний процесс, принимает от него bounded IR,
повторно проверяет модель в родителе и печатает только количество листов и
ячеек. OS sandbox и загрузка XLSX в редактор ещё не реализованы. Владелец документа
может скачать подтверждённую книгу кнопкой «Скачать XLSX» или через
`GET /api/documents/{id}/export.xlsx`; права и durable audit проверяются до ответа.
По умолчанию экспорт доступен владельцу. «Выдача и аудит» задаёт правила книги
и отдельных гостевых ссылок; участники и гости получают экспорт только после
явного разрешения владельца. Правила пересекаются и не дают права правки.
Копирование и вырезание также требуют durable authorization; отказ не меняет
системный буфер или вырезанный диапазон. Требование обезличивания пока блокирует
выдачу до появления проверенной безопасной копии (ADR-108).
Поддерживаются scalar values/formulas и ограниченный профиль из 16 стилей ячейки,
включая оформление пустых ячеек. XLSX styles import/export/reimport прошёл локальную
PostgreSQL recovery и browser download проверку при DPR 1/2; bounded merged-cell
OOXML import/export и durable Undo/Redo также локально проверены в ADR-096;
подробный контракт —
в [ADR-088](docs/adr/088-xlsx-cell-style-profile.md) и
[handoff](docs/handoff/xlsx-styles-roundtrip.md). Импорт остаётся локальной CLI
командой с явной политикой потерь; browser upload, широкая Excel совместимость
и произвольные форматы не входят в завершённый профиль. Merge import/export
поддерживает bounded rectangular ranges и отклоняет hidden non-anchor data;
unbounded OOXML extensions остаются неподдерживаемыми. Неподдерживаемый
export profile по-прежнему возвращает typed ошибку.

Для локального dev-стенда scalar XLSX с 1–64 листами можно атомарно превратить
в новый документ после `just infra-up`/`just infra-native`. Возьмите opaque
`workspace_id` и `actor_id` из `GET /api/dev/identity` своей dev-сессии:

```sh
PXEL_DEV_AUTH=1 cargo run --locked -q -p pxel-server --features dev-auth -- xlsx-import fixtures/xlsx/single_sheet_scalars.xlsx <workspace_id> <actor_id>
```

Команда проверяет членство в workspace в PostgreSQL, печатает только ID нового
документа, версию и число событий. Совокупный лимит — 1000 заполненных ячеек.
В редакторе можно переключаться между импортированными листами по вкладкам;
кнопка «Добавить лист» создаёт новый пустой лист через durable операцию;
формулы в многолистовой книге показывают подтверждённый серверный результат
и отдельный локальный WASM preview, включая проверенные межлистовые зависимости.
Если XLSX содержит свойства, которые scalar-модель пока не
сохраняет, команда выводит типизированные коды и не создаёт документ. Локальный
оператор может явно добавить `--allow-loss` последним аргументом, чтобы
импортировать поддержанные ячейки с потерей перечисленных свойств. Веб-загрузка
будет добавлена после OS sandbox, malware scan и полноценного import report.

Формульный корпус и реальный browser Worker проверяются `just formula-parity`.
Поддержаны AND/OR/NOT с диапазонами и typed resource limits. Текущий engine
identity — `pxel-formula-0.16.0`; числовые литералы формул ограничиваются 15
значащими десятичными цифрами по наблюдению Excel (ADR-101). INDEX поддерживает
выбор из contiguous range и ссылочные аргументы агрегатов (ADR-070). Dynamic
spill и multi-area ещё не поддержаны.
MATCH выполняет exact/wildcard и sorted approximate lookup в одномерных
диапазонах (ADR-071, provisional blank/error/collation profile).
VLOOKUP/HLOOKUP выбирают scalar result из contiguous table, с exact/wildcard
и sorted approximate режимами (ADR-072; edge-case Excel verification ещё впереди).
XLOOKUP поддерживает ссылочные результаты, forward/reverse и проверенный sorted
binary search, ближайшие значения и wildcard mode (ADR-073). Spill и сочетание
wildcard с binary search пока отклоняются как неподдерживаемые.
SUMIF/COUNTIF поддерживают сравнения и wildcard-критерии; расширенный диапазон
SUMIF от ячейки-якоря участвует в инкрементальных зависимостях (ADR-074).
Пограничные правила совместимости ещё требуют ручной проверки в Excel.
SUMIFS/COUNTIFS пересекают до127 критериев; все диапазоны должны совпадать по
числу строк и колонок (ADR-075). Лимиты применяются и к несовпавшим ячейкам.
Доступны DATE/YEAR/MONTH/DAY в календарном
профиле ADR-063 (книги редактора используют 1900; чистый evaluator проверен также
для 1904; выбор системы книги ещё впереди). NOW/TODAY/RAND работают в явно
сохранённом epoch по ADR-065; ADR-066 добавляет автоматический первый контекст.
Новое «Пересчитать» записывает следующий epoch.
Доступны CONCAT/LEFT/RIGHT/MID/LEN/TRIM/LOWER/UPPER
в Unicode profile по ADR-062; POWER и `^` принимают дробные степени по ADR-061;
доступны ROUND/ROUNDUP/ROUNDDOWN в ограниченном
decimal profile по ADR-060; «Пересчитать» в меню или авторизованный
`POST /api/documents/{id}/recalculate` с новым `operation_id` создаёт
следующий durable epoch без переписывания журнала. Старый несовместимый контекст
volatile-книги обновляется сервером при чтении. Повтор кнопки после потери
HTTP-ответа использует прежний ID до получения подтверждённой версии.
Нужны wasm32 target, wasm-bindgen-cli **0.2.129** и установленный Playwright Chromium.
Команда собирает WASM из исходников; generated binaries не хранятся в Git.

После запуска локального backend/PostgreSQL сетевую сходимость двух клиентов
проверяет `pnpm test:network-convergence` (3×100); полный профиль задаётся
`PXEL_NETWORK_SEEDS=100` и `PXEL_NETWORK_COMMANDS=1000` для той же команды.
Проверка использует настоящие WebSocket и PostgreSQL. `pnpm test:recovery`
дополнительно проверяет сходимость двух авторов после жёсткого рестарта;
случайная нагрузка вместе с отзывом прав остаётся отдельным gate.

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
Для нового dev-хранилища setup также создаёт приватный keyring в
`.local/keys/keyring.json`; это ключи конкретного локального стенда. Существующее
plaintext-хранилище требует offline-миграции в отдельный кластер: обычный запуск
отказывается переписывать его на месте. Срез миграции и его проверки описываются
в RUNBOOK по мере реализации.
Откройте <http://127.0.0.1:5173>. Ctrl+C останавливает оба процесса приложения.
PostgreSQL остается запущенным; `docker compose down` останавливает его с сохранением volume.

Локальная проверка совместной работы использует отдельные dev accounts.
`GET /api/dev/identity` показывает opaque actor/workspace IDs текущей сессии.
Владелец книги может вызвать `POST /api/documents/{id}/permissions` с
`{"action":"grant","change_id":"<uuidv7>","target_user_id":"<uuidv7>","role":"viewer"}`
или `action="revoke"` без `role`. Grant приглашает существующего пользователя
в workspace; при следующем dev login он передает `X-PXEL-DEV-WORKSPACE` с ID
workspace владельца. ACL-команда требует cookie, разрешённый Origin и роль
owner; в production dev login отключен. В редакторе есть владельческий диалог
защищённых гостевых ссылок и отдельные настройки выдачи с журналом попыток.

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
just share-log-redaction # synthetic PIN/password/token/cookie отсутствуют в application logs
just s3-encrypted-snapshot # S3-backed snapshot ciphertext survives server hard restart
just distributed-share-revocation # idle guest WebSocket closes across two server processes
pnpm test:recovery # собирает dev-auth binary и проверяет hard restart
just crash-recovery-postgres # отдельный PostgreSQL 18: immediate stop, WAL recovery, snapshot+tail
just backup-restore-postgres # pg_basebackup + pg_verifybackup + snapshot objects → новый кластер
just pitr-restore-postgres # локальный WAL archive и named restore point между v4 и v5
just bench-collab # отдельный локальный замер ACK и доставки второму клиенту
```

До `check/test/sqlx-check` задайте `DATABASE_URL` из `.local/database-url` без вывода
в логи (`env.ps1` делает это локально). Для Playwright установите browser:
`pnpm --filter @pxel/web exec playwright install chromium`;
Linux CI использует `--with-deps`. Порты 8080/5173 перед `just smoke` должны быть свободны.
Cargo-nextest закреплен на 0.9.146. `cargo test --workspace --all-features`
также работает без nextest. SQLx compile-time metadata хранится в `.sqlx/`;
`SQLX_OFFLINE=true cargo build --locked --workspace` не требует БД при компиляции.
`just share-log-redaction` запускает отдельный loopback сервер с dev-auth и требует
`DATABASE_URL` из изолированной acceptance PostgreSQL и `PXEL_KEYRING_FILE` с
активными write generations. Тест генерирует краткоживущие секреты в памяти,
проверяет неудачный и успешный unlock для PIN и password, затем ищет их, link
token, URL и guest cookie в захваченном stdout/stderr сервера.
Если `PXEL_TEST_POSTGRES_LOG` указывает на лог того же изолированного кластера,
тест также проверит только записи, добавленные за время сценария.
`just s3-encrypted-snapshot` требует изолированные `DATABASE_URL`,
`PXEL_KEYRING_FILE`, `PXEL_S3_TEST_ENDPOINT`, `PXEL_S3_TEST_BUCKET`,
`PXEL_S3_TEST_REGION` и тестовые AWS SDK credentials. Для извлечения объекта из
S3-compatible test endpoint нужен Python с boto3 (`PXEL_TEST_PYTHON`; по умолчанию
`.local/moto-venv/Scripts/python.exe` на Windows). Тест проверяет envelope и
ciphertext checksum самого объекта, отсутствие контрольного style value и
восстановление snapshot+tail после hard restart.
`just distributed-share-revocation` использует те же изолированные
`DATABASE_URL` и `PXEL_KEYRING_FILE`; тест запускает два локальных server process
против общей PostgreSQL и проверяет закрытие idle guest WebSocket и отказ чтения
после revoke на первом экземпляре.
`crash-recovery-postgres` требует `PXEL_PG_BIN`, создает отдельный кластер под
`.local`, включает `synchronous_commit`, `fsync` и `full_page_writes`, затем
удаляет кластер после остановки. Это проверка аварийного завершения процесса БД,
а не физической потери питания, диска или backup/PITR.
`backup-restore-postgres` также использует только отдельные локальные кластеры.
Он останавливает приложение на время копирования blob-каталога и физического
backup, проверяет manifest, затем восстанавливает PostgreSQL и blobs вместе.
Это restore drill для согласованной копии; WAL archiving/PITR и резервирование
S3 в присутствии писателей остаются отдельными production gates.
`pitr-restore-postgres` проверяет локальный WAL archive и named restore point:
после v4 и до v5 создаётся маркер, а восстановленная копия должна содержать
ровно v4. Архиватор использует простую локальную команду копирования только
в изолированном тесте; production archiver, off-site retention и RPO/RTO
этим не проверены.

GitHub Actions выполняет те же команды на Ubuntu 24.04. Наличие workflow
не означает выполненный remote CI: фактический статус записывается в RUNBOOK.

## Конфигурация и границы

| Переменная                                | Назначение                                                                                                   |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `DATABASE_URL` / `PXEL_DATABASE_URL_FILE` | DB URL из environment или файла; default file `.local/database-url`                                          |
| `PXEL_KEYRING_FILE` / `PXEL_KEYRING_JSON` | Ровно один источник keyring schema 1; приватный файл вне PGDATA, blobs и backup/restore. Нет fallback-ключа. |
| `PXEL_BIND`                               | Backend bind; default `127.0.0.1:8080`                                                                       |
| `PXEL_ALLOWED_ORIGINS`                    | Список точных Origin через запятую, без wildcard                                                             |
| `PXEL_DEV_AUTH`                           | `1` работает только с compile feature `dev-auth` и loopback bind                                             |
| `PXEL_BLOB_ROOT`                          | Private writable directory; default `.local/blobs`                                                           |
| `PXEL_SNAPSHOT_INTERVAL`                  | Число committed событий между snapshots; default `1000`, предел `1..100000`                                  |
| `PXEL_SNAPSHOT_CLEANUP_HOURS`             | Возраст incomplete snapshot до уборки; default `24`, предел `1..8760`                                        |
| `PXEL_SNAPSHOT_ACTIVE_RETENTION_DAYS`     | Возраст старых ACTIVE snapshots до вывода; default `30`, предел `1..3650`; два новых поколения сохраняются   |
| `PXEL_S3_ENDPOINT`                        | При сборке с `--features s3` выбирает S3 adapter; HTTPS вне loopback                                         |
| `PXEL_S3_BUCKET` / `PXEL_S3_REGION`       | Обязательны вместе с endpoint; credentials задаются через стандартные AWS SDK providers                      |
| `PXEL_DB_PORT`                            | Порт только локального bootstrap; default `55432`                                                            |
| `PXEL_PG_BIN`                             | Каталог binaries для native bootstrap                                                                        |

Для проверки S3 endpoint задайте `PXEL_S3_TEST_ENDPOINT`,
`PXEL_S3_TEST_BUCKET` и SDK credentials, затем выполните `just s3-contract`.
Команда запускает восемь сетевых тестов. Локальный прогон на Moto 5.2.3 не
заменяет отдельные прогоны на Garage и production S3.

`GET /healthz` проверяет процесс, `GET /readyz` — подключение, точную версию/checksum
миграций и writable blob root. Dev login требует проверенный Origin, создает
HttpOnly SameSite cookie. `/api/ws` принимает protobuf v1 Hello, выдает Welcome,
принимает document commands и versioned calculation requests. Команды коммитятся
перед подтверждением; результаты формул доставляются отдельным ограниченным
потоком. Лимит handshake 1 KiB, deadline 5 s, 128 connections, heartbeat 15 s /
timeout 45 s. Development session можно адресно отозвать через
`DELETE /api/dev/session`.

`GET /api/documents/{id}/versions/{version}` возвращает авторизованную
историческую cacheless projection подтверждённой версии `1..head`. Snapshot
используется только если он не новее запрошенной версии; при отсутствии
валидного snapshot применяется журнал. Маршрут ограничен двумя параллельными
чтениями и 30-секундным таймаутом восстановления. UI просмотра прошлых версий
документа пока не добавлен.

`GET /api/documents/{id}/undo-history` восстанавливает историю текущего автора
из проверенного журнала. Требуется действующее право owner/editor. Ответ schema 1
содержит только IDs, версии, два ограниченных стека и признак усечения; значения
ячеек, формулы и preimages не передаются. Чтение ограничено последними 10 001
записями, включая цель и до 10 000 последующих версий, и 64 MiB payload.
Стек прежней отдельной вкладки не сохраняется: история общая для автора и
документа. Подробности и статус финальной проверки — в ADR-086 и RUNBOOK.

Без `dev-auth` login не доступен и WebSocket не авторизуется. Это безопасный skeleton,
а не готовая production auth. В проекте нет обязательной внешней телеметрии.
MinIO/Garage не запускаются; локальные snapshots относятся к фазе 5, S3 adapter ещё впереди.
Ограничения локального blob adapter: [storage README](crates/spreadsheet-storage/README.md).

## Репозиторий

Из библиотеки доступен «Импорт CSV»: UTF-8, до 16 МиБ / 100 000 полей /
1000 столбцов / 32 КиБ на поле. Поддерживаются запятая, точка с запятой,
табуляция, кавычки и переносы строк внутри кавычек. По умолчанию все значения
текстовые; числа и TRUE/FALSE выбираются явно для столбцов. Предпросмотр не
создаёт книгу, публикация атомарна, повтор после потери ответа идемпотентен.
`POST /api/imports/csv/preview` и `POST /api/imports/csv` требуют текущую member
сессию и разрешённый Origin; DTO schema1 описан в ADR-128.

Проверка с отдельными PostgreSQL/keyring/blob root и портами (сначала выполнить
`. ./scripts/env.ps1` на Windows и собрать `cargo build --locked -p pxel-server
--features dev-auth`): `node --test tests/recovery/csv-import.test.mjs`.
С `PXEL_CSV_VERIFY_WORKSPACE=1` этот fixture также запускает `just check` на
своей базе, сохраняя приватный `.local/csv-acceptance-*/workspace-check.log`.
План всех десяти улучшений: [functional plan](docs/sheets-functional-plan.md).

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

XLSX crate используется безопасным разбором и worker preview; пустых crates
«на будущее» нет. MSRV policy до production v1 не установлена.
Toolchain и временное lint exception: [web tooling](apps/web/TOOLING.md).
