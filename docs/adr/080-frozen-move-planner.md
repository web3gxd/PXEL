# ADR-080: frozen cut/move reference planning

Status: ACCEPTED FOR PURE PLANNER ONLY (2026-09-30).
Server command, persisted move/compensation events and UI cut/paste are pending.

## Context and previous decisions

ADR079 range history captures preimages for copy/TSV. Copy shifts relative
references; moving a formula must preserve its targets unless the referenced
content itself moves. Implementing cut as copy plus clear would break inbound
references and overlap. Existing RangeWrite permits 1000 destination writes;
that does not fit 1000 disjoint source and destination cells plus inbound formulas.
It will not be silently reused or made to restrict source rectangles to 500 cells.

Microsoft documents that moving a formula preserves its references regardless
of absolute/relative modes: [Move or copy a formula in Excel](https://support.microsoft.com/en-US/Excel/move-or-copy-a-formula-in-excel).
This source does not establish golden parity for inbound references or partially
moved ranges. Those remain explicit PXEL subset semantics pending manual fixtures.

## Decision

The pure Rust formula crate exposes `plan_move` and cancellable planning over
an immutable workbook. A same-sheet source rectangle of up to 1000 cells maps
simultaneously into an already allocated destination. The mapping includes blank
cells and is applied exactly once, including overlap in either direction.
Source-only coordinates clear, destination contents replace, and all preimages
come from the input head. Formula scanning includes all sheets and excludes
overwritten destination-only formulas. There are no implicit axis extensions.

Moved formula origins relocate. Scalar bound references follow source targets
even with `$` flags; otherwise targets remain stable. Relative deltas are
recomputed against the new origin. Fully contained range endpoints follow the
mapping with their original order and absolute modes. Disjoint ranges remain.
A partial intersection rejects the complete plan as `PartialRange`; the current
bound AST cannot represent disconnected unions and endpoint-only changes could
silently lose referenced cells. Cross-sheet moves and identical rectangles are
typed refusals. Cross-sheet inbound references are supported.

Rewritten formulas preserve authored source/canonical text, like existing axis
rebases. The bound AST supplies current displayed A1 text. Their binding version
is the input head, strictly preceding any future committed event. The planner
returns that input version and canonical domain hash, not a durable operation.
Its output must never be trusted as a client-authored AST or as authorization.

Limits are independent: 1000 source cells, 10000 output addresses, 1000000
visited formula nodes. Cancellation is checked before planning, per source row
and per visited source/document cell; a formula has its existing 4096-node cap.
Any refusal drops the complete private plan and leaves the input untouched.
Scanning remains deterministic in workbook sheet/cell order. No new index or
parallel transform is introduced before profiling.
External ASTs are inspected by reference; AST/source cloning is restricted to
changed external formulas and moved source formulas. This avoids rebuilding
unrelated formulas simply to compare and discard an identical AST.

## Verification and remaining integration

Twelve native tests include both overlap directions, mixed absolute modes,
fully contained reversed ranges, cross-sheet inbound references, blank targets,
overwritten formula exclusion, cancellation, geometry, the independent output
limit, exact preimage restoration and property-based frozen value rectangles.
The entire formula suite passes; the WASM crate builds for wasm32. This build
does not prove relocation browser parity: no worker relocation API exists yet.
The 1000-cell / 5000-inbound-formula benchmark is recorded separately.

Next implementation must add a versioned frozen move event with a full guarded
footprint and atomic candidate validation, authorized exact-head command,
CAS/commit/replay, incremental recalculation and reconnect projection refresh.
Undo needs complete moved/external preimages and guards against foreign
reference edits; ordinary ADR079 destination-history checks alone are insufficient
for references introduced after a cut. Acceptance requires real PostgreSQL,
hard restart, two browsers and native/browser move fixtures. No move UI, storage
schema, wire message, engine version or migration changes in this planner slice.

Partial-range union support, cross-sheet moves, styles/merges and manual Excel
goldens remain explicit follow-up requirements, not silently advertised features.
