# ADR-019: Append-only row extension as the first structural slice

Date: 2026-09-29. Status: ACCEPTED FOR PHASE 4 FIRST SLICE. This does not approve insert/delete, copy/move, sort or undo semantics.

## Context

An initial sheet allocates 1,000 stable row IDs and 26 stable column IDs, but no empty cells. The viewport must grow as a user reaches the bottom. A structural operation must survive commit, retry, replay and concurrent clients before we add insertion at arbitrary positions.

## Decision

`ExtendRows` carries a sheet ID and 1–1,000 new UUIDv7 row IDs. The actor validates uniqueness, the 100,000-row workbook cap and the current sheet, then appends the IDs at the current end. Concurrent extensions are serialized by the document actor and PostgreSQL CAS; both batches remain, in committed order. Existing row IDs and positions stay unchanged. The UI sends one batch automatically near the last 20 rows, without creating cell records.

The operation has a distinct persisted event encoding (schema 4) and an additive wire v1 field. Replay derives `last_axis_change_version`. A formula command whose base version precedes an axis change receives `RebaseWindowExceeded` and resynchronizes. This is deliberately conservative until historical axis projections and reference transformation are implemented. Existing formulas are rebuilt after extension; the structural event itself has no optimistic axis preview. Browsers fetch the authorized canonical projection after a structural head hint or acknowledgement.

## Consequences

Append is a low-risk entry point for durable structure and stable identity; it does not solve insertion/deletion or formula editing across structural versions. The projection refresh costs an HTTP transfer, acceptable for this first bounded slice and subject to a measured replacement. An extension retry uses the existing operation ID receipt and cannot append twice. The domain state hash includes ordered row IDs; its format has not changed.

`ExtendRows` has no user-facing undo yet. Its eventual inverse must be guarded against intervening edits and extensions; deleting the appended rows blindly would violate history and other users' work.

## Verification

Domain tests cover stable cell identity, duplicate rejection and direct/replay hash equality. Integration tests cover two concurrent clients, idempotent retry, stale formula rejection and replay. Recovery tests cover process restart; Chromium tests at DPR 1 and 2 cover automatic extension, second-tab refresh and reload. The remaining phase 4 gate requires insert/delete, reference transformation, batch copy/paste and guarded compensating undo.
