# ADR-094: Styled range copy as one durable cell batch

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-02).

## Context

Formatting is canonical sparse workbook state, but `CopyRange` historically
froze only values and formulas. Committing content first and styles second would
expose a partially copied range and require two Undo actions. Existing content
only `CopyRange` events must keep their historical schema and hash semantics.

## Decision

- Reuse `CellBatchWrite` for style-bearing range copies. Its explicit intent
  includes source, destination, and rectangle dimensions; content and styles
  share one candidate, event, version, and inverse identity.
- The server resolves copied values and translates bound formulas from the
  confirmed source range. It also freezes all sixteen style leaves per
  destination cell, using resets for properties absent at the source. This
  exactly replaces existing destination formatting, including formatting on
  blank source cells.
- Journal validation checks the command geometry, source/destination axes,
  style preimages, content preimages, and each style leaf's guarded history.
  Undo restores content and styles together; a same-operation retry remains
  idempotent.
- Existing scalar/style batch schemas 28/29 are unchanged. Styled copy uses
  additive storage schemas 30/31 and `202610020003_styled_copy_batches.sql`.
  Copy intent and geometry are included in the semantic hash; the existing
  wire `CopyRange` command is unchanged.
- The operation remains bounded by the existing 1,000-cell, text-byte, style,
  and durable event limits. Clipboard formats and the server-authoritative
  PostgreSQL/CAS commit model do not change.

## Consequences

Formula-bearing range copy preserves exact sparse formatting while translating
formulas as before. Its one durable Undo restores both the prior destination
contents and all prior style leaves. Copy formatting is implemented; Move
formatting still needs its own additive operation schemas and guarded preimages.

## Verification

Tests cover normalization with replacement of destination-only formatting,
schema/hash round trips for authored and inverse events, PostgreSQL commit,
idempotent retry and Undo, and real browser formula copy at Chromium DPR1/DPR2.
Exact test commands and results are recorded in RUNBOOK.md.
