# ADR-096: stable, guarded merged cell ranges

Interactive populated-cell merging is extended by [ADR-123](123-populated-cell-merges.md)
on 2026-10-06. Legacy events and import validation retain this ADR's conservative
meaning; the explicit browser action clears covered data and styles atomically.

Status: accepted and implemented as a bounded local slice, 2026-10-02. The
implementation handoff is
[`docs/handoff/merged-cells.md`](../handoff/merged-cells.md).

## Context

Merges affect authored state, rendering, hit testing, editing, formulas,
structural edits, history, persistence, snapshots, and XLSX. Implementing only a
Canvas span would make the displayed document disagree with PostgreSQL replay
and Undo. The implementation therefore landed through the canonical domain and
durable journal before exposing the toolbar action and OOXML profile.

## Decision

1. Store merges in canonical per-sheet authored state as disjoint rectangles of
   stable first/last row and column IDs. Resolve visible geometry from the sheet's
   current axis ordering; never persist A1 labels or current numeric positions.
2. Reject merges containing authored content or non-default styles outside the
   top-left anchor. The anchor remains authoritative and visible. This avoids
   hidden data loss and keeps the initial operation limited to metadata.
3. Reject content/style writes to covered non-anchor cells and structural edits
   that touch a merge. Insertions outside a range preserve its stable anchors.
   Any richer transformation must be a later explicitly versioned behavior.
4. Represent merge/unmerge as one server-authoritative guarded operation with
   exact prior membership for Undo/Redo. Include sorted merge state in snapshots
   and the canonical Domain hash using an additive snapshot schema.
5. Enforce 10,000 merges per sheet, 10,000 covered cells per merge, and 100,000
   covered cells per sheet. Validate overlap and rectangle contiguity against
   stable axes on command, event replay, projection decode, and snapshot restore.
6. Keep formula binding/references unchanged. A reference to a non-anchor cell
   remains a reference to that stable cell, whose content is guaranteed empty
   while covered. Merges do not change calculated authored state semantics.
7. UI and bounded profile-1 OOXML import/export were added after durable domain,
   journal and snapshot gates passed. Malformed/unsupported external ranges fail
   atomically.

## Consequences

The initial behavior is conservative: users must clear hidden cells before
merging, and cannot edit covered cells or change intersecting axes before
unmerging. It avoids destructive implicit clears, formula rebinding, legacy
event rewrites, or implicit expansion. It requires coordinated additive changes
to domain, protobuf codecs, journal validation, snapshots, browser projection,
and XLSX IR. The implementation was locally verified on Windows. Linux/Compose,
remote CI, production storage/deployment, and broad Excel golden compatibility
remain separate gates.

## Verification status

Implementation status: domain, codec/snapshot, PostgreSQL journal, projection,
Canvas hit-testing, Undo/Redo, and bounded XLSX import/export are complete for
the conservative profile. Workspace tests and Clippy pass; focused merge browser
coverage passes at DPR1/DPR2. See the handoff and RUNBOOK for exact test counts
and remaining deployment/compatibility gates.
