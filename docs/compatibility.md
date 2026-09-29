# Совместимость

Статус: матрица целевого профиля с фактически реализованным первым набором формул. Работающий редактор и движок есть; базовый phase 2 gate пройден локально. Фаза 6 читает ограниченный XLSX в отдельную scalar IR, экспортирует подтверждённую книгу владельца и создаёт документ из одного scalar-листа локальной dev-командой. Веб-загрузка ещё не доступна.

Google Sheets задает пользовательскую модель мгновенного редактирования и collaboration. Excel — целевая совместимость формул/файлов в явно ограниченном профиле, не источник runtime architecture. Excel/LibreOffice headless, готовые spreadsheet/formula/collaboration engines не входят в runtime.

## Formula profile

Точные правила и ограничения: [formula-semantics.md](formula-semantics.md). Convergence нашего native/WASM — exact normalized result hash. Совместимость с Excel проверяется fixtures с версией Excel/ОС/locale и указанным tolerance, где он необходим. Bitwise Excel parity не обещается.

| Статус целевого профиля      | Formula feature                                                                                                                                                                                                                                                    |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Supported после phase 2 gate | Literals, basic operators, A1/mixed references, bounded ranges, SUM/AVERAGE/MIN/MAX/COUNT/COUNTA/IF/IFS/IFERROR/AND/OR/NOT/ABS/MOD                                                                                                                                 |
| Partially supported          | CONCAT/LEFT/RIGHT/MID/LEN/TRIM/LOWER/UPPER: bounded Unicode-scalar profile (ADR-062); modern surrogate counting, locale-independent case and numeric formatting, legacy UTF-16/Excel locale verification open                                                      |
| Partially supported          | POWER / fractional exponent: portable binary64 profile (ADR-061), manual Excel precision verification open; ROUND/ROUNDUP/ROUNDDOWN: shortest-decimal profile (ADR-060); static dependency treatment IF; range caps; date system metadata и formatting ещё открыты |
| Preserved but not editable   | Unknown function при распознаваемом normalized AST: source/AST сохраняются, canonical result NAME; UI показывает unsupported feature                                                                                                                               |
| Dropped with warning         | Импортированные calculated caches не считаются canonical values; могут показываться только как untrusted preview                                                                                                                                                   |
| Rejected                     | Неподдержанная грамматика без lossless normalized representation; external/UDF refs; превышение limits                                                                                                                                                             |

Формула сохраняется только с server-validated normalized AST. Сохранить неизвестный произвольный текст вместо AST и молча считать cache корректным нельзя. Native/WASM version mismatch приводит к отключению соответствующего preview/reload, а не к доверию клиентскому AST.

DATE/YEAR/MONTH/DAY — partially supported по ADR-063: numeric serials,
Gregorian1900/1904 и fake1900-02-29, bounded normalization. Shared corpus337
проверен native/browser с обоими профилями, но текущая durable workbook model
использует1900. Workbook date-system metadata/import, locale date text/number
formatting и manual Excel goldens ещё открыты; NOW/TODAY включены только
с persisted explicit context по ADR-065.

## XLSX profile: phase 6

Фактически реализован локальный `xlsx-preview <path>`: дочерний процесс проверяет ограниченный ZIP/OOXML пакет, извлекает числа, boolean, простые shared/inline strings и исходный текст формулы в XLSX-specific IR; родитель повторно валидирует IR и выводит только счетчики. `xlsx-import` с dev-auth атомарно публикует новый документ с 1–64 листами и суммарно до 1000 scalar-ячеек после серверного binding формул. Формульный cache отбрасывается. `GET /api/documents/{id}/export.xlsx` для владельца записывает scalar projection подтверждённой книги; аудит фиксирует версию, размер и checksum до ответа. Живой scalar import/edit/export/reimport проверен на нашем синтетическом fixture и reader; формула `=A1+B1` становится эквивалентной `=(A1+B1)`. Excel-совместимость этим не доказана. Подробные лимиты и незакрытые границы — [ADR-043](adr/043-bounded-xlsx-package-inspection.md), [ADR-044](adr/044-normalized-xlsx-scalar-ir.md), [ADR-045](adr/045-xlsx-worker-framed-process.md), [ADR-046](adr/046-bounded-xlsx-worker-ir.md), [ADR-047](adr/047-scalar-xlsx-export.md), [ADR-048](adr/048-owner-audited-document-xlsx-export.md), [ADR-049](adr/049-atomic-local-xlsx-document-import.md). Таблица ниже описывает целевой профиль, не текущую полноту реализации.

Новый ограниченный отчёт по [ADR-050](adr/050-bounded-xlsx-compatibility-issues.md) помечает неизвестные package parts, metadata книги/листа и форматирование строк/ячеек типизированными кодами. Локальный import по умолчанию отказывает, если какие-либо из этих свойств были бы потеряны; `--allow-loss` разрешает оператору явно принять указанную потерю. Отчёт пока агрегирован и не заменяет полный per-feature compatibility report или проверку реальным Excel.

После [ADR-051](adr/051-durable-multiple-sheets.md) локальный импорт сохраняет 1–64 листа при суммарном лимите 1000 scalar-ячеек; бинарный snapshot schema 2 хранит их по порядку. [ADR-052](adr/052-active-sheet-ui.md) добавляет переключение листов в Canvas UI. [ADR-053](adr/053-multi-sheet-wasm-preview.md) включает локальный WASM preview межлистовых формул с общим native/browser hash для проверенного fixture; серверный расчёт остаётся каноническим. [ADR-054](adr/054-interactive-add-sheet.md) добавляет пустой лист через авторизованную WS-команду. Удаление, переименование и перестановка листов, стили и Excel golden parity открыты.

Синтетический `cross_sheet_formula.xlsx` также прошёл локальный import/edit/export/reimport с формулой `Plan!A1+2`: ложный cached result `999` не использован, а экспортный нормализованный текст `=('Plan'!A1+2)` повторно связан с целевым листом. Это один проверенный профиль, не общий Excel golden corpus.

ZIP → OOXML package relationships → streaming XML → NormalizedImportWorkbook → parent schema validation → server formula binding → canonical document publication. Export идет из согласованной document version через отдельную Export IR. XLSX DTO не становится внутренней workbook model.

| Статус                     | Feature и границы                                                                                                                                                                                       |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Supported                  | XLSX package, несколько worksheets, string/number/boolean values, shared strings, поддержанный formula text, row height/column width, basic fonts/fills/borders/alignment, merges/frozen panes          |
| Partially supported        | Number formats по allowlist; basic autofilter; простые defined names; formulas только accepted profile; partial styles с явным report                                                                   |
| Preserved but not editable | Распознаваемый unknown function AST с NAME result; безопасные metadata только по явной normalized schema                                                                                                |
| Dropped with warning       | Charts/drawings/pivot presentation и неподдержанные безопасные formatting extensions, если их удаление не скрывает запрещенную активную часть                                                           |
| Rejected                   | VBA/macros, Power Query, external links, Data Model, embedded OLE, encrypted files, malformed package/relationships, ZIP bombs, DTD/entities, неподдержанные dynamic arrays/UDF и недопустимые formulas |

Ни arbitrary OOXML passthrough, ни обещание полного round-trip не допускаются. Неподдержанная часть не может быть обозначена одновременно «сохранена» и «удалена»: report содержит part/feature/status/reason/location. Имена/содержимое report относятся к документу и доступны только через authorization, не обычные logs.

Импорт по умолчанию создает новый документ. Merge imported content в существующий — отдельный future use case. Import result не публикуется частично: job либо создает валидный canonical document, либо typed failure/compatibility report.

## Недоверенный файл

ZIP limits проверяют реально распакованные bytes, не только headers. Ограничены input size, entry count, expansion ratio, XML depth/attributes/text, relationships, row/cell/formula counts и elapsed budget. Запрещены traversal, duplicate ambiguous entries, external entities/DTD и network relationship fetch. Текущий parent убивает дочерний процесс по таймауту; завершение всей process group/Job Object и жёсткие CPU/RAM лимиты ещё не реализованы.

Linux production adapter: cgroups v2 memory/CPU, process timeout, FD limits, isolated private working directory и read-only input. Windows development adapter: Job Objects с kill-on-close/memory/time, private temp directory и restricted child environment. Child не получает DB/S3 credentials; stdin/stdout — versioned framed protocol с length caps. Parent повторно валидирует IR; exit 0 не доказывает безопасность данных.

Malware scanning interface добавляется с upload flow; он не требует внешнего SaaS и не заменяет parser limits. [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) не дает hard cancellation уже работающей задачи, поэтому отдельный процесс обязателен для этих лимитов.

## Протокол и fixtures

Wire compatibility: additive optional fields, reserved retired tags, explicit capability/major gates; unknown operation не применяется частично. Durable events имеют отдельные schema versions/read migrations. XLSX golden corpus хранит source provenance, license и ожидаемый compatibility report. Fixtures с реальными персональными данными не коммитятся.

Round-trip acceptance: supported workbook import → edit → export → import сохраняет поддержанные values/formulas/styles и сообщает об отличиях. Нельзя назвать функцию Excel-compatible на основании одной интуитивной формулы или отсутствия panic.

ADR-065: NOW/TODAY/RAND — Partially supported: persisted epoch, native/WASM convergence, numeric serial без автоматического date formatting. ADR-066 создаёт первый контекст автоматически; кнопка «Пересчитать» создаёт следующий epoch. Automatic open-renewal/timer/dependency triggers и ручные Excel goldens остаются открыты. RAND sequence является собственным deterministic profile, не Excel RNG parity.

ADR-066 снимает ручной bootstrap prerequisite для NOW/TODAY/RAND: первый durable контекст создаётся сервером автоматически. Статус всё ещё Partially supported до automatic dependency/timer epochs и Excel verification; compatible context/value сохраняется после reload.
