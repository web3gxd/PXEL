# ADR-074: bounded SUMIF and COUNTIF criteria

Status: ACCEPTED for the locally verified bounded criteria slice (2026-09-30).

## Contract

COUNTIF takes2 arguments; SUMIF takes2 or3. Range arguments are contiguous
references, including INDEX/XLOOKUP selections. Criteria support scalar number,
text, boolean, blank and date serial, comparisons= <> < <= > >= and charged
case-insensitive text wildcard rules of ADR071. Empty text matches blank/empty
text; blank scalar criterion means numeric zero. Numeric criteria coerce finite
numeric text and blank to0; booleans remain distinct. Other-type inequality matches
unequal types; ordered comparisons require compatible types. Error-string criteria
can match error cells; error-valued criteria propagate. Numeric-text/blank/boolean,
error precedence, Unicode collation and255-scalar criterion cap are explicit
provisional rules, requiring manual Excel goldens. ADR-097 adds a verified
exception for formula-empty text: it matches `<>`, while genuinely blank cells
do not; `*` does not match formula-empty text. Longer criteria are typed
unsupported, oversized text/work/arrays/cancellation remain typed failures.

SUMIF adds selected finite numbers/date serials in fixed row-major order; ignores
text/boolean/blank sums, propagates selected errors and ignores unselected errors.
COUNTIF ignores unmatched errors. All visited values still pass typed boundaries.
Separate sum range uses criterion dimensions at its top-left anchor, as documented
by Microsoft. Literal Cell/Range sources support expansion with corresponding
implicit reverse-range dependencies. Dynamic selected references may shrink inside
their admitted rectangle but cannot extend beyond it without static dependency
coverage; that combination fails typed unsupported. No silent dependency misses.
Rectangle/Excel-axis/work/cardinality limits remain enforced. Dependencies stay
conservative, including cycles in unselected source cells.

Streaming COUNTIF/default SUMIF; separate SUMIF uses a bounded boolean selection
mask followed by one bounded sum-range traversal. No whole workbook transfer,
external engine, new library or persisted schema. Engine identity is0.13.0.
SUMIFS/COUNTIFS are the following slice, reusing tested criteria semantics.

Local verification: seven native criteria tests, two implicit dependency/structural
tests,520 expected/native/browser fixtures and1010books/2020patches parity;
52real-PostgreSQL HTTP,90Chromium DPR1/2,83frontend unit and2hard-restart recovery
tests passed. Native resource/property suites and compile/lint/build checks passed.
Preliminary100k range measurements are recorded in
[the benchmark report](../benchmarks/2026-09-30-criteria-local.md). Full
Excel/locale/arrays, controlled performance and broader MVP gates remain open.

Sources: [Microsoft SUMIF](https://support.microsoft.com/en-us/excel/functions/sumif-function),
[Microsoft COUNTIF](https://support.microsoft.com/en-gb/excel/get-started/use-the-countif-function-in-microsoft-excel?nochrome=true).
