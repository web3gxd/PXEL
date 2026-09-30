# ADR-082: frozen move inverse planning and reference barriers

Status: ACCEPTED FOR PURE INVERSE PLANNING (2026-09-30).
Durable move compensation, transactional admission and user-facing undo remain open.

## Context and previous decision

ADR081 deliberately leaves moves ineligible for UndoCell. Moving the destination
back is not an inverse: overwritten destination contents and external formula ASTs
must be restored from the immutable event. Equality with current contents also
fails to detect a later foreign same-value write. A newly created formula can
depend on either rectangle or a changed external formula, so checking only the
inverse's write addresses is insufficient.

## Decision

Add a pure `spreadsheet_formula::move_history` planner taking the current workbook,
a verified durable user MoveRange event and its verified contiguous canonical
tail. The caller must establish event codec/hash/provenance verification; this API
does not authenticate users or replace the journal's ACL, receipt or CAS checks.

The planner checks document identity, exact tail versions through current head,
terminal content hash and exact current contents at every target write. It returns
the input version/hash, target operation ID/version and the complete ordered frozen
inverse: each target's content and preimage are swapped without rebinding or
rereading the source. AST, original source, origin and binding version remain exact.

Any later axis change or move is a conservative conflict. Any later content write
to the inverse footprint is also a barrier, including equal values and writes
subsequently compensated. This first planner deliberately does not reduce own
neutralized history; that remains required for full multistep move undo/redo.
ADR077/079 transactional history reduction continues unchanged for existing types.

New formula contents in the tail are inspected across legacy SetCellFormula,
CopyRange, CellWrite and RangeWrite, including compensation contents. A reference
to any inverse write is a conflict even if that new formula was later cleared.
Unchanged pre-existing destination references are allowed. The footprint includes
external rewrites on other sheets. Cell references use stable identity; ranges
normalize reversed endpoints and test interior intersection using current axis
positions, valid because later axis changes have already been refused.

Limit the tail to10000 events, footprint to10000 unique writes, and inspection to
1000000 charged units across visited writes, AST nodes and intersected index rows.
A per-sheet row index of sorted columns avoids expanding ranges into cells.
Cancellation is checked at entry, per event/write/node/index row and before output.
No partial plan or projection mutation escapes an error.

## Consequences and next integration

This introduces no persisted schema, eligibility, wire message, browser behavior,
dependency or engine evaluation identity change. Move remains a history barrier
in the current application. Schema18's1000-write limit remains unchanged.

The next slice needs a separately versioned large-footprint compensation, exact
target/geometry/preimage validation under document lock/CAS, bounded verified tail
acquisition, reference guards rechecked at transaction admission, own inverse-pair
reduction, actor/calculation handling, UI undo/redo and real PG/restart/browser tests.
Planner acceptance does not constitute acceptance of those unimplemented pieces.

## Verification

Twelve native tests cover frozen scalar/formula restoration, overlap and overwritten
formulas, existing destination observers, independent and disjoint new references,
foreign same-value writes, removed references, copy/range/compensation routing,
reversed/interior/cross-sheet ranges, malformed tails/hashes, axes, cancellation
and a real10000-event candidate history exceeding the inspection budget.

Full formula tests, strict all-target Clippy and wasm32 compilation passed locally.
The new API has no browser worker binding; native/WASM inverse runtime parity is
not yet verified. Actual commands and benchmark conditions are in RUNBOOK and
the move benchmark document. No broader MVP/deployment gate is closed.
