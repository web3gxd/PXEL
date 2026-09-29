# ADR-021: Versioned row-reference rebase

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET INSERTION SLICE. Delete/copy/move and multi-sheet compatibility remain open.

## Context

ADR-020 allowed `InsertRows` only in formula-free books. A stable bound target still calculates after a row insertion, but its relative copy offset and visible A1 address can change. Updating only one of these would make editing and later copying inconsistent. Persisted schema 5 must keep its historical meaning.

## Decision

New commands normalize to `InsertRowsRebased`, persisted event schema 6, while the wire protocol keeps the same client `InsertRows` intent. The schema-5 operation and its formula guard remain available only for replay of older events. Schema 6 inserts IDs before a stable anchor, then deterministically rebases every formula's relative row offset against the new axes and sets `bound_at_version` to the new document version. Absolute row modes and stable target IDs do not change. Range endpoints follow their stable IDs; inserting between endpoints expands the displayed rectangle. The candidate validates every formula and the current rendered text before CAS. Failure writes no event. Calculation graph rebuilds after a structural commit.

`Formula.source` remains the text authored at its original edit; `Formula.canonical` is the canonical text at binding. Neither is silently rewritten during insertion. The server derives `current_text` from bound targets and current axes for HTTP projection. The browser edits/previews `current_text` only when it differs from the stored canonical representation; otherwise it preserves the author's source text. This is a read model, not a second source of truth. The server continues to bind new formula commands against the current axes and rejects formula commands older than the last axis change.

The schema-6 variant has a distinct semantic event-hash tag. Command fingerprints continue to identify client intent, so an idempotent retry after commit returns its receipt even though the committed operation has a server-only variant. The database migration permits schema 6 for user operations; older payloads and hashes are unchanged. Replay applies the same deterministic transform and verifies the resulting domain hash. Any future change to this transform requires a new persisted schema; old schema-6 replay behavior must remain available.

## Limits and migration

The current workbook is single-sheet. Cross-sheet reference handling is implemented in the pure rebase lookup but must gain a multi-sheet fixture before declaring multi-sheet structural compatibility. The transform scans all formulas and rebuilds the graph; its initial 10,000-formula candidate benchmark is recorded separately. Delete, column insertion, copy/move, cut and rectangular sort require their own semantics and event versions. Historical schema-5 events are not rewritten. No snapshot migration is needed yet because snapshots are not implemented.

## Verification

Domain replay test checks stable targets, relative delta and hash. Codec test keeps schema 5 and checks schema 6 round-trip and distinct event hash. Real PostgreSQL/WebSocket integration checks the shifted A1 display; hard restart recovers the same formula and hash. Browser DPR 1/2 tests insertion and reload. Full local quality checks and benchmark are recorded in RUNBOOK; Linux/Compose and multi-sheet gates remain open.
