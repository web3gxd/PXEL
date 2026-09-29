# ADR-023: Stable column insertion with bound formula rebase

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET INSERT COLUMNS SLICE.

## Context

Rows already have stable IDs and versioned structural events. A positional column insertion must preserve authored cell identities and formula targets while updating the displayed A1 addresses and relative copy offsets. The previous schema-7 `DeleteRows` format must retain its historical meaning.

## Decision

`InsertColumns` names a live stable column anchor and supplies 1–100 new IDs. The server normalizes it to `InsertColumnsRebased`, persists schema 8, and inserts the IDs immediately before the anchor. Multiple concurrent inserts before the same anchor follow the committed server order. The candidate recalculates relative column copy offsets for all bound references after the axis change; absolute modes and stable target IDs remain unchanged. The formula source and canonical authored text are retained; `current_text` is rendered against current axes. The calculation graph rebuilds after the committed structural operation.

The server validates unique IDs across the workbook, the 1,000-column cap, live anchor, all transformed formulas, and rendered text before CAS. Invalid commands create no durable event. A repeated operation ID returns the original receipt. Wire field 7 and durable operation field 10 have separate compatibility lifecycles. Schema-8 replay reexecutes the same transform and verifies the domain hash; older schemas are not rewritten.

## Limits

The current book has one sheet. Historical-axis binding of a stale formula command still requires resync. Column deletion, inverse/undo, copy/move/cut, merges and XLSX column metadata are later contracts. The transform currently scans all formulas; a benchmark and correctness tests accompany this choice. No snapshot migration is needed until snapshots exist.

## Verification

Domain and binder tests check stable targets, relative offsets, current A1 text and replay hash. Codec tests cover schema-8 round-trip and command fingerprint. PostgreSQL/WebSocket integration tests cover formula recalculation, idempotent retry and concurrent insert order. Hard restart and browser DPR 1/2 are recorded in RUNBOOK. The benchmark measures only the optimized domain candidate.
