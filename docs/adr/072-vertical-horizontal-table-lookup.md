# ADR-072: bounded VLOOKUP and HLOOKUP

Status: ACCEPTED for the bounded table lookup slice (2026-09-30).

## Context

MATCH supplies bounded deterministic key search. VLOOKUP/HLOOKUP add table value
selection in the same pure Rust engine without implementing search twice. Stable
bound references, native/WASM parity, charged cancellation and separate confirmed
results remain invariants. Engine identity advances to pxel-formula-0.11.0;
AST, wire, persisted formats and dependencies do not change.

## Contract

Accept three or four arguments and a contiguous table/cell/INDEX reference.
VLOOKUP searches its first column; HLOOKUP its first row. The return index is
one-based: less than1 gives #VALUE!, larger than the corresponding extent gives
#REF!. Positive fractions truncate using the explicit provisional numeric profile.
Numeric scalar coercion remains bounded and charged. Omitted range_lookup means
TRUE; explicit blank/missing means FALSE. Existing boolean scalar coercion accepts
booleans, finite numbers, TRUE/FALSE text and blanks. Array values remain typed
unsupported rather than being coerced accidentally.

Exact mode returns the first matching key and supports existing wildcard/case
rules. Approximate mode validates ascending order and selects the last eligible
key not greater than lookup. Unsorted approximate data gives #N/A. Blank keys,
type rank, source error precedence and Unicode collation inherit ADR071's
provisional profile, not manually verified Excel behavior.

The full table must fit100,000 cells. Traverse and validate only the key axis,
including cardinality after an exact hit; read one selected return cell. Unselected
return cells are not evaluated by this lookup. Output is a value, not a reference:
selected blank becomes numeric zero; text/boolean/error are preserved; nonfinite
numeric results become #NUM!. Selected arrays are typed unsupported and text is
bounded to32KiB with charged work. IFERROR cannot hide boundary failures.

Full-source/argument dependencies remain conservative, including INDEX's cycle
limitation. This is not a promise of dynamic selected dependencies, array literals,
named table references, Excel locale collation or full Excel parity.

## Verification

Require native bounds/coercion/error tests, transpose property, shared expected/
native/browser fixtures, stateful patch parity, real PostgreSQL/editor/restart
checks, structural references and measured table benchmarks. Final outcomes will
be recorded before acceptance. No mocked production storage or new infrastructure.

Sources: [Microsoft VLOOKUP](https://support.microsoft.com/en-us/excel/functions/vlookup-function),
[Microsoft HLOOKUP](https://support.microsoft.com/en-us/excel/functions/hlookup-function),
[Microsoft Learn VLookup](https://learn.microsoft.com/en-us/office/vba/api/excel.worksheetfunction.vlookup).

## Recorded outcome

Seven native table contract/resource/property tests and two dependency/structural
tests passed.443 expected/native/browser cases and1010books/2020patches converge;
live PostgreSQL50 HTTP tests,86 browser tests and2 hard-restart tests passed.
Frontend83 unit tests, typecheck/lint/build, Rust clippy and SQLx checks passed.
Read-only adversarial review found no concrete defect and ran no tests itself.
Benchmarks and limitations are recorded in
[the preliminary report](../benchmarks/2026-09-30-table-lookup-local.md).
Acceptance covers this slice only, not full Excel compatibility, all MVP phases
or production deployment. Detailed command outcomes are in RUNBOOK.
