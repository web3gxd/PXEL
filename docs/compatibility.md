# Совместимость

Статус: матрица целевого профиля с фактически реализованным первым набором формул. Работающий редактор и движок есть; phase 2 gate еще не закрыт. Фаза 6 начата с bounded package inspection; значения XLSX еще не импортируются, экспорт отсутствует.

Google Sheets задает пользовательскую модель мгновенного редактирования и collaboration. Excel — целевая совместимость формул/файлов в явно ограниченном профиле, не источник runtime architecture. Excel/LibreOffice headless, готовые spreadsheet/formula/collaboration engines не входят в runtime.

## Formula profile

Точные правила и ограничения: [formula-semantics.md](formula-semantics.md). Convergence нашего native/WASM — exact normalized result hash. Совместимость с Excel проверяется fixtures с версией Excel/ОС/locale и указанным tolerance, где он необходим. Bitwise Excel parity не обещается.

| Статус целевого профиля      | Formula feature                                                                                                                      |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Supported после phase 2 gate | Literals, basic operators, A1/mixed references, bounded ranges, SUM/AVERAGE/MIN/MAX/COUNT/COUNTA/IF/IFS/IFERROR                      |
| Partially supported          | Integer-only exponent first; static dependency treatment IF; range caps; date system semantics до ввода date functions               |
| Preserved but not editable   | Unknown function при распознаваемом normalized AST: source/AST сохраняются, canonical result NAME; UI показывает unsupported feature |
| Dropped with warning         | Импортированные calculated caches не считаются canonical values; могут показываться только как untrusted preview                     |
| Rejected                     | Неподдержанная грамматика без lossless normalized representation; external/UDF refs; превышение limits                               |

Формула сохраняется только с server-validated normalized AST. Сохранить неизвестный произвольный текст вместо AST и молча считать cache корректным нельзя. Native/WASM version mismatch приводит к отключению соответствующего preview/reload, а не к доверию клиентскому AST.

## XLSX profile: phase 6

Фактически реализован только первый шаг: `xlsx-inspect <path>` проверяет ограниченный ZIP/OOXML пакет и возвращает имена/ID/пути листов. Он не читает ячейки и не создает документ. Подробные лимиты и незакрытые границы — [ADR-043](adr/043-bounded-xlsx-package-inspection.md). Таблица ниже описывает целевой профиль, не текущую полноту реализации.

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

ZIP limits проверяют реально распакованные bytes, не только headers. Ограничены input size, entry count, expansion ratio, XML depth/attributes/text, relationships, row/cell/formula counts и elapsed/CPU/RAM. Запрещены traversal, duplicate ambiguous entries, external entities/DTD и network relationship fetch. Cancellation убивает всю worker process group/Job Object.

Linux production adapter: cgroups v2 memory/CPU, process timeout, FD limits, isolated private working directory и read-only input. Windows development adapter: Job Objects с kill-on-close/memory/time, private temp directory и restricted child environment. Child не получает DB/S3 credentials; stdin/stdout — versioned framed protocol с length caps. Parent повторно валидирует IR; exit 0 не доказывает безопасность данных.

Malware scanning interface добавляется с upload flow; он не требует внешнего SaaS и не заменяет parser limits. [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) не дает hard cancellation уже работающей задачи, поэтому отдельный процесс обязателен для этих лимитов.

## Протокол и fixtures

Wire compatibility: additive optional fields, reserved retired tags, explicit capability/major gates; unknown operation не применяется частично. Durable events имеют отдельные schema versions/read migrations. XLSX golden corpus хранит source provenance, license и ожидаемый compatibility report. Fixtures с реальными персональными данными не коммитятся.

Round-trip acceptance: supported workbook import → edit → export → import сохраняет поддержанные values/formulas/styles и сообщает об отличиях. Нельзя назвать функцию Excel-compatible на основании одной интуитивной формулы или отсутствия panic.
