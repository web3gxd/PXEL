# ADR-075: bounded SUMIFS and COUNTIFS

Status: ACCEPTED for the locally verified bounded multi-criteria slice (2026-09-30).

COUNTIFS accepts1..127 range/criterion pairs. SUMIFS accepts a sum range first,
then1..127 pairs. All admitted contiguous rectangles must have equal row AND
column counts after normalization; mismatch produces VALUE, never top-left
expansion. INDEX/XLOOKUP reference selections work within the same100k-cell
rectangle cap. Existing conservative dependencies cover every source and criterion.

Criteria reuse ADR074 comparison/coercion/wildcard rules, including blank scalar
criterion0. Numeric-text/blank/error/Unicode/255-character rules remain provisional
until manually checked Excel fixtures. Each corresponding cell must match every
criterion. Traversal is fixed row-major, with one bounded boolean mask intersected
for each criteria range; source values are not retained. Even already-unmatched
cells retain scalar boundaries, cardinality checks and charged matching work.

SUMIFS traverses the sum range after criteria, adds selected finite numbers/date
serials and selected booleans as1/0 (Microsoft's documented SUMIFS rule). Text and
blank sums are ignored. Selected spreadsheet errors propagate in traversal order;
unselected errors are ignored. Typed work/text/array/cardinality/cancellation errors
are never spreadsheet fallback values. No new dependencies or persisted schemas.
Engine identity is0.14.0.

Local verification: four new native argument/shape/coercion/error/resource/property
tests (11criteria total), three dependency/structural tests,554expected/native/
browser fixtures and1010books/2020patches parity passed. Real PostgreSQL53HTTP,
92Chromium DPR1/2,83frontend unit and2hard-restart recovery tests passed. Native
suites and compile/lint/build checks passed; preliminary100k measurements are
recorded in [the report](../benchmarks/2026-09-30-multi-criteria-local.md).
Full manual Excel/locale/arrays, controlled performance and broader MVP gates
remain open. Existing SUMIF boolean treatment remains unchanged.

Sources: [Microsoft SUMIFS](https://support.microsoft.com/en-gb/excel/functions/sumifs-function),
[Microsoft COUNTIFS](https://support.microsoft.com/en-us/excel/functions/countifs-function).
