# ADR-071: bounded MATCH lookup profile

Status: ACCEPTED FOR BOUNDED MATCH SLICE (2026-09-30).

## Context and invariants

Extend shared pure Rust lookup evaluation after INDEX. Preserve stable bound
references, calculation identity, native/WASM convergence, charged work and typed
resource errors. No schema, HTTP, persistence or dependency changes are needed.
Engine identity advances to pxel-formula-0.10.0 only with the verified slice.

## Initial contract

MATCH accepts two or three arguments. Lookup source is a single contiguous row
or column, a cell or an INDEX reference selection. One-based positions include
skipped blanks. Exact mode0 returns the first matching value. Default mode1
searches ascending values for the last value not greater than lookup; mode-1
does the corresponding descending search. Approximate sources are checked for
monotonic order; unsorted data returns #N/A. Not found returns #N/A.

Number, text and boolean lookup types remain distinct; date serials compare as
numbers. Text comparisons use deterministic Unicode lowercase and scalar order,
not OS locale. Exact text patterns support *, ?, and tilde escapes using bounded
nonrecursive matching. Approximate mode treats pattern characters literally.
Rank order is number, text, boolean. Two-dimensional sources return #VALUE!;
array values remain typed unsupported. No dynamic-array literal support is added.

The following are provisional PXEL rules needing manual Excel edge fixtures:
blank lookup is zero, source blanks are skipped; source errors before an exact
hit propagate, later spreadsheet errors do not override the first hit;
approximate source errors propagate; modes must normalize to exactly-1/0/1
through existing scalar numeric coercion, other values return #VALUE!. Unicode
collation/escaping and mixed-type ordering require further compatibility proof.
Tilde escapes only *, ? and ~; other/trailing tildes are literal. Wildcard ?
counts normalized Unicode scalars after lowercase expansion. A source error
overrides an unsorted approximate result; boundary errors always remain typed.
These are not asserted as manually verified Excel behavior.

## Bounds and verification

The source cap is100,000 cells; traversal/cardinality remain checked even after
an exact hit. Pattern/candidate text is bounded; folding, comparisons and wildcard
work consume cancellation budget. IFERROR cannot catch resource/unsupported
errors. Native edge/property tests, shared expected/browser fixtures, stateful
patch parity, actual PostgreSQL/editor/restart tests and range benchmarks are
required. Full-source dependency tracking retains INDEX's conservative-cycle
limitation. No claimed acceleration without measured correctness/benchmark proof.

Sources: [Microsoft Support MATCH](https://support.microsoft.com/en-us/excel/functions/match-function)
and [Microsoft Learn MATCH](https://learn.microsoft.com/en-us/office/vba/api/excel.worksheetfunction.match).
The contract was reviewed before implementation; completion and final rule
changes are recorded in RUNBOOK.

## Observed verification

409 expected/native/browser fixtures and1010stateful books/2020patches passed.
Native wildcard and numeric position properties, resource/type/reference cases
and source/lookup/structural dependency tests passed. Full49 HTTP cases,84 browser
cases at DPR1/2,83 frontend unit tests and2 hard-restart cases passed, including
durable MATCH/result hashes, receipt retry and later source recalculation.
Actual-PG Open/Timer/volatile admission regressions and clippy/typecheck/lint/
build/format/SQLx checks passed.100k bound evaluator timings are preliminary
local observations. Manual Excel, locale/array support, general MVP and production
deployment/load gates are excluded from this acceptance.
