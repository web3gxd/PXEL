# ADR-089: Browser interoperability and render invalidation

**Status:** Accepted for the implemented browser slice; follow-up contracts remain open.

**Date:** 2026-09-30

## Context

Spreadsheet clipboard formats use quoted fields and can contain newlines inside a
single value. Plain Markdown can also contain pipes and line breaks that are not
table delimiters. Canvas rendering must preserve the existing Canvas viewport
model, while authored values, styles, calculations and undo remain under the
existing canonical server operation model.

## Decisions

- Parse TSV/CSV with a state machine; serialize rectangular selections to both
  `text/plain` and `text/html`. When an external HTML table is present, read that
  table first and use plain text only as fallback. Internal PXEL clipboard types
  keep their existing precedence.
- Parse GFM with the pinned mdast parser in a Web Worker. Resolve the parser's
  worker export condition so entity decoding does not depend on a browser DOM.
  Markdown without a table stays one value; detected tables require the explicit
  import preview. HTML is parsed in a detached template and active elements are
  ignored.
- Keep shortcut definitions in a typed registry, with versioned browser-local
  overrides, context matching and conflict checks. Shortcut preferences do not
  enter the authoritative workbook projection.
- Keep the canonical style model and operations unchanged. Toolbar edits use the
  existing style operations. The Canvas renderer may repaint changed visible
  cells when viewport geometry and overlays are unchanged; scrolling, selection,
  presence and geometry changes invalidate the full viewport.
- Keep the previous confirmed formula result visible while a new authoritative
  calculation is pending. Reject calculation patches whose document, input
  version, context, epoch, sequence or formula set does not match the current
  confirmed head.
- Invalidate both the old and new Canvas overflow spans when a blocker is typed
  or committed, including an authored formula with an empty result. A visible
  optimistic formula-bar/cell draft must be recognized before it enters the
  confirmed value projection.

## Follow-up contracts

GFM alignment markers map to supported cell alignment styles through the
server-authoritative composite operation in ADR-093, so imported values and
alignments share one commit and one inverse.

ADR-100 carries the evaluator's changed and cleared results through a
base-version-checked WebSocket delta. A missing or stale client base still gets
a full confirmed snapshot; full-state hash calculation and fallback remain
linear in the result set.

The original profile had a single `wrap` flag. ADR-087 extends it compatibly
with a versioned clip/overflow/wrap mode, Canvas spill into blank cells and
profile-aware persistence. The earlier browser slice did not claim this support.

## Consequences

The Markdown composite operation and text-overflow profile are implemented in
ADR-093 and ADR-087. Calculation patches remain subordinate to the confirmed
input version and never change authored content or operation history.
