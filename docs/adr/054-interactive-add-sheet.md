# ADR-054: Authenticated interactive AddSheet

Date: 2026-09-29. Status: ACCEPTED FOR THE ADD-SHEET SLICE.

## Context

ADR-051 introduced a durable `AddSheet` event for local XLSX import. Wire v1 could not carry it, so replay across that event required a full projection. The UI showed imported sheet tabs but its add button was disabled. The domain and snapshot schema already support up to 64 ordered sheets.

## Decision

Add `CanonicalOperation.AddSheet` at new wire v1 field 10. The command carries the proposed sheet UUIDv7, a name and ordered stable row/column UUIDv7 arrays. The server checks the 1–1000 row and 1–100 column command bounds, ID shape and duplicates. The domain checks workbook-wide ID uniqueness, a unique bounded name and the 64-sheet ceiling. The browser creates a 1000×26 empty axis seed and shows the new tab only after the server commits it. Concurrent additions append in CAS order; a name collision is rejected atomically and the client may retry after refresh.

`AddSheet` uses the existing typed command, candidate-state validation, authorization, CAS transaction, event schema 15, semantic event hash and idempotency receipt. No persisted event or snapshot migration is needed. A user must have editor or owner access; an unauthorized actor cannot create a sheet. The canonical wire event enables bounded replay as `DocumentHeadChanged.latestOperation`; clients currently refresh the full projection for a structural operation before changing the active tab. Stale formulas remain subject to the existing axis-version rebase guard.

## Consequences

Interactive add now works locally and reaches another browser, including after reload. Sheet rename/delete/reorder and one-step undo of `AddSheet` are not part of this slice. The client waits for commit rather than drawing an optimistic tab, and the current structural refresh transfers all sparse cells. Large-workbook refresh cost requires a separate benchmark before optimizing the patch format. Existing wire fields and old journal bytes are unchanged.
