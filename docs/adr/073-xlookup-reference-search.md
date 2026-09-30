# ADR-073: bounded XLOOKUP reference search

Status: ACCEPTED for the bounded reference/search slice (2026-09-30).

## Contract

Three to six arguments. Lookup source is a contiguous one-dimensional reference;
return source is contiguous, including INDEX/XLOOKUP selections. Vertical lookup
requires equal return row count and selects a row; horizontal lookup requires
equal column count and selects a column. A single-cell source uses vertical
orientation. Each admitted rectangle has at most100k cells. Return references
survive aggregation and nested INDEX/XLOOKUP; scalar context accepts one cell,
blank becomes zero. Dynamic spill, arrays and reference-valued colon syntax
remain unsupported. Conservative whole-source dependencies remain explicit.

Default match0 exact, modes-1 nearest smaller,1 nearest larger,2 wildcard.
Default search1 forward,-1 reverse,2 ascending binary,-2 descending binary.
Linear nearest search accepts unsorted keys and exact matches take precedence;
equal nearest keys retain traversal priority. Binary modes validate sorted order
and use bounded lower-bound search; unsorted input gives #N/A as a controlled PXEL
profile. Binary wildcard is typed unsupported pending a meaningful ordered
wildcard specification. All keys are boundary-validated even after a match.

Lookup blank (including omitted lookup_value) finds blank keys; other comparisons
reuse charged Unicode/type profiles of ADR071. Mode arguments use bounded scalar
numeric coercion and require exact permitted integers; missing defaults to0/1.
if_not_found evaluates lazily only on no match, not source spreadsheet errors or
typed boundary failures. Error precedence, duplicate binary ties, blank nearest
behavior, numeric-text coercion and locale collation are provisional PXEL rules,
requiring manual Excel goldens. Source/return shape and resource checks precede
fallback. Binary mode validates the entire source, so total cost remains O(n).

Engine identity is0.12.0; AST/wire/storage formats and dependencies
remain unchanged. Require native/property/resource/reference fixtures, shared
native/browser parity, graph/structural tests, real PostgreSQL/browser/restart
verification and measured search modes before accepting this slice.

Source: [Microsoft XLOOKUP](https://support.microsoft.com/en-us/excel/functions/xlookup-function).

## Recorded verification

Ten native XLOOKUP contract/resource/property tests and two graph/structural tests
passed.482 expected/native/browser fixtures and1010books/2020patches converge.
Real PostgreSQL51 HTTP,88 browser,83frontend unit and2hard-restart tests passed.
Format/lint/build/clippy/SQLx checks passed. Read-only review found and drove fixes
for Missing fallback and nested selected-text boundaries; scalar INDEX now shares
DateTime/text/array normalization. No persisted schema migration was needed.
Benchmarks are [preliminary and locally loaded](../benchmarks/2026-09-30-xlookup-local.md),
not performance gates. Full MVP, manual Excel, spill/colon/binary-wildcard,
production/Linux/Compose/remote CI and broad load/recovery gates remain open.
Detailed actual command results and failed-regression causes are in RUNBOOK.
