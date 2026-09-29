# ADR-051: Durable multi-sheet workbook and XLSX import

Date: 2026-09-29. Status: ACCEPTED FOR THE BOUNDED SCALAR IMPORT SLICE.

## Context

The domain hash already included sheets in order, and the XLSX reader/writer handled multiple worksheets. Document import and snapshots still assumed one sheet. Silently flattening imported worksheets would change formula targets and lose data.

## Decision

`CreateWorkbook` remains the genesis event. Each additional imported sheet is an immutable `AddSheet` event (persisted event schema 15) carrying a validated seed with stable sheet, row and column IDs. Sheet names are unique under the domain's Unicode lowercase comparison, and axis IDs are unique across the workbook. Import prepares all sheets before binding any formulas, then publishes all events in one PostgreSQL transaction. The existing 1,000-cell and 64-sheet limits still apply.

Snapshot schema 2 stores an ordered list of sheets and their sparse authored cells. The decoder still accepts schema 1 as a single-sheet image; the writer emits only schema 2. PostgreSQL metadata accepts both versions. Neither format includes calculated cache. Snapshot restoration creates every sheet before restoring formulas and checks the canonical domain hash. Existing event and snapshot bytes are never rewritten.

Wire protocol v1 has no canonical `AddSheet` message. A subscriber crossing this event is sent to full projection resync, as it already is for genesis. The event remains durable and fully replayable by the server. Interactive add/delete/reorder sheet commands require separate wire semantics and authorization tests. Client-only sheet selection is specified in ADR-052.

This was the behavior of the import slice. [ADR-054](054-interactive-add-sheet.md) adds the wire v1 `AddSheet` field and interactive command; old persisted schema-15 events are unchanged.

## Consequences

This is scalar XLSX multi-sheet preservation, not full spreadsheet feature parity. Styles, merged cells, metadata and unsupported formulas still follow the explicit compatibility loss policy. The import command remains local and dev-auth gated. A later snapshot schema change must retain an explicit old-reader path; schema 2 snapshots cannot be read by a schema 1-only server. The database migration extends the allowed event and snapshot schema versions before writing them.

## Verification

The binary codec reads both schema 1 and schema 2. A PostgreSQL test commits `CreateWorkbook`, `AddSheet` and two sheet values atomically, publishes a schema-2 filesystem snapshot, commits a second-sheet tail edit and compares snapshot-plus-tail recovery to full journal replay by canonical hash. This verifies the local filesystem adapter and PostgreSQL path; S3 implementations require their own contract runs.
