# Formula semantics: проект профиля v1

Статус: первый срез фазы 2 реализуется по ADR-008/009; полная приемка фазы еще не пройдена. Семантика уточнена корпусом fixtures/formulas/profile-v1.json, результаты исполнения фиксируются в RUNBOOK. Имя профиля: `pxel-excel-subset-v1`.

## Модель

Pure spreadsheet-formula зависит только от domain/general-purpose libraries. Diagnostic syntax AST содержит source spans; versioned normalized persisted AST — отдельный тип без parser spans. Formula хранит source text, canonical text, normalized AST/schema. Engine API не зависит от wasm-bindgen.

Value: Blank, finite Number(binary64), Text, Boolean, DateTime(serial+date system), Array, Error. Reference допускается только как intermediate. Error values: NULL, DIV/0, VALUE, REF, NAME, NUM, N/A, SPILL, внутренний CYCLE. Runtime resource error/cancellation не подменяется Blank/0.

Bound references: BoundCell, BoundRange или Invalid(ReferenceError). BoundCell содержит target sheet/row/column IDs и отдельные axis modes: Absolute либо Relative { original_delta: i64 }. Stable target отвечает за identity, mode — за copy. Relative delta сохраняется вместе с versioned origin basis и нормализуется после structural change относительно актуальных positions. «Original» не означает вечное смещение от самого первого ввода: оно не должно расходиться с отображаемой формулой после вставки.

## Reference transformations

| Действие                   | Проектная семантика                                                                                                                                                                 |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Copy formula               | Relative axes смещаются на offset source→destination; absolute target axes сохраняются; out-of-bounds → REF. Source content фиксируется canonical event, replay его не перечитывает |
| Move formula cell          | Перенос origin сохраняет bound targets; относительные deltas пересчитываются для нового origin; внешние ссылки на перенесенное содержимое обрабатывает relocation map               |
| Cut/paste referenced cells | Atomic content relocation map преобразует входящие ссылки на перемещаемые ячейки; за пределами поддержанного overlap/merge scope операция отклоняется                               |
| Insert row/column          | Target IDs сохраняются, displayed A1 и relative deltas обновляются; range anchors расширяются по documented affinity                                                                |
| Delete target              | BoundCell становится Invalid REF; partial range сжимается до живых границ, fully deleted range → REF                                                                                |
| SortRange                  | Не входит в ранний MVP; будущая atomic content permutation внутри rectangle, не перестановка всей row identity; до отдельного контракта rejected                                    |
| SortSheet                  | Отдельная будущая команда; ее семантика не маскируется SortRange                                                                                                                    |

Для move/cut/copy нужны отдельные fixtures Excel/Google-style UX; таблица задает проектный контракт, не полную Excel parity. Merge/array dependencies вне поддержанного scope отклоняются до commit. Formula binding на base_version определяется точным window/checkpoint алгоритмом в collaboration.md.

## Числа и порядок

IEEE-754 binary64, -0 → +0; nonfinite literal/input rejected; computed NaN/infinity → NUM, divide by zero → DIV/0. Hash использует normalized bit pattern, не форматированный decimal text. Subnormals сохраняются и считаются отдельным compatibility case.

`+ - * /` выполняются в фиксированном AST order. Нет fast-math, reassociation или недокументированного FMA. SUM обходит аргументы слева направо, диапазоны в workbook row-major order; parallel reduction, меняющий order, запрещен. Optimization требует benchmark и correctness corpus.

POWER и `^` используют общий portable libm0.2.16 с отключённым arch и включённым force-soft-floats по ADR-061. Дробные и большие finite exponents допустимы; negative base с nonintegral exponent даёт NUM; zero negative exponent даёт DIV/0, zero-to-zero NUM; nonfinite output NUM. Underflow сохраняет subnormals либо normalized zero. Бывший integer-only squaring profile заменён с новым engine identity0.5.0; history/context не переписываются. Native/WASM corpus доказывает сходимость в проверенных случаях, а не bitwise Excel parity или все возможные platform combinations.

Три разных обещания: convergence compatibility (наш native/WASM exact normalized results); Excel compatibility (fixtures/допуски поддержанного профиля); bitwise Excel parity (не обещается).

## Синтаксис и первые функции

`=`; numbers/strings/booleans; unary/binary operators; Excel precedence; parentheses; A1/$A$1/A$1/$A1; bounded ranges; quoted sheet names с doubled apostrophe; cross-sheet references; function calls. Canonical function names English. Locale adapter token-aware: decimal/argument separators не заменяются внутри strings или quoted names. UTF-8 parser spans переводятся в UTF-16 UI offsets явно.

Текущий реализованный срез: SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, IF, IFS, IFERROR, AND, OR, NOT и arithmetic/comparisons. Math, text, date, lookup, criteria groups добавляются последовательно после specs/fixtures. Array literals, dynamic spill, structured/3D/external refs, UDF/LET/LAMBDA и iterative cycles пока вне профиля.

| Функция   | Arity | Начальный контракт                                                                                                                                  |
| --------- | ----: | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| SUM       | 1–255 | Range text/bool/blank игнорируются; zero учитывается; scalar coercion отдельно; ошибки распространяются в фиксированном порядке                     |
| AVERAGE   | 1–255 | Numeric count; empty numeric set → DIV/0; direct/range coercion отдельны                                                                            |
| MIN / MAX | 1–255 | Empty numeric set → 0; range text/bool игнорируются; direct arguments требуют отдельной таблицы                                                     |
| COUNT     | 1–255 | Range/cell refs: только Number/DateTime; direct Number/DateTime, Boolean и finite numeric Text считаются; Blank и обычные error values игнорируются |
| COUNTA    | 1–255 | Любое nonblank значение, включая Text(""), Boolean и cell error, считается; Blank/omitted не считается                                              |
| IF        |   2–3 | Scalar lazy branch; logical test coercion, omitted/empty arguments — отдельные fixtures                                                             |
| IFS       | 2–254 | 1–127 пар test/value; первый Boolean TRUE выбирает результат; иной вычисленный test → VALUE, отсутствие TRUE → N/A; Blank выбранного результата → 0 |
| IFERROR   |     2 | Первый аргумент возвращается без вычисления fallback, если нет spreadsheet Error; иначе возвращается fallback. Blank выбранного значения → Text("") |

IFS вычисляет условия слева направо и только результат первой TRUE-пары; более поздние условия и пропущенные результаты не вычисляет. Ошибка вычисленного условия распространяется. Это проектный контракт для ленивости и Blank; фактические Excel results ещё не внесены как golden fixtures. Максимум 127 пар следует из [описания Microsoft IFS](https://support.microsoft.com/en-us/excel/functions/ifs-function).

IFERROR перехватывает только spreadsheet Error values. Ограничение вычисления, отмена и неисправность resolver остаются typed engine errors и не заменяются fallback. Выбранный Blank даёт пустой Text по [описанию Microsoft IFERROR](https://support.microsoft.com/en-us/excel/functions/iferror-function). Array-formula behavior вне профиля.

| Функция | Arity | Coercion и ошибки                                                                                                            |
| ------- | ----- | ---------------------------------------------------------------------------------------------------------------------------- |
| AND     | 1–255 | Все участвующие значения должны быть true; zero → false. Text/Blank в ссылках пропускаются; без участвующих значений → VALUE |
| OR      | 1–255 | Достаточно одного true; zero → false. Text/Blank в ссылках пропускаются; без участвующих значений → VALUE                    |
| NOT     | 1     | Scalar logical coercion и инверсия; Blank → true, zero → true, ненулевое finite number → false; error распространяется       |

AND/OR не прекращают вычисление после решающего boolean: `AND(FALSE,1/0)` и `OR(TRUE,1/0)` дают DIV/0. Порядок аргументов слева направо, диапазонов — row-major. Первая spreadsheet error сохраняется; typed cancellation/budget/resolver error имеет приоритет и не маскируется. В непосредственном аргументе Text("TRUE")/Text("FALSE") приводятся к boolean, иной текст даёт VALUE. NOT использует те же scalar rules; диапазоны/arrays и implicit intersection вне профиля. Microsoft описывает [AND](https://support.microsoft.com/en-us/excel/functions/and-function), [OR](https://support.microsoft.com/en-us/excel/functions/or-function), [NOT](https://support.microsoft.com/en-us/excel/functions/not-function); дополнительные edge cases — provisional project rules до ручного Excel golden corpus. [ADR-055](adr/055-logical-function-profile.md) фиксирует engine identity 0.2.0 и переход старых persisted contexts через новый durable epoch, без изменения исторических событий.

Coercion не сводится к универсальному to_number. Operators, direct arguments, references и ranges имеют отдельные rules. Blank и Text("") различаются. Ошибка выбранной ветви IF распространяется; невыбранная не вычисляется. Начальная static dependency analysis обеих ветвей может консервативно считать cycle; такое отличие явно тестируется и документируется до acceptance.

COUNT/COUNTA посещают диапазон в row-major порядке с теми же 100 000-cell/work-budget лимитами и проверкой числа реально посещённых ячеек. Обычная ошибка, записанная в ячейке, для COUNT пропускается, для COUNTA считается. Сломанный stable-ID target проецируется в отдельный `InvalidReference` и возвращает `#REF!`, а не маскируется как пустая ячейка. Для вложенного выражения, которое возвращает ошибку, текущий профиль применяет правило direct argument; provenance структурной ошибки во вложенном выражении ещё не сохраняется и требует отдельного compatibility fixture перед структурными операциями. Лимит 1–255 — профиль PXEL; утверждение о полной Excel parity по arity не делается.

Контрактные ожидания, еще не Excel-verified fixtures: `=1+2*3` →7; `=-2^2` →4; `=1/0` →DIV/0; blank A1 +1 →1; SUM над [1,"x",2] →3; AVERAGE пустого range →DIV/0; IF(FALSE,1/0,7) →7; неизвестная FOO →NAME; A1↔B1 →CYCLE. Каждый fixture позднее хранит provenance и verified_in_excel flag.

Источники для fixtures: [Excel precedence](https://support.microsoft.com/en-us/excel/calculation-operators-and-precedence-in-excel), [SUM](https://support.microsoft.com/en-us/excel/functions/sum-function), [AVERAGE](https://support.microsoft.com/en-us/excel/functions/average-function), [COUNT](https://support.microsoft.com/en-us/excel/functions/count-function), [COUNTA](https://support.microsoft.com/en-gb/excel/functions/counta-function), [IF](https://support.microsoft.com/en-us/excel/functions/if-function), [IFS](https://support.microsoft.com/en-us/excel/functions/ifs-function), [IFERROR](https://support.microsoft.com/en-us/excel/functions/iferror-function).

## Dependency graph и budgets

Скалярные ABS/MOD реализованы по [ADR-059](adr/059-scalar-abs-mod-profile.md):
арность 1/2, существующий arithmetic coercion, blank как zero, ошибки сохраняются.
MOD возвращает remainder со знаком divisor, zero divisor даёт DIV/0; fixed
binary64 n-d*floor(n/d), nonfinite или потеря sign/bounds из-за крайних чисел
даёт NUM. Native/WASM корпус проверяет 27 новых cases; ручная Excel verification
и extreme precision corrections остаются открыты. Engine identity 0.7.0 требует
явного epoch advance для старых persisted contexts.

ROUND/ROUNDUP/ROUNDDOWN используют bounded shortest-decimal quantization по
[ADR-060](adr/060-decimal-rounding-profile.md). Два scalar аргумента; дробная
точность усекается к нулю, ROUND округляет ties от нуля, ROUNDUP от нуля,
ROUNDDOWN к нулю. Overflow даёт NUM; -0 нормализуется. Общий корпус содержит
202 примера, включая midpoint neighbors и extreme/subnormal values. Это явный
PXEL профиль; полная Excel precision compatibility ещё не подтверждена.

CONCAT/LEFT/RIGHT/MID/LEN/TRIM/LOWER/UPPER реализованы по [ADR-062](adr/062-bounded-text-functions.md).
Unicode scalar indexing соответствует modern surrogate-pair profile; combining
marks/variation selectors считаются отдельно. TRIM касается только ASCII space,
case mapping locale-independent. CONCAT range traversal row-major, blank empty;
arity1–253, logical32767-character cap VALUE, byte32KiB/work caps — evaluator
errors. Numeric text indices также проверяются и расходуют бюджет до coercion.
Вариант legacy UTF-16 counting, locale-specific case/number formatting и ручная
Excel verification ещё открыты. Engine0.7.0 требует новый calculation epoch
для старого context; AST/event/snapshot schemas не меняются.

DATE/YEAR/MONTH/DAY реализованы по [ADR-063](adr/063-calendar-serial-profile.md):
DATE возвращает numeric serial, не меняет формат ячейки. Чистая Gregorian
арифметика поддерживает1900/1904, фиктивное1900-02-29 (serial60) и legacy
serial0 YEAR1900/MONTH1/DAY0. Fractional arguments усекаются; month/day overflow
нормализуется, невалидный диапазон даёт NUM. Calendar parts принимают дробь
последнего дня9999-12-31. Text coercion проходит bounded scan/work budget;
resource failures не скрываются IFERROR. Общие337 fixtures включают53 calendar
cases с обоими системами; manual Excel verification не заявлена. Текущие книги
редактора используют1900. Durable выбор1904, locale date parsing/formatting и
NOW/TODAY с persisted injected context реализованы в ADR-065; их automatic
epoch triggers остаются следующим срезом.

Forward/reverse direct edges, range nodes и reverse range index: сначала row intervals per sheet с column filtering, без полного scan всех ranges при каждом edit. Maximum 100 000 addressed cells/range, 256 range dependencies/formula и 1 000 000 deterministic work units/job. Подтверждение разумности caps — benchmark фазы 2.

Range index связывается со stable boundaries/current axis projection; structural event обновляет индекс прежде, чем dirty propagation пользуется новыми positions. Change formula удаляет старые dependencies. Topological/SCC traversal детерминирован и ограничен dirty subgraph; long chains не используют рекурсию на 10k depth. Cycles дают CYCLE, не зависание.

CalculationLimitExceeded — typed calculation diagnostic/status, не поддельное canonical value. Worker timeout/cancellation оставляет generation unfinished; старые caches не выдаются за current. При caps admission можно отклонить формулу, чья статическая стоимость заведомо недопустима, до сохранения. Динамическое превышение после durable edit не отменяет committed command.

## Даты и volatile context

DateTime хранит finite serial days с 1900/1904 date system; fraction — часть дня. Serial 60 в системе 1900 сохраняет фиктивное 1900-02-29 как специальную Excel-семантику, а не невозможную Gregorian date. Системные timezone/clock evaluator не читает. [Excel 1900 behavior](https://learn.microsoft.com/en-us/troubleshoot/microsoft-365-apps/excel/wrongly-assumes-1900-is-leap-year).

CalculationContext: context_id, calculation_epoch, effective_timestamp_utc, workbook_timezone, random_seed, engine_version, compatibility_profile. Для воспроизводимого civil time сохраняется timezone ruleset version или уже нормализованные civil date/offset в контексте; обновление OS tzdata не должно менять replay.

Контекст меняется только durable `AdvanceCalculationEpoch` system event с полным контекстом и reason. Event получает обычную monotonic document version, не маскируется под пользовательский edit. Новая activation/open, explicit full recalc, timer для NOW/TODAY, volatile dependency change или engine/profile migration могут запрашивать новый epoch. Частые открытия coalesce в document actor; restore/replay не генерирует новое время сам. Периодический timer включается только при наличии volatile consumers.

RAND основан на versioned deterministic hash(document_id, epoch, formula_cell_id, stable_ast_node_id, seed); AST node key различает несколько RAND() в одной формуле. Из hash фиксированно извлекаются 53 bits в [0,1), без platform PRNG. Worker order/parallelism не меняют результат. Seed не копируется в каждое пользовательское событие.

## Native/WASM parity

Один engine, один fixture corpus, одинаковые context/profile и normalized results. Browser worker держит долгоживущую projection; initial checkpoint один раз, затем commands/patches. Preview/cache не смешивается с confirmed server calculations. Отдельно измеряются encode/decode, JS/WASM crossing и Rust calculation; workbook целиком на каждое нажатие не сериализуется.

# Structural deletion in the current single-sheet profile

Schema-7 `DeleteRows` preserves surviving stable row targets. A scalar bound
reference to a deleted target becomes typed `Invalid(DeletedTarget)` and renders
as `#REF!`. A rectangular range with deleted endpoints shrinks to its first
and last surviving rows; if none survive, the range becomes `#REF!`. Relative
copy offsets of surviving references are recomputed from the new axes.
`Formula.source` is retained as edit history; the projection's `current_text`
is derived from the transformed bound AST. This behavior is verified for
native replay and browser display, but has not yet been certified against
Excel for every range case. See ADR-022.

# Column insertion in the current single-sheet profile

Schema-8 `InsertColumnsRebased` inserts stable column IDs before a live anchor.
Bound targets keep their IDs; relative column copy offsets are recalculated
against the new origin position, while absolute modes remain absolute. Ranges
continue to span their stable endpoints and therefore include inserted columns
between them. The displayed `current_text` is rendered from the new axes;
authored source/canonical text remain historical. Native replay, browser and
the domain benchmark cover this slice. See ADR-023.

# Column deletion in the current single-sheet profile

Schema-9 `DeleteColumns` preserves surviving stable targets. A bound scalar
reference to a removed column becomes typed `Invalid(DeletedTarget)` and
renders as `#REF!`. A rectangular range with removed endpoints shrinks to
its first/last surviving columns; a range without survivors becomes `#REF!`.
Relative column copy offsets are recomputed from the new axes. Authored
source/canonical text is retained; `current_text` reflects the transformed
bound AST. Native replay and browser display are verified, while full Excel
range parity remains outside the current profile. See ADR-024.

ADR-065: engine0.8.0 включает NOW/TODAY/RAND только в сохранённом epoch. NOW/TODAY возвращают numeric serial с fixed offset, RAND — 53-bit SHA256 derivation по document/epoch/stable cell/AST node/seed. Missing context — typed error, не host clock fallback. Смена контекста помечает все formulas dirty с graph budget. ADR-066 добавляет initial/legacy bootstrap; automatic open-renewal/timer/dependency admission ещё не реализован.

ADR-066: первое сохранение volatile формулы автоматически записывает initial context отдельным system event после user commit. Authorized read legacy volatile книги также bootstrap/migrates context. Повторное чтение уже покрытой версии переиспользует compatible epoch. Следующий срез ADR-067 добавляет автоматические epochs после правок; таймер ещё не включён.

ADR-067: content edits, copy/undo и explicit insert/delete в volatile книге автоматически создают новый durable epoch. Расширение пустых осей при scroll не меняет RAND. Reads восстанавливают пропущенный trigger из журнала, включая restart между user/system commit; повторное чтение уже покрытой версии не пересэмплирует. Timer и compatible open-renewal ещё впереди.

ADR-068: heartbeat открытой WebSocket-подписки запрашивает timer epoch. NOW требует минимум60s от сохранённого timestamp, TODAY-only — смены civil day при той же offset policy. RAND-only/static книги не создают timer event. Неполученный receipt разрешается по прежнему ID до проверки необходимости; следующий heartbeat не дублирует durable epoch. Server timezone policy пока UTC0; mismatch не исправляется незаметно. Open-renewal и нагрузочные замеры ещё впереди.

ADR-069: actual WS subscription transition создаёт Open context для compatible volatile книги. First bootstrap/recovered edit context этого admission переиспользуется. Повтор Subscribe того же active document, HTTP reads, head checks и replay не являются открытием. Новая вкладка/reconnect обновляет shared RAND у всех подключённых клиентов; replay сохранённых событий сам не пересэмплирует. Pending ID сохраняется до успешного post-commit recovery. Будущая версия replay cursor проверяется до Open. Многоподписочная нагрузка и timezone settings остаются gates.

## INDEX selection (engine0.9.0)

Private intermediate references survive nested INDEX and aggregate arguments;
they are never persisted as cached values. Scalar reads resolve only the selected
cell; aggregate reads visit the selected rectangle in row-major order. Graph
dependencies conservatively retain the full source plus selector dependencies.
Source size and selected traversal are bounded separately: selecting one cell
does not bypass the 100,000-cell source admission cap. Unsupported array values,
invalid resolver cardinality, cancellation and exhausted budgets remain typed
evaluator errors, including through IFERROR. ADR070 defines omitted-axis handling,
negative/fractional selectors and remaining manual compatibility gates.

## MATCH lookup (engine0.10.0)

Single-row/column references retain one-based positions including skipped blanks.
Exact lookup selects the first hit; sorted approximate lookup selects the last
eligible item. Wildcards are charged nonrecursive matching on normalized Unicode
scalars, with bounded scratch storage. Entire-source boundary validation remains
active after a hit. AST/wire/storage formats are unchanged; engine context identity
changes and old contexts require the existing authorized upgrade epoch. ADR071
specifies provisional blank/error/type/coercion rules and manual Excel gaps.
