# ADR-070: INDEX reference selection

Status: ACCEPTED FOR CONTIGUOUS INDEX SLICE (2026-09-30).

## Context

The shared engine needs lookup functions without losing reference coercion or
introducing dynamic arrays implicitly. INDEX is the first lookup slice.
Durability, bound stable identities, separate confirmed/preview results and
native/WASM convergence remain mandatory.

## Decision

Engine identity advances to pxel-formula-0.9.0. AST, wire and persisted event
schemas stay unchanged. A private intermediate selection preserves a contiguous
reference through nested INDEX and aggregate arguments. Scalar evaluation reads
one selected cell; aggregate evaluation visits the selected rectangle in fixed
row-major order. Blank scalar results become zero, while COUNT/COUNTA and numeric
aggregates retain reference blank/text/boolean/error handling.

Selectors are one-based; zero selects the whole corresponding axis. Out-of-bounds
selectors and a non-one area produce #REF!. Only one contiguous area is supported.
Negative selectors produce #VALUE! before truncation. Positive fractions truncate;
blank, boolean and numeric text use the existing numeric coercion profile. These
coercion details are provisional PXEL semantics until manual Excel goldens exist.
Two-argument vertical references default to column one. Compact horizontal/2D
omission, multi-area expressions and dynamic-array output remain unsupported
rather than guessing their interpretation. Unsupported/resource errors cannot
be hidden by IFERROR. Nested reference selection does not eagerly resolve cells.
Scalar sources such as INDEX(42,1) are treated as a one-cell value source.

The full source rectangle is bounded by the existing 100,000-cell limit, even
when selection reads one cell. Selector text, traversal, depth and cancellation
remain charged. Resolver range cardinality is checked.

The dependency graph initially retains the complete bound source rectangle and
selector dependencies. Unselected source edits may recalculate INDEX, and a
formula inside its own source can produce a conservative cycle. Dynamic selected
reference dependencies require a later graph change with structural-edit and
selector-transition tests; this limitation is not full Excel compatibility.

## Validation and follow-up

Require shared expected/native/browser fixtures, stateful WASM patches, selector
and source edit propagation, live PostgreSQL/editor reload tests and range
benchmarks before accepting this slice. Manual Excel verification, arrays,
multi-area references and reference-valued colon operands remain open work.

Observed verification:367 expected/native/browser fixtures,1010 stateful books
with2020 patches, native reference/resource and graph tests,48 live HTTP cases,
82 browser cases at DPR1/2 and2 hard-restart recovery cases passed. Recovery
explicitly rebuilds INDEX from the durable journal and checks content/result
hashes, old receipt and subsequent source recalculation. Frontend83 unit tests,
clippy/typecheck/lint/build/format and SQLx checks passed. Preliminary local100k
bound evaluator timings are documented separately. This acceptance excludes
manual Excel, dynamic selected dependencies, unsupported forms and general MVP
production/load/deployment gates.

Reference: [Microsoft INDEX documentation](https://support.microsoft.com/en-us/excel/functions/index-function).
The profile above is our explicit implementation contract; it does not assert
bitwise Excel parity or that every documented Excel form is implemented.
