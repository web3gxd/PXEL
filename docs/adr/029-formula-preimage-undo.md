# ADR-029: Restore a formula preimage only across unchanged axes

Date: 2026-09-29. Status: ACCEPTED FOR THE NEXT PHASE-4 UNDO SLICE.

## Context

ADR-028 stores a formula as the preimage of a later single-cell value or clear, but refuses to restore it. The historical bound AST retains stable targets and relative-copy offsets. While the formula is absent from the workbook, structural commands cannot transform its stored preimage. Restoring that AST after an axis change could show the wrong A1 text or retain an obsolete range boundary.

## Decision

`UndoCell` may restore that formula when the document's `last_axis_change_version` is no greater than the target write version. The actor checks this after refreshing the authoritative projection and loading the target. Any intervening row/column extension, insertion or deletion returns typed `NotUndoable`; the client must not silently rebind the historical source against today's axes. The existing journal guard still rejects every later write to the stable cell, and PostgreSQL CAS retries the complete actor path if the head changes while preparing the candidate. A successful restoration is schema-13 compensation with the original bound formula, a new document version and a canonical wire `SetCellFormula` patch. Calculation dirties the cell and dependents at that version.

The protobuf field numbers and domain hash format do not change. The persisted envelope uses schema 13 only for formula compensation; existing schema-12 value/blank compensation remains byte-compatible. A schema-12 envelope containing formula compensation is rejected, preserving the old reader contract. Authored schema-11 value/clear writes still cannot contain a formula result. Migration 012 adds schema 13 to the database constraint. A formula preimage from an older event without schema-11 metadata remains ineligible. This rule is deliberately conservative: even an unrelated axis change prevents restoration until historical AST transformation is specified and tested.

## Verification and limits

Domain and codec tests cover formula restoration, schema-13 round-trip and rejection of forged schema-12 formula payloads. A real PostgreSQL/WebSocket test verifies restore, calculation result `8` for `=A1+1`, and `NotUndoable` after row insertion. Playwright verifies the editor source before overwrite, after undo and after reload at DPR 1/2. Recovery tests replay the new schema after hard restart. General formula undo, formula source event undo, batch undo, structural inverse and redo remain open. Before extending this rule across axis changes, add an explicit historical transform with copy/move/delete fixtures and replay convergence tests.
