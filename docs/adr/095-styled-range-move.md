# ADR-095: Styled range move as one durable cell batch

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-02).

## Context

Legacy `MoveRange` freezes cell contents and formula relocation, but does not
carry sparse cell formatting. A separate style event would expose partial state
and require a second Undo. Existing schema 19/20 move events must retain their
historical content-only meaning.

## Decision

- New moves normalize to `CellBatchWrite` with a typed `MoveRange` intent. The
  source, destination, and rectangle are frozen in the event; the existing wire
  `MoveRange` command is unchanged.
- One candidate moves values/formulas, rewrites inbound formulas and replaces
  all sixteen sparse style leaves at the destination. Source-only cells reset
  to inherited defaults. Overlapping moves use a simultaneous source snapshot.
- Journal admission validates the operation through the domain candidate,
  checks exact command geometry and style preimages, and verifies Undo history.
  Compensation restores content and styles in one batch event.
- Application history reads the batch's `undoes` and `undoes_version` identity
  as compensation metadata, preserving Redo discovery after reload.
- Every consumer of durable Move history must classify both legacy
  `Operation::MoveRange` events and normalized `CellBatchWrite` events with a
  `MoveRange` intent. This includes history-tail cancellation for formula Undo
  and the formula-binding relocation floor; the persisted intent is authoritative
  even though the wire command retains its legacy name.
- Persisted schemas 32/33 use additive `CellBatchCopyIntent.is_move`; schema
  30/31 remain byte- and hash-compatible. Migration
  `202610020004_styled_move_batches.sql` expands the schema allowlist.
- Moves remain capped at 1,000 cells. Style writes can cover both source and
  destination (up to 2,000 addresses), validated and applied in bounded chunks
  using the existing style target limit.

## Consequences

New cut/move operations carry formatting through commit, reload, Undo and Redo.
Old persisted move events still replay as content-only operations. No new wire
operation or non-PostgreSQL authority path is introduced.

## Verification

Normalization, schema/hash round-trip, PostgreSQL-backed browser move, style
replacement and Undo checks are recorded in `RUNBOOK.md`. The 2026-10-02
formula-0.15 application run added 80/80 live PostgreSQL/WebSocket tests and
190/190 Chromium E2E tests; it caught the two history consumers that missed the
normalized MoveRange intent, and the final rerun passed after the fix.
