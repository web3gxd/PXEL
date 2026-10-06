# ADR-092: structural style preimages

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-02).

## Context

AxisEdit already froze content preimages so deletion, undo and redo could be
derived and guarded independently from the client. Sparse cell formatting is
part of the canonical workbook, including on blank cells, so axis deletion must
freeze its style changes under the same server-authoritative operation.

## Decision

- `AxisChange.style_writes` stores sorted stable addresses and full optional
  `CellStyle` before/after images. `None` means no sparse style entry; default
  styles are invalid in the sparse map. Row/column deletion removes styles on
  the deleted axis; inverse restores the exact prior styles after stable IDs
  have been revived.
- Structural candidates derive style preimages from the current workbook and
  compare them with the submitted operation before applying it. Undo verifies
  the exact style side currently committed. Style writes are bounded to 10,000
  addresses and the existing workbook style capacity.
- Storage schema 26 (AxisEdit with style preimages) and 27 (AxisCompensation)
  are additive. Style-free edits stay schema 21/22 with unchanged semantic hash
  tags 23/24. New style-bearing events use semantic hash tags 27/28 and encode
  full profile-1 style preimages in the existing storage protobuf.
- PostgreSQL accepts schema versions 26/27 through a new constraint migration.
  Existing event bytes, readers and hashes are unchanged.
- Axis history checks later style writes at affected stable IDs by address and
  property. Exact same-author compensation pairs neutralize; any remaining style
  intent blocks axis undo/redo. Independent styles elsewhere do not block it.
- Legacy direct structural event schemas retain their historical validation.
  New server commands use AxisEdit/AxisCompensation as before.

## Verification

Domain tests cover row and column style deletion plus exact undo/redo restoration,
and history barriers for a later style edit and its own compensation. Codec tests
cover schemas 26/27, style preimage round-trip, style-sensitive semantic hashes,
and unchanged schema 21/22 golden hashes. Browser coverage exercises durable row
deletion, undo, reload and redo at DPR 1 and 2.

This ADR does not itself add formatting to CopyRange/MoveRange. Styled CopyRange
was added later through additive schemas 30/31 without changing existing
content-only event meaning ([ADR-094](094-styled-range-copy.md)). Styled MoveRange
remains open.
