# ADR-081: atomic durable cut/move

Status: ACCEPTED FOR EXACT-HEAD CONTENT MOVE (2026-09-30).
Undo of moves and WASM optimistic move preview remain open.

## Context

ADR080 supplies frozen planning. ADR079 copy events and compensations have
1000-write caps and different semantics: they cannot represent a cut with source
clears and references outside the rectangle. Copy-plus-clear is not atomic and
does not preserve inbound references. Existing history/event bytes stay unchanged.

## Decision

`CommandPayload::MoveRange` carries stable source/destination top-left addresses
and dimensions; it has no client AST or calculated values. Additive wire v1
`DocumentCommand.move_range` field10 is mutually exclusive with all other command
payloads. Unsupported dependency refusal is code18; source-version change is17.
An older server cannot apply this unknown payload as another operation.

The authenticated document actor checks permission and resolves a durable
operation identity before requiring the command base to equal the current head.
It runs the frozen Rust planner against that head, then validates one candidate
and prepares the content hash before the existing ACL-locked CAS transaction.
The journal repeats exact-head admission after receipt lookup; stale attempts
produce no new event, version or receipt. COMMIT precedes actor publication/ACK.

New `Operation::MoveRange` contains geometry plus complete guarded writes.
Dedicated storage schema19/oneof19 and event hash tag21 preserve schemas1–18;
command fingerprint tag17 preserves old intent fingerprints. Principal migration
allows19 only for user events. Limits:1–1000 source cells, up to10000 unique frozen
writes, independent512KiB incoming and preimage text caps,4MiB persisted envelope.
Source-only cells clear, destination values/blank equal frozen sources, and every
source/destination coordinate is represented even when blank. Replay never derives
new contents by rereading sources; it validates recorded preimages and mappings.

Candidate validation checks formula AST schema/root/shape, unchanged literals,
operators and authored/canonical text, exact once-mapped stable references,
preserved absolute modes and correctly recomputed relative deltas. It also scans
omitted external formulas: an affected omitted formula or partial range rejects
the event rather than leaving an inbound dependency behind. External writes
must be formula rewrites; arbitrary external value writes are rejected.
Scanning is bounded by existing workbook budgets and a1M external-node cap.

Calculations reconcile the complete changed address union incrementally. A
committed move has no v1 canonical content patch; the existing authorized HTTP
projection refresh and replay barrier fallback deliver source clears, destination
contents and cross-sheet inbound formulas together. No client result is confirmed
from a WASM preview. Calculation input-version/epoch rules remain unchanged.

## Delayed formulas, history and retry

Axis history records a content-free relocation floor after publication. Formula
commands based before a cut receive SourceVersionChanged rather than binding
against changed content semantics. A fresh actor restores bounded historical
transitions even when axis order did not change; encountering a move restores
the same refusal. This is conservative exact semantic admission, not silent rebase.

A move is currently an explicit history barrier, not an eligible UndoCell target.
The client clears its local undo/redo stack only after a resolved move ACK/receipt.
Earlier scalar/range history cannot cancel a move's touched addresses. Restoring
any historical formula across a later move is NotUndoable, including overwritten
formulas that were not live when the move was planned. Move compensation needs
its own versioned representation and guards for newly introduced references;
schema18 limits will not be widened retroactively.

Cut clipboard metadata is untrusted intent, validated by document/sheet, stable
geometry and base version. Same-document cross-tab paste is supported. The source
remains until durable commit/projection refresh; native text cut inside the DOM
editor stays native. A stale source or unsupported partial range never falls
back to TSV/copy. Retryable QueueFull/Unavailable retains identical bytes and
operation ID even after an uncertain ACK; a new cut/paste is blocked until that
identity resolves. Terminal conflicts require a new user cut.

## Scope and migration

Only content moves within one sheet are supported initially. Scalar inbound
references on any sheet and fully contained ranges follow the cut. Partial ranges,
cross-sheet moves, styles/merges and arrays remain explicitly unsupported. The
reference rules are the documented PXEL subset; manual Excel goldens remain open.
Deploy the additive migration and matching server/client together. Older events,
snapshots and hashes remain readable; old clients refresh new moves as projections.

Verification commands/outcomes are recorded in RUNBOOK. Pure planner parity,
worker preview, reference-aware move undo, controlled commit p95 and full remaining
MVP/deployment gates are separate acceptance requirements.
