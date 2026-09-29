# ADR-022: Stable row deletion and reference tombstones

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET DELETE ROWS SLICE.

## Context

ADR-021 defines insertions that preserve stable formula targets. Deleting a row needs a distinct persisted meaning: a reference to removed content must not silently start pointing at the next visible A1 row, and a removed stable ID must never be assigned again. Historical event schemas 1–6 cannot be reinterpreted.

## Decision

`DeleteRows` is a bounded client command (1–1,000 distinct live row IDs) and a schema-7 durable event. The candidate removes sparse cells on those rows, updates workbook text/AST budgets, removes the IDs from the ordered axis, and records them in a retired-ID set. At least one row must remain. `ExtendRows` and `InsertRows` reject an ID in that set. Version, hash and formula transforms are prepared before CAS; replay executes the same schema-7 semantics and checks the resulting hash. The retired set participates in the domain hash. It is currently capped at one million IDs as a resource budget.

Bound scalar references to a deleted row become typed invalid references and display `#REF!`. For a bound rectangular range, a removed endpoint moves inward to the first/last surviving row; a range with no surviving row becomes `#REF!`. Other stable targets keep their identity, and relative copy offsets are recalculated against surviving axes. `Formula.source` and persisted canonical text remain authored history; current A1 text is rendered from the transformed bound AST. The calculation graph rebuilds after a structural commit. A stale write to a removed row receives a typed rejection; retry of the original operation ID receives the durable receipt.

Schema 7 has its own semantic event tag, independently of the WebSocket field number. Migrations permit schema 7 without rewriting older events. A future snapshot schema must encode the retired-ID set and restore it before replaying the tail; snapshot activation is blocked until this is tested. The current implementation uses full formula scanning for structural changes and a linear retired-ID hash suffix. These costs are measured separately and are not extrapolated to large workbooks.

## Limits

The workbook remains single-sheet. This slice does not define column deletion, merge interaction, copy/move/cut/sort, inverse/undo, or historical-axis binding of a stale new formula. Those remain separate contracts. Current range endpoint behavior is the internal convergence profile; Excel parity needs verified fixtures before it can be claimed. Removing one row creates one version; no partial mutation is committed on validation failure.

## Verification

Domain tests cover scalar `#REF!`, partial/fully removed ranges, sparse content cleanup, ID non-reuse and hash-equal replay. Formula tests cover current display text. Codec round-trip and fingerprint tests cover schema 7. Real PostgreSQL/WebSocket integration checks commit, retry, stale write rejection and calculation; hard restart checks durable replay. Browser DPR 1/2 and the structural benchmark are recorded in RUNBOOK.
