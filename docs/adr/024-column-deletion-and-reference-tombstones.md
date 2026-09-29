# ADR-024: Stable column deletion and reference tombstones

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET DELETE COLUMNS SLICE.

## Context

ADR-023 inserts stable column IDs and rebases relative formula offsets. Deleting a column must not silently retarget a reference to the next visible A1 coordinate. Schema 1–8 events remain immutable and must replay with their original semantics.

## Decision

`DeleteColumns` removes 1–100 distinct live stable IDs in one schema-9 durable event. At least one column remains. The candidate removes sparse cell contents on those columns, updates text and AST budgets, and records retired IDs in a capped set. Future insertion rejects a retired ID. The retired set participates in the domain hash; replay must reach the same hash.

Bound scalar references to deleted columns become typed `Invalid(DeletedTarget)` and display `#REF!`. Range endpoints move inward to surviving columns; a fully deleted range becomes `#REF!`. Relative column offsets of surviving targets are recomputed after the axis change. Formula source and canonical authored text remain historical; `current_text` is rendered from the bound AST and current axes. The calculation graph rebuilds after a structural commit.

Validation and transformation happen in a private candidate before the durable CAS transaction. A repeated operation ID yields a receipt, while a new command targeting a removed column receives a typed rejection. Wire field 8 and persisted field 11 have separate compatibility lifecycles. Schema-9 replay applies the same transform; no older event is rewritten.

## Limits

The current book is single-sheet. The retired set is capped at one million IDs and must be encoded by the future snapshot schema before snapshots can activate. Historical-axis binding for stale new formula text still requires resync. Undo, copy/cut/move, merges, sort and XLSX column metadata remain separate work. The current transform scans all formulas; benchmark and correctness tests cover this implementation, not a general latency guarantee.

## Verification

Domain tests check scalar `#REF!`, ID non-reuse and replay hash. Binder tests check range shrink and current A1 display. Codec tests check schema-9 round-trip/fingerprint. PostgreSQL/WebSocket integration covers commit, calculation, retry, stale write and ID reuse rejection; hard restart covers durable replay. Browser DPR 1/2 and the optimized domain benchmark are recorded in RUNBOOK.
