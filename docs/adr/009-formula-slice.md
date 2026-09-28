# ADR-009: первый формульный профиль и проверяемый корпус

Дата: 2026-09-28. Статус: принято для реализации первого среза фазы 2 по поручению продолжать стадии; завершение фазы не заявляется.

## Контекст и изменение решения

В phase-0 документации функции и coercion оставались проектом. Теперь фиксируем исполняемый контракт `pxel-excel-subset-v1` в [корпусе](../../fixtures/formulas/profile-v1.json). Он содержит ожидаемые результаты, источник и основание каждого ожидания. `verified_in_excel=false` означает именно отсутствие запуска конкретного случая в Excel, даже если правило опубликовано Microsoft. Native/WASM совпадение само по себе этот флаг не меняет. Bitwise Excel parity не обещается.

Прежняя ссылка MS-OE376 с UUID `981b17ff-5594-42cf-ad8d-7cb39e653afa` ведет на `tav (Time Animate Value)` и не доказывает Excel precedence. Используем [действительную таблицу Excel](https://support.microsoft.com/en-us/excel/calculation-operators-and-precedence-in-excel): unary minus выше exponentiation; одинаковый приоритет вычисляется слева направо. Поэтому `=-2^2` дает 4, `=-(2^2)` дает -4, `=2^3^2` дает 64. Миграция старых событий не нужна: формулы ранее не сохранялись.

## Синтаксис и числовой контракт

Английские function identifiers, `.` decimal separator, `,` argument separator; локализация отдельным adapter. Поддерживаются literals, скобки, unary `+/-`, binary `+ - * / ^`, comparisons `= <> < <= > >=`, A1/mixed references, ranges и quoted sheet names с doubled apostrophe. Error literals и empty argument nodes должны быть представлены явно; unknown function сохраняет identifier и дает `#NAME?`.

Вычисления binary64 выполняются в порядке AST, без fast-math/reassociation. `-0` нормализуется в `+0`; overflow/NaN становятся `#NUM!`, деление на ноль — `#DIV/0!`. Range traversal — row-major, function arguments слева направо; первая встреченная ошибка распространяется. Для `^` пока разрешены только целые exponent в [-1024,1024], deterministic exponentiation-by-squaring; вне профиля — `UnsupportedCompatibilityFeature`, а не приблизительное число. Сравнение чисел точное binary64; numeric Text не превращается в Number для equality. ASCII text equality case-insensitive; полная locale/Unicode collation остается отдельной совместимостью. Corpus помечает последние правила как project contract, не подтвержденные Excel наблюдения.

## Coercion и функции

Происхождение значения сохраняется: прямой expression argument отличается от cell reference и range member. Общий `to_number` для всех контекстов запрещен. В arithmetic принимается numeric text ограниченного invariant numeric syntax, Blank→0, Boolean→0/1; произвольный текст дает `#VALUE!`. Currency/date/locale-dependent numeric strings не входят в этот срез.

| Функция | Arity профиля | Прямые аргументы                                          | References/ranges                                                        | Пустое числовое множество |
| ------- | ------------- | --------------------------------------------------------- | ------------------------------------------------------------------------ | ------------------------- |
| SUM     | 1–255         | Number, Boolean→0/1, numeric Text→Number; иной Text→VALUE | Только Number; Blank/Text/Boolean пропускаются; Error распространяется   | 0                         |
| AVERAGE | 1–255         | Такой же coercion; см. конфликт источников ниже           | Только Number; ноль учитывается, Blank/Text/Boolean пропускаются         | DIV/0                     |
| MIN/MAX | 1–255         | Такой же coercion                                         | Только Number; Blank/Text/Boolean пропускаются; Error распространяется   | 0                         |
| IF      | 2–3           | Scalar logical test; вычисляется только выбранная ветвь   | Test Blank→false; выбранный Blank cell reference отображается числовым 0 | Не применяется            |

Правила SUM подтверждает [WorksheetFunction.Sum](https://learn.microsoft.com/en-us/office/vba/api/excel.worksheetfunction.sum); limits VBA-метода не используются как worksheet arity. MIN/MAX описаны на страницах [MIN](https://support.microsoft.com/en-us/excel/functions/min-function) и [MAX](https://support.microsoft.com/en-us/excel/functions/max-function). Принятый strict minimum arity 1 означает, что `SUM()`/`MIN()`/`MAX()` дают `#VALUE!`; это ограничение нашего профиля, а не утверждение об Excel. Wrong arity — spreadsheet `#VALUE!`, malformed syntax — structured parser diagnostic.

**AVERAGE-DOC-CONFLICT:** [Support AVERAGE](https://support.microsoft.com/en-us/excel/functions/average-function) утверждает, что direct logical/numeric-text arguments не учитываются; [Learn WorksheetFunction.Average](https://learn.microsoft.com/en-us/office/vba/api/excel.worksheetfunction.average) утверждает обратное. Выбран counted-контракт, например `AVERAGE(TRUE,"5",3)=3`, явно помеченный `project_contract`. Критерий закрытия: сохранить фактический Excel result, version/build/OS/locale и дату; при расхождении менять профиль/fixtures открыто, не переписывать persisted события.

## IF и пустые аргументы

| Formula                    | Результат                                                  |
| -------------------------- | ---------------------------------------------------------- |
| `IF(FALSE,7)`              | Boolean false: третий argument отсутствует                 |
| `IF(FALSE,7,)`             | Number 0: третий argument присутствует как MissingArgument |
| `IF(TRUE,,7)`              | Number 0                                                   |
| `IF(TRUE,"",7)`            | Text пустой длины; не Blank                                |
| `IF(FALSE,1/0,7)`          | Number 7: первая ветвь не вычисляется                      |
| `IF(TRUE,1/0,7)`           | DIV/0                                                      |
| `IF(A1,7,9)` при пустой A1 | Number 9                                                   |

[IF documentation](https://support.microsoft.com/en-us/excel/functions/if-function) описывает выбор двух результатов и ноль при незаданном значении, но не доказывает все перечисленные различия. Поэтому точные omission/laziness fixtures остаются project contract до Excel verification. `MissingArgument`, отсутствующий optional argument и пустой Text — разные представления. Static dependency graph может учитывать обе IF branches для dirty propagation; evaluator не должен вычислять невыбранную ветвь. Консервативный cycle по невыбранной ветви отмечается как ограничение профиля и требует отдельного fixture.

## Durable AST и зависимости

Diagnostic syntax AST содержит source spans и A1 positions; в journal он не сохраняется как canonical truth. Server binder создает versioned normalized bound AST: flat arena с root/NodeId, literal/unary/binary/call/missing/reference nodes. Bound cell/range используют stable sheet/row/column IDs, absolute/relative modes, delta basis и range affinity. Source/canonical text, AST schema, compatibility profile и origin сохраняются вместе; parser spans остаются diagnostic-only. Limits nodes/depth и forward-only arena references проверяются при decode. Hash использует собственное тегированное semantic encoding, не protobuf bytes.

Binding относительно base_version использует оговоренное структурное окно; слишком старый ввод получает RebaseWindowExceeded. Replay применяет сохраненный bound AST после проверки schema, не перепарсивает старый текст новой грамматикой. Copy/move/cut/insert/delete требуют отдельных transformations. Выбор evaluator implementation не может поменять identity ссылок. Unknown function имеет normalized identifier вместо неверсированного registry ordinal.

## Проверка и открытые работы

Уточнение первого алгоритма `^`: для отрицательного целого exponent сначала
берется reciprocal основания, затем выполняется exponentiation-by-squaring.
Прежнее предложение reciprocal после возведения переполняло промежуточный
результат для представимого `2^-1024`. Новый порядок сохраняет этот subnormal;
каждый nonfinite промежуточный результат дает NUM. Persisted formulas со старым
алгоритмом еще не существовали, миграция не нужна. Fixtures и native/WASM parity
проверяют выбранный фиксированный порядок; bitwise Excel parity не обещается.

Corpus `schema_version=1` общий для native/browser. Input cells и expected values — tagged `{type,value}`; отсутствующая cell означает Blank. Expected diagnostic — ошибка ограничения движка, отдельно от spreadsheet Error. `provenance.basis` различает documented example, derived expectation и project contract. Поддержка новых случаев не означает автоматическую активацию функций вне профиля.

- FORMULA-EXCEL-GOLDEN: actual Excel пока не запускался; закрытие требует результатов и окружения, включая disputed coercion/omissions/negative powers.
- FORMULA-PARITY: native и browser WASM должны прочитать один corpus и сравнить normalized values/hashes; отдельный отчет содержит реальные результаты.
- FORMULA-BOUNDS: limits parser/range/work budget и cancellation проверяются adversarial inputs; ни exhaustion, ни cancellation не превращаются в Blank.
- FORMULA-RANGES-BENCH: benchmarks direct cells/ranges/reverse range lookup и chain 10k обязательны перед claims performance.

Эта ADR и корпус описывают контракт. Они не являются отчетом о прошедшем evaluator, WASM, incremental recalculation или Excel тесте.
