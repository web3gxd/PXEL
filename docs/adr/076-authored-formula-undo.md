# ADR-076: guarded undo of an authored formula write

Status: ACCEPTED FOR AUTHORED FORMULA UNDO SLICE (2026-09-30).

## Decision

ADR028/029 cover new value/clear writes and their historical formula preimages.
New SetCellFormula commands now produce domain CellWrite with SetFormula kind,
server-bound formula content and exact previous content. Persisted schema16 adds
authored_kind3 to the existing storage message; semantic event-hash tag17 is new.
Legacy schema2 formula events remain readable and ineligible for this undo path.
Schema11 cannot silently admit a formula authored write. No wire schema change:
canonical patches remain SetCellFormula/SetCellValue/ClearCell, without preimages.
Formula-bearing writes and compensations with an older binding version use the
existing transformed/projection-resync response. Persisted source/canonical text
alone is not a current A1 view: an axis change before the overwritten target can
already have changed its preimage's displayed reference. This conservatively
refreshes even unchanged-axis inverses instead of publishing misleading text.

UndoCell retains current tenant/document authorization, actor ownership, bounded
10k tail history guard and new operation ID. Any later write to the target address
conflicts even if content is equal. Candidate/preimage validation precedes CAS;
target/history/ACL guards run again inside the transaction; durable COMMIT precedes
publication. Receipt lookup remains first, including after a lost ACK and later
edits. Compensation uses existing schemas12/13 for blank/value/formula restoration.

If either recorded result or preimage is a formula, intervening axis changes return
NotUndoable. Stored historical ASTs cannot be silently rebound or compared as though
they were structurally transformed. This extends the explicit ADR029 conservative
rule until historical inverse transformation has its own tested contract. Current
cell deletion or a later write remains UndoConflict. No other user's event is undone.

Frontend remembers its acknowledged formula write as the latest eligible single
cell operation. This slice extends eligibility, not multi-step/batch/structural
history or redo; those remain required subsequent phase4 work. Formula engine
identity remains0.14.0; calculation uses existing CellWrite invalidation.

## Required verification

Domain candidate/preimage and codec schema16 round-trip/downgrade/malformed/future
binding tests; legacy golden hashes unchanged. Actual PostgreSQL blank/value/formula
restoration, ownership/equal-write/axes/receipt/CAS checks; editor DPR1/2 and restart
source/hash/recalculation tests. Migration, protobuf generation, SQLx and lint/build
checks must pass. All changes recorded in RUNBOOK before accepting this slice.

## Verification

Domain undo5/5, codec23/23 including legacy hashes, PostgreSQL undo5passed/one
intentional timing ignore, collab22passed/two benchmark ignores and server17unit
tests passed. Final64HTTP integration/network/security cases,96Chromium DPR1/2
cases,83frontend unit tests and2hard-restart cases passed. Browser regressions
cover both delayed binding and a preimage transformed before the overwritten
edit. SQLx/protobuf/format/lint/build checks passed locally. No engine identity,
dependency or wire schema change. Broad phase4 history/batch/redo, controlled
performance/manual Excel and deployment gates remain open.
