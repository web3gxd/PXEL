# ADR-085: guarded row and column Undo/Redo

Status: ACCEPTED FOR EXPLICIT AXIS UNDO/REDO (2026-09-30).
Local integration verified; wider MVP and deployment gates remain open.

## Previous decision and problem

ADR083 supports guarded content moves. Explicit axis edits were durable but
could not be undone. Reusing a removed stable ID without recording its revival
would also admit commands authored before deletion after an Undo restoration.

## Decision

Explicit row/column insertion and deletion become frozen AxisEdit events with
ordered surviving anchors, inverse geometry, axis-order hashes and exact content
and changed-formula preimages. Automatic ExtendRows remains outside user history.
AxisCompensation identifies its own user's target operation and committed version.
The journal derives both canonical edits and inverse candidates again against an
independently hash-verified projection of the locked PostgreSQL head. Candidate
application precedes CAS; publication and acknowledgement follow durable COMMIT.

Undo restores retired IDs, but ordinary insertion can never reuse retired IDs.
Retained per-ID activation floors prevent stale commands, formula origins and
references from crossing a restoration. Range interiors are checked on historical,
intermediate and proposed axes before an ID can disappear through transformation.
Unrelated old-base edits remain admissible. The transactional document activation
marker makes verified projection context mandatory after any restoration; callers
cannot bypass lifecycle admission by omitting context. Snapshots retain floors,
including those for IDs subsequently removed again.

Own exact inverse pairs can neutralize earlier own history. Foreign equal-value
writes, foreign reference writes subsequently cleared, incompatible structural
history and moves remain conflict barriers. Range checks include reversed,
cross-sheet and newly restored interior IDs. Removal vector order is semantically
a set; insertion placements remain ordered. Structural pairs may have additional
offset patches for independent later formulas, so their provenance and geometry,
rather than literal equality of every patch, establish cancellation.

Inspection is bounded by 10000 events, 64 MiB durable tail, one million work units
and 100000 matching touches. Original and anticipated immediate inverse envelopes
must fit 4 MiB, with 32 bytes reserved for future metadata varint growth. Later
independent formulas can enlarge an inverse and still cause a typed size refusal.
These limits do not promise that every future Undo remains admissible.

Persisted event schemas 21/22 and semantic hash tags 23/24 are independent of the
unchanged public command protocol. Snapshot schema 3 writes activation floors;
readers retain schemas 1/2. Unknown operation schemas fail atomically; ordinary
unknown optional protobuf fields retain the existing compatibility policy.
Formula evaluator identity stays 0.14.0 because numeric/function semantics do not
change. Diagnostic spans never enter the persisted AST.

## Verification and remaining work

Domain, codec, historical binding and genuine PostgreSQL tests cover exact
restoration, lifecycle admission, snapshots, forged projections, own/foreign
history and size boundaries. Actual outcomes are recorded in RUNBOOK.md.
Frontend history, live protocol, hard restart and full integration checks passed
locally. Durable history discovery and style history remain
separate requirements; this ADR does not close all phase-4 or wider MVP gates.
