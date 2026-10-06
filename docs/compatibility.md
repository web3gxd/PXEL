# Совместимость

ADR-117: меню и общий стиль элементов редактора меняют только клиентский UI.
Команды, capabilities, URLs, wire/DTO/persisted versions сохранены. Отклик
WebSocket — клиентское измерение последнего Hello/Welcome, без новых сообщений
протокола. Навигация сохраняет существующий guard незавершённого ввода; окно
размеров использует общий Modal с удержанием и восстановлением фокуса.

ADR-116: current-workspace catalog/metadata/personal preference/layout DTO 1
независимы от wire и durable workbook codecs. Titles/favorite/recent зашифрованы
DEK; catalog использует текущие ACL и delivery lease. Личные настройки не меняют
book head/context/Undo. Templates и file import атомарно публикуют metadata;
file export проверяет metadata revision. Root — библиотека, прежние document/share
URL сохранены. PG pagination/CAS, template/restart и 12+8 browser cases прошли;
combined final gate пока записывается в RUNBOOK.

ADR-115: standalone encrypted .pxel file v1, independent header/DTO lifecycle.
POST-only password, pinned Argon2id v19 KDF 64 MiB/3/1 и XChaCha20-Poly1305.
Полностью проверенный import создаёт fresh-ID workbook/settings atomically;
history/comments/ACL/keys не переносятся. Safe files содержат только masked
values/settings; omitted filters явно указаны. Policy1 принимает новый формат
pxel, capabilities2 additive can_export_pxel; явные старые grants сохранены.
Crypto/adapter/restart, Chromium 12+2, 259 frontend/type/lint/Clippy прошли локально.
Combined final acceptance, NVDA и deployment gates открыты.

ADR-114: независимые activity/historical/diff DTO 1 поверх existing encrypted
journal и snapshot replay, без нового persisted/wire codec. Historical container
явно read_only; authored и derived results раздельны. Diff сравнивает stable IDs,
formula bindings/current text и отдельные свойства style/layout/structure.
Бюджеты ограничивают отчёт целиком. Pure/PG/restart, 12/12 Chromium DPR1/DPR2
и 257 frontend tests прошли локально; общие финальные gates/NVDA открыты.

ADR-113 добавляет отдельный redacted projection DTO 1 и encrypted privacy policy
DTO 1. Safe projection содержит layout и scalar values без formula source/AST,
authored hash/context/history. Clipboard receipt additive anonymized/projection;
XLSX использует прежний serializer со scalar безопасной копией. Privacy revision
обновляет disclosure stamp/access epoch без нового workbook event или snapshot
codec. Guest cell writes запрещены также в storage; view/comment разрешены.
Scanner — opt-in bounded heuristic. Native/PG/restart, 10/10 Chromium DPR1/DPR2
и 254 frontend tests прошли локально; финальные общие gates остаются открыты.

ADR-112: owner profile DTO 1, fixed head/context, read_confirmed без context
initialization, clock-free observer в formula kernel и отдельный server Instant
executor. Profiling не меняет formula engine version, results, hash, head,
epoch или Undo/Redo; browser сравнил hash с normal calculation. Potential graph
усекается до 500/2000, slow list до 1000. Native/actor/server, 12/12 Chromium,
251 frontend tests прошли локально. Финальные общие gates/NVDA открыты.

ADR-111 добавляет diagnostics DTO 1 и WASM diagnoseFormula с diagnostic UTF-8
spans. В браузере позиции переводятся в UTF-16 без изменения текста. Эвристика
100/90% и персональные encrypted dismissals не меняют wire/durable book codecs,
formula engine version, calculated values или Undo/Redo. Native/WASM/PG/restart
и 10/10 Chromium прошли локально; общий финальный smoke остаётся открытым.

Дополнение 2026-10-05: [ADR-108](adr/108-disclosure-policies-and-audit.md)
добавляет schema-1 политики выдачи и audit DTO, schema-2 capabilities с правами
копирования и режимом представления. Исходный default XLSX — владельцу;
участники получают экспорт только после владельческого разрешения, гости —
после пересечения разрешений книги и ссылки. XLSX serializer/profile не изменён.
Clipboard пишет те же TSV/HTML/formula/range данные после durable authorization;
внутренние payloads переносятся в escaped HTML table attribute, legacy custom
MIME остаются читаемыми. Это не меняет wire/durable workbook codecs.
Требование обезличивания использует безопасную проекцию ADR-113 и блокирует
выдачу при отключённом распознавании или превышении бюджета. Формат `.pxel` и остальные дополнения из нового
плана на момент ADR-108 ещё не заявлялись поддержанными; текущий статус — ADR-115/116.

ADR-109 добавляет shared frozen rows (0–1000), independently encrypted personal
filter DTO 1 и подтверждённую геометрию скрытых строк. Durable event writer 36,
snapshot writer 7/readers 1–7, authored projection 7 и Worker init profile 5
версионируются отдельно от additive wire 1. Worker init 5 переносит размеры,
объединения и закрепления в authored hash; старые profiles 1–4 сохранены. Политика
личных фильтров не меняет authored hash, calculation context или Undo/Redo.
Начальная поддержка text comparisons — case-sensitive Unicode, числа не
сравниваются с текстом через coercion. Windows baseline check/sqlx/audit/parity и full Chromium 224/224 прошли;
integration 77/77, recovery 4/4. Финальный exact smoke и NVDA пока открыты.

ADR-110: SortRange command schema 1 (additive wire field 17), durable authored
37/inverse 38, snapshot 7 без изменения. Стабильные независимые ключи используют
confirmed scalars; Unicode text ordering, numeric numbers, blank/error в конце.
Относительные/смешанные ссылки используют общий copy transformer. Ограничения:
100000 выбранных ячеек, событие 4 МиБ, нет merges в теле. Native/codec/PG,
12/12 Chromium DPR1/2 и hard restart с Undo/Redo прошли локально.

Статус: матрица целевого профиля с фактически реализованным первым набором формул. Работающий редактор и движок есть; базовый phase 2 gate пройден локально. Фаза 6 читает ограниченный XLSX в отдельную scalar IR, экспортирует подтверждённую книгу владельца и создаёт документ из одного scalar-листа локальной dev-командой. Веб-загрузка ещё не доступна.

Google Sheets задает пользовательскую модель мгновенного редактирования и collaboration. Excel — целевая совместимость формул/файлов в явно ограниченном профиле, не источник runtime architecture. Excel/LibreOffice headless, готовые spreadsheet/formula/collaboration engines не входят в runtime.

Cut/move content (ADR-081): same-sheet stable rectangle, overlap, scalar inbound
references across sheets и fully contained ranges поддержаны сервером. Partial
range intersection получает typed rejection; cross-sheet moves и перенос
styles/merges остаются открытыми. ADR-083 adds guarded own move undo/redo
with exact frozen AST restoration and foreign-reference conflicts. Эти правила
не заявлены как manual Excel parity.

## Formula profile

Точные правила и ограничения: [formula-semantics.md](formula-semantics.md). Convergence нашего native/WASM — exact normalized result hash. Совместимость с Excel проверяется fixtures с версией Excel/ОС/locale и указанным tolerance, где он необходим. Bitwise Excel parity не обещается.

| Статус целевого профиля      | Formula feature                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Supported после phase 2 gate | Literals, basic operators, A1/mixed references, bounded ranges, SUM/AVERAGE/MIN/MAX/COUNT/COUNTA/IF/IFS/IFERROR/AND/OR/NOT/ABS/MOD                                                                                                                                                                                                                                                                |
| Partially supported          | CONCAT/LEFT/RIGHT/MID/LEN/TRIM/LOWER/UPPER: bounded Unicode-scalar profile (ADR-062); modern surrogate counting, locale-independent case and numeric formatting, legacy UTF-16/Excel locale verification open                                                                                                                                                                                     |
| Partially supported          | POWER / fractional exponent: portable binary64 profile (ADR-061), manual Excel precision verification open; ROUND/ROUNDUP/ROUNDDOWN: shortest-decimal profile (ADR-060), selected Excel 16 rounding cases and 15-digit formula-literal normalization verified in ADR-101; broad precision remains open; static dependency treatment IF; range caps; date system metadata и formatting ещё открыты |
| Preserved but not editable   | Unknown function при распознаваемом normalized AST: source/AST сохраняются, canonical result NAME; UI показывает unsupported feature                                                                                                                                                                                                                                                              |
| Dropped with warning         | Импортированные calculated caches не считаются canonical values; могут показываться только как untrusted preview                                                                                                                                                                                                                                                                                  |
| Rejected                     | Неподдержанная грамматика без lossless normalized representation; external/UDF refs; превышение limits                                                                                                                                                                                                                                                                                            |

Формула сохраняется только с server-validated normalized AST. Сохранить неизвестный произвольный текст вместо AST и молча считать cache корректным нельзя. Native/WASM version mismatch приводит к отключению соответствующего preview/reload, а не к доверию клиентскому AST.

DATE/YEAR/MONTH/DAY — partially supported по ADR-063: numeric serials,
Gregorian1900/1904 и fake1900-02-29, bounded normalization. Shared corpus337
проверен native/browser с обоими профилями, но текущая durable workbook model
использует1900. Workbook date-system metadata/import, locale date text/number
formatting и manual Excel goldens ещё открыты; NOW/TODAY включены только
с persisted explicit context по ADR-065.

## XLSX profile: phase 6

Фактически реализован локальный `xlsx-preview <path>`: дочерний процесс проверяет ограниченный ZIP/OOXML пакет, извлекает числа, boolean, shared/inline strings и исходный текст формулы в XLSX-specific IR; родитель повторно валидирует IR и выводит только счетчики. Версия IR 3 представляет followers Excel shared-formula групп ссылкой на master. При `xlsx-import` сервер расширяет относительные формулы общим formula-copy engine, затем связывает их с stable IDs. Команда с dev-auth атомарно публикует новый документ с 1–64 листами и суммарно до 1000 scalar-ячеек. Формульный cache отбрасывается. `GET /api/documents/{id}/export.xlsx` для владельца записывает scalar projection подтверждённой книги; аудит фиксирует версию, размер и checksum до ответа. Живой PostgreSQL import/export/reimport подтвердил shared followers как обычные formulas с корректно сдвинутыми ссылками; широкая Excel-совместимость этим не доказана. Подробности — [ADR-098](adr/098-xlsx-shared-formulas.md) и прежние лимиты: [ADR-043](adr/043-bounded-xlsx-package-inspection.md), [ADR-044](adr/044-normalized-xlsx-scalar-ir.md), [ADR-045](adr/045-xlsx-worker-framed-process.md), [ADR-046](adr/046-bounded-xlsx-worker-ir.md), [ADR-047](adr/047-scalar-xlsx-export.md), [ADR-048](adr/048-owner-audited-document-xlsx-export.md), [ADR-049](adr/049-atomic-local-xlsx-document-import.md). Таблица ниже описывает целевой профиль, не текущую полноту реализации.

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

ZIP limits проверяют реально распакованные bytes, не только headers. Ограничены input size, entry count, expansion ratio, XML depth/attributes/text, relationships, row/cell/formula counts и elapsed budget. Запрещены traversal, duplicate ambiguous entries, external entities/DTD и network relationship fetch. Worker запускается с timeout и завершением всего дерева процессов: Windows Job Object, Unix process group (ADR-099). Жёсткие CPU/committed-RAM лимиты, filesystem/network isolation, private input staging и descriptor limits пока не реализованы.

Linux production adapter должен добавить cgroups v2 memory/CPU, private working directory и read-only input поверх текущего process-group cleanup. Windows development adapter пока использует Job Object для process-tree lifetime; memory/time Job limits, restricted filesystem/network, private temp directory и ACL ещё не настроены. Child не получает DB/S3 credentials; stdin/stdout — versioned framed protocol с length caps. Parent повторно валидирует IR; exit 0 не доказывает безопасность данных.

Malware scanning interface добавляется с upload flow; он не требует внешнего SaaS и не заменяет parser limits. [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) не дает hard cancellation уже работающей задачи, поэтому отдельный процесс обязателен для этих лимитов.

## Протокол и fixtures

Wire compatibility: additive optional fields, reserved retired tags, explicit capability/major gates; unknown operation не применяется частично. Durable events имеют отдельные schema versions/read migrations. XLSX golden corpus хранит source provenance, license и ожидаемый compatibility report. Fixtures с реальными персональными данными не коммитятся.

Round-trip acceptance: supported workbook import → edit → export → import сохраняет поддержанные values/formulas/styles и сообщает об отличиях. Нельзя назвать функцию Excel-compatible на основании одной интуитивной формулы или отсутствия panic.

ADR-065: NOW/TODAY/RAND — Partially supported: persisted epoch, native/WASM convergence, numeric serial без автоматического date formatting. ADR-066 создаёт первый контекст автоматически; кнопка «Пересчитать» создаёт следующий epoch. Automatic open-renewal/timer/dependency triggers и ручные Excel goldens остаются открыты. RAND sequence является собственным deterministic profile, не Excel RNG parity.

ADR-066 снимает ручной bootstrap prerequisite для NOW/TODAY/RAND: первый durable контекст создаётся сервером автоматически. Статус всё ещё Partially supported до automatic dependency/timer epochs и Excel verification; compatible context/value сохраняется после reload.

ADR-067 добавляет automatic edit epochs, в том числе после независимой content правки, и recovery отсутствующего epoch из журнала. Volatile functions остаются Partially supported: timer, open-renewal, date formatting и Excel goldens не закрыты.

ADR-068 добавляет active NOW/TODAY timer через durable context, проверенный на реальном heartbeat. Partially supported остаётся до open-renewal, timezone/date formatting и manual Excel verification; fixed interval не обещает realtime deadline при backpressure.

ADR-069 включает open-renewal: новая WS-подписка/switch/reconnect меняет общий context и RAND для всех клиентов. Duplicate active Subscribe и HTTP reads не меняют epoch. Replay воспроизводит сохранённый context; новое открытие намеренно создаёт следующий. Partially supported сохраняется до timezone/date formatting и manual Excel goldens; Excel RNG sequence parity не обещается.

## INDEX contiguous-reference profile (ADR-070)

Engine0.9.0 adds one-cell selection, zero-axis rectangle selection inside
SUM/AVERAGE/MIN/MAX/COUNT/COUNTA/AND/OR, and nested INDEX references. Only one
contiguous area is supported. Scalar blank becomes zero; aggregate reference
coercion preserves blank/text/boolean distinctions. Two-argument vertical
references are supported; compact horizontal/2D omission and dynamic spill remain
unsupported. Selector coercion fixtures are derived PXEL expectations, not
manually verified Excel goldens. The conservative dependency graph tracks the
whole source and can report a cycle through an unselected source cell.
Reference-valued colon operands, multi-area and dynamic dependency tracking remain
open. See ADR070 for resource limits and the provisional selector profile.

## MATCH lookup profile (ADR-071)

Engine0.10.0: partially supported. Exact and ascending/descending approximate
lookup work on contiguous rows/columns, cells and INDEX reference selections.
Exact text lookup supports case-insensitive * / ? / tilde patterns. Approximate
mode checks monotonic order and treats wildcard characters literally. Blank,
source error precedence, mode coercion and Unicode lowercase/scalar collation
are explicit provisional PXEL rules; derived fixtures are not Excel goldens.
Array literals and locale collation remain open. Cardinality, source/text size
and work/cancellation checks remain active after a first exact match.

## Table lookup profile (ADR-072)

Engine0.11.0: partially supported. VLOOKUP/HLOOKUP accept a contiguous table,
cell or nested INDEX reference. Exact mode returns the first key match with
wildcards; approximate mode requires ascending keys and returns the last floor.
Return index is one-based, positive fractions truncate; selected blank becomes
scalar zero. All tables admit at most100k cells; only the key axis and one return
cell are read. Scalar coercion/error/type/collation rules are provisional, derived
fixtures are not manually verified Excel goldens. Dynamic arrays, multi-area,
locale collation and selected-only dependency tracking remain open.

## XLOOKUP reference profile (ADR-073)

Engine0.12.0: partially supported. Contiguous lookup vectors with aligned return
rectangles, reference results through SUM/COUNT/INDEX/nested XLOOKUP, lazy missing
fallback, exact/nearest/wildcard and forward/reverse/validated binary modes.
Omitted fallback meansNA, explicit blank-cell fallback becomes scalar0. Binary
wildcard, dynamic spill/array literals and reference-valued colon remain unsupported.
Sorted validation returnsNA on unsorted binary source; binary duplicate tie rules
and type/blank/error/coercion/collation are provisional. Manual Excel goldens remain
open. All keys and selected result boundaries are bounded and charged; binary total
cost isO(n). Conservative full-source dependencies may report unselected cycles.

## Single-criterion aggregate profile (ADR-074)

Added in engine0.13.0; current identity is engine0.15.0. COUNTIF(range,criterion)
and SUMIF with optional
sum range support comparison operators and charged case-insensitive wildcards.
The separate sum rectangle starts at the supplied top-left and uses criteria
dimensions; literal expansion is included in reverse dependencies. INDEX/XLOOKUP
selections support shrinkage within their resolved rectangle; dynamic expansion
outside that rectangle is typed unsupported. The verified empty-text behavior
from ADR-097 is implemented. Other numeric-text, blank/boolean/error, Unicode
collation and255-character criteria rules are provisional PXEL semantics.
Derived fixtures do not claim manual Excel validation. SUMIFS/COUNTIFS extend
this profile in ADR075 below.

## Multiple-criteria aggregate profile (ADR-075)

Engine0.15.0: partially supported. SUMIFS/COUNTIFS accept up to127 range/criterion
pairs, intersect corresponding cells, and require identical normalized row AND
column counts. SUMIFS does not expand a short sum range. Contiguous INDEX/XLOOKUP
selections are supported. Selected sum booleans become1/0; selected errors propagate,
unselected errors do not. All visited boundaries and calculation budgets stay active.
Shared blank/numeric-text/error/collation semantics remain provisional; manual Excel,
locale and arrays remain open. No persisted schema change.
