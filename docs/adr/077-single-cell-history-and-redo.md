# ADR-077: multi-step single-cell history and guarded redo

Status: ACCEPTED FOR BOUNDED SINGLE-CELL HISTORY (2026-09-30).

## Context

ADR028/029/076 cover a single authored cell write. A naive client stack fails
when the same cell is written twice: undoing the second write leaves a durable
compensation in the tail, so the original guard refuses the first write. Redo
must also preserve authorship, concurrent writes, stable identities and history.

## Decision

UndoCell is a guarded inverse intent. Its target may be an own authored CellWrite
or an own complete compensation CellWrite. Redo inverses the committed undo,
using its operation ID as target and a new command ID. The server derives both
inverse content and preimage from the immutable target. Kind/undo metadata must
be coherent; legacy writes, batches, copies and structural operations remain
ineligible here. This extends accepted intents without changing event bytes,
hash tags or the wire/storage schema. An older server can reject the new target
with NotUndoable atomically; a client must never emulate redo as an ordinary edit.

Within the locked CAS transaction, scan at most10,000 tail versions. Foreign
writes, legacy writes, batches and copies touching the address are barriers even
when their values match or they were later reverted. Relevant own CellWrite
events form a bounded stack of operation ID and version only. A compensation
whose undoes ID AND version match the stack top cancels that top; any other
eligible write pushes its own identity. The target is eligible only when this
effective tail stack is empty. Recorded compensation validity is independently
enforced by exact target/preimage/inverse checks at commit. No formula content is
retained in the tail stack. Actor candidate validation precedes the transaction;
receipt lookup precedes history checks and returns no historical document payload.

Examples: A,B,undo(B) permits undo(A); A,undo(A),redo(undo(A)) permits undo(A)
again; undo(A) alone prevents a second inverse of A. A foreign write never becomes
cancelled for another user's undo. Formula result or preimage still rejects any
axis change after the target (ADR076). Structural changes before a target keep
the exact stored AST; formula patches use projection-resync when necessary.
An address that has been deleted returns UndoConflict before the conservative
formula-axis refusal. The journal still validates the target/history inside the
transaction; malformed target versions cannot cancel a different history entry.

The client maintains bounded100-entry undo and redo stacks of acknowledged
operation IDs. Accepted undo pops its target and pushes the committed inverse ID
onto redo; accepted redo does the reverse. New acknowledged edits clear redo.
Unsupported own commands clear both stacks, avoiding an undo that silently skips
a nonundoable action. A nonretryable failure drops only its attempted top entry;
retryable failures and uncertain acknowledgements retain intent and identity.
Pending operations and confirmed projection remain separate. History is local to
the current document session, reset on reload/document switch. Durable history
discovery and batch/structural compensation are later required phase4 work.

## Verification

Direct PostgreSQL tests must prove same-cell multi-step undo/redo, exact inverse
pair cancellation, idempotency, no double inverse, foreign equal-value barriers,
axis guards and exact replay hashes. UI unit and Chromium DPR1/2 tests must prove
ordered undo/redo, new-edit branching, independent cells, formula restoration,
no optimistic confirmed inverse and remote convergence. Hard restart tests target
a persisted compensation and reconstruct content/calculated hashes. Existing
legacy codec hashes, guards, protocol checks and full local regressions must pass.
No new performance threshold is claimed; existing10k scan cost remains an open
profiling gate. All work is recorded in RUNBOOK before accepting this slice.

## Recorded results

Native domain/codec/collab suites passed (codec23, collab22/two benchmark ignores,
domain full stress intentionally ignored). PostgreSQL undo6passed/one timing
ignore; the500-pair timing harness was separately executed with durability on and
full replay hash equality. Wrong-version pairing and a foreign equal-value write
followed by its own compensation both reject. Frontend87unit tests passed,
including100-entry traversal, branching, nonretryable failure and overlapping
receipt/ACK regression. Full98Chromium DPR1/2 cases passed; final66HTTP/network/
security cases include source/compensation deletion error priority. Hard restart
tests cover redo of a persisted formula compensation and exact hashes/receipt.
SQLx/protobuf/clippy/default-server/format/typecheck/lint/build checks passed
locally. No wire/storage schema, engine version or dependency change. Batch,
structural and durable discovered history remain open; no whole-phase gate closes.
