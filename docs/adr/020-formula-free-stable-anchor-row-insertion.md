# ADR-020: Stable-anchor insertion before formula transformation

Date: 2026-09-29. Status: ACCEPTED FOR PHASE 4 FIRST INSERTION SLICE. Full structural formula compatibility remains open.

## Context

`ExtendRows` proved the durable structural path but could not insert inside a sheet. Existing formulas persist authored source text and stable-ID-bound references. Moving rows without updating visible A1 text and relative deltas would create misleading editing and copying behavior. There is no verified transformation or formatter yet.

## Decision

`InsertRows` names the existing row ID before which 1–1,000 new UUIDv7 row IDs are inserted. The anchor is validated at the current actor head; it is never interpreted as a stale position. Both concurrent insertions before the same anchor survive in committed order: the earlier batch precedes the later batch. Existing cells keep their stable row IDs. The domain rebuilds the ID→position index atomically in the candidate state. The normal CAS, event hash, idempotency and durable ACK rules apply. The event uses persisted schema 5 and an additive wire v1 field.

Until formula transformation is implemented, insertion is rejected with `UnsupportedStructuralDependency` if **any** sheet contains a formula. This conservative workbook-wide check avoids silently changing cross-sheet references. The rejection is not retryable and creates no journal event. Plain value edits addressed by stable row ID may be rebased across insertions. A formula command older than the latest axis change still requires resynchronization. The browser does not preview uncommitted axes; after commit or remote hint it refreshes the authorized canonical projection.

The editor exposes “Строка ↑” for one row before the selected row. `ExtendRows` remains the automatic way to grow at the bottom. The footer reports actual axis sizes.

## Consequences and next step

This is a usable, safe subset of insertion, not the phase-4 gate. Next, implement a versioned reference transformation with bound-target preservation, recalculated relative deltas, current A1 rendering, range handling and cross-sheet fixtures. Only then remove the formula guard. Delete rows, columns, copy/paste and guarded undo remain separate slices. Vector insertion is O(rows) within the 100,000-row cap; any replacement needs a benchmark and correctness test.

## Verification

Domain tests cover stable identity, atomic formula rejection and replay hash. Persisted codec tests cover schema-5 round-trip and anchor-dependent command fingerprint. Live integration tests cover simultaneous insertions, retry, stale cell write and typed formula rejection. Browser tests at DPR 1/2 cover the visible action and reload. Recovery test exercises restart after insertion.
