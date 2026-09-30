# ADR-088: bounded XLSX cell-style round trip

Status: PROPOSED FOR IMPLEMENTATION (2026-09-30).

## Context

ADR-087 adds sparse profile-1 cell formatting, including styled empty cells.
The current OOXML reader reports cell styles as unsupported and the exporter
rejects any formatted workbook with HTTP 422. Removing that guard before the
package reader, import journal, and writer preserve the same leaves would make
download silently discard user data.

The first slice is deliberately limited to the existing 16 closed style
properties. It does not add row/column styles, arbitrary format strings,
conditional formatting, table styles, themes, gradients, or workbook-wide
inheritance. OOXML is parsed into a versioned normalized IR; neither OOXML
indices nor Domain types cross the worker boundary.

## Decision

1. Add `schema_version: 2` to `NormalizedWorkbook` and an independently sparse,
   sorted `styles` list to each sheet. Each record contains a one-based
   coordinate and a fully resolved, typed `NormalizedStyle`; content and style
   lists may overlap. A style-only coordinate contributes to extent and union
   quotas. The parent validates every field and quota again after child decode.
2. Resolve the styles part only through the workbook relationship graph. Validate
   `fonts`, `fills`, `borders`, `cellStyleXfs`, `cellXfs`, and custom number
   formats before resolving worksheet `c/@s`. Reject invalid references,
   duplicate custom number-format IDs, excessive tables, and malformed style
   values as package errors. Never turn a bad index into the default style.
3. The supported subset maps explicit RGB colors, solid fills, the six Domain
   border line values, font family/size/bold/italic/underline/strike, horizontal
   and vertical alignment, wrap, and the seven existing number formats. Font
   sizes are parsed as decimal hundredth-points without a binary-float round
   trip. General/Integer/Decimal2/Percent0/Percent2/Scientific2 map only from
   explicitly enumerated built-in IDs or exact supported custom codes;
   DateIso maps only from `yyyy-mm-dd`. No heuristic format detection is used.
4. Theme/indexed/automatic/tinted colors, non-solid fills, unrecognized border
   variants, unsupported custom number formats, nonzero style inheritance, and
   other unsupported components produce typed compatibility issues. Under
   `LossPolicy::Reject`, any issue rejects the import before creating events.
   Under `AcceptReported`, affected unsupported style components are omitted
   only when the report explicitly identifies the issue; valid independent
   components may still be imported. A damaged XML/index is always rejected.
5. Missing style parts and the default cell style normalize to the PXEL profile
   defaults. This profile does not claim identical rendering of workbook theme
   defaults. Preserve the distinction between an absent style and a supported
   non-default style; a normalized default style does not create a Domain entry.
6. Convert normalized style records to one bounded `StyleWrite` event per sheet
   (up to 1,000 union addresses per batch), with explicit stable row/column IDs,
   all changed leaves, and reset semantics. The import transaction admits only
   the existing creation operations plus validated style writes, after checking
   total events, payload bytes, union addresses and Formula binding limits.
   Import remains atomic and server-authoritative.
7. Export enumerates the union of content and style addresses. It deterministically
   interns identical font/fill/border/number-format records, writes the styles
   relationship and content type, and applies an `s` index to styled blank cells
   as well as populated cells. Styles never change stored numbers or formulas.
   Existing issue-bearing workbooks and any unsupported style value remain
   rejected. Remove the HTTP 422 style gate only after this complete round-trip
   path passes native package, journal recovery, and browser download tests.
8. Keep archive, XML depth, entries, elapsed time, parent revalidation and child
   resource bounds. Style tables are capped at 4,096 records each; each document
   keeps the Domain limits of 100,000 union content/style addresses per sheet,
   100,000 styles per workbook and the existing byte budgets. Any expanded IR
   budget must be justified by measured fixtures and remain below the framed
   child response cap.

## Compatibility and migration

The XLSX worker protocol is bumped from v3 to v4 together with IR schema2. The
parent and child reject unknown versions atomically. This ephemeral local
protocol is independent from persisted event codecs and public WebSocket
protocols. No existing journal entries or snapshots change. Scalar normalized
IR v1 is accepted only by the explicit compatibility decoder during migration;
new child output is always v2. Export emits deterministic OOXML style indices,
but those ZIP/XML bytes are not a canonical Domain hash.

## Validation

The gate requires parser fixtures for all 16 properties on values, formulas and
styled blanks; shared and repeated style deduplication; multiple sheets and
style-only extent; clear-value preservation; exact numeric/formula and style
round trip through import journal, recovery/snapshot, export and reimport; and
malformed/oversized package, table, parent IR, relationship and style-index
rejections. Browser tests must prove a styled blank exports with a style index
and downloads successfully. Unsupported themes, tints, patterns, custom formats,
and inheritance need typed report assertions. XLSX fuzzing stays in its existing
isolated parser suite.

This ADR does not claim full Excel formatting fidelity or close the XLSX phase.
