# ADR-083: durable guarded move undo and redo

Status: ACCEPTED FOR SAME-SHEET CONTENT MOVE HISTORY (2026-09-30).
Broader structural/style history and optimistic move preview remain open.

## Context and previous decisions

ADR081 kept moves as history barriers. ADR082 proved frozen inverse/reference
planning but conservatively refused even neutralized own edits. An inverse can
contain10000 writes; widening schema18's1000-write limit would change historical
semantics. Reapplying a backwards move would lose overwritten contents and exact
external ASTs. The reference guard must run inside locked commit admission, where
new foreign writes cannot race the check.

## Decision

MoveRange and its own MoveCompensation become eligible UndoCell targets. The wire
intent remains unchanged. A dedicated user-only storage schema20/oneof20/hash22
records original source/destination geometry, the complete ordered swapped frozen
writes and mandatory target operation ID/version. Existing schemas and hashes,
including19 and command fingerprints, remain unchanged. Redo is another own
compensation targeting the committed undo, with a new operation ID/version.

The actor loads the authorized own target, derives the exact inverse, checks live
destinations, validates one private final-capacity candidate and hashes it before
commit. Domain validation requires complete original rectangle union, unique
footprint up to10000writes, exact current preimages, coherent live formula origins/
binding versions and independent512KiB incoming/preimage text caps. It restores
ASTs without executing the forward relocation algorithm. The persisted envelope
remains capped at4MiB; source geometry remains1–1000 cells on one sheet.

## Transactional reference admission

Move history planning now lives in domain because it only uses domain content and
bound-reference types. Formula reexports the previous API. Storage keeps its
existing dependency boundaries and does not acquire an evaluator dependency.

An internal CommitRequest may carry an Arc of the confirmed input workbook for
MoveCompensation. This is neither a transport field nor client-supplied state.
After existing receipt lookup, ACL/document locking and expected-head checks, the
journal verifies its document, version and content hash against the independently
decoded/checksummed/semantic-hashed durable head event. Missing or mismatched
context is rejected. A late retry resolves its receipt before requiring context.

The journal validates exact target ownership, ID/version, operation family,
preserved move geometry and complete ordered swapped content/preimage pairs.
It scans verified contiguous history and reruns the domain inverse planner under
the same document lock, before CAS, insert and COMMIT. Foreign same-value writes
remain conflicts; projection equality alone never supplies eligibility.

Own inverse pairs reduce per stable address only when operation ID, version,
principal and exact swapped contents match. Foreign/legacy entries cannot be
neutralized. Effective newly written formulas referring to inverse addresses,
including sparse external formula addresses and range interiors on other sheets,
block undo. A foreign referencing formula remains a barrier even if later cleared;
an exact own formula-plus-compensation pair can cancel. Later effective moves are
global barriers; fully neutralized own moves no longer block earlier own formula
history. Later axis changes remain conservative refusals.

Limits remain10000tail versions,64MiB persisted payload bytes and100000 matching
address updates. The domain planner charges writes, AST inspection/equality and
indexed range rows against1000000work units. It returns no partial inverse.
Currently the generic touch scan and reference scan each read the bounded tail;
consolidation requires profiling and preserved correctness, not a new abstraction.

## Delivery, calculation and UI

MoveCompensation reconciles all changed dependency nodes incrementally. It uses
the existing authorized projection-refresh/replay barrier fallback, preserving
confirmed/calculated/pending separation. Axis history advances its relocation
floor on compensation too, including recovery, so stale formula text never binds
against the wrong relocation semantics. COMMIT still precedes publication/ACK.

Acknowledged moves now enter the existing bounded100-entry own undo history.
Duplicate ACK/receipt does not advance history twice. Retryable uncertainty retains
the original bytes/ID. Undo/redo remains disabled while unresolved or refreshing;
the UI does not manufacture confirmed inverse contents or a WASM move preview.

## Migration and remaining scope

Deploy the new principal allowlist migration and codec/server together. Older
events/snapshots remain readable, and old clients can refresh unknown canonical
content changes. Older servers reject unsupported move history; clients cannot
emulate it as independent clear/copy writes.

Same-sheet content moves and scalar/contained inbound ranges are supported.
Cross-sheet moves, partial-range union, style/merge movement, axis/style inverses,
durable discovered user history, worker move preview/parity and manual Excel
goldens remain required later work. Current limits may reject otherwise valid
large histories without changing the head.

## Evidence

Actual native/domain/codec, genuine PostgreSQL, HTTP, DPR1/2 browser, restart and
parity outcomes are recorded in RUNBOOK. Tests cover ownership, foreign equal
values/new references, neutralized own history, forged family/geometry/context,
inverse footprints exceeding schema18 capacity, retry receipts and exact replay.
No whole-MVP, controlled latency or production/deployment gate is closed here.
