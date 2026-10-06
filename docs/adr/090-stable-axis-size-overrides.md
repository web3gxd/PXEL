# ADR-090: Stable axis size overrides

**Status:** Accepted; locally verified (2026-10-01)
**Date:** 2026-10-01

## Context

Rows and columns have stable identities and may be inserted, deleted, revived or
reordered. Keeping display sizes only in Canvas state would lose them on reload
and attach them to the wrong logical row after structural edits. Putting sizes
inside cell styles would conflate axis geometry with cell formatting and formula
semantics.

## Decision

- Store row heights and column widths as sparse overrides keyed by stable axis
  ID on the sheet. Defaults are 28 CSS px for rows and 112 CSS px for columns;
  default-valued entries are omitted.
- Accept integer CSS pixel sizes in the bounded profile: rows 12–1000 px,
  columns 24–2000 px. Reset removes an override.
- Change sizes through the existing server-authoritative candidate/CAS/commit
  path. Each write carries an exact previous value and a target identity; stale
  preimages reject atomically. Geometry changes do not dirty formula results.
- Include sparse overrides in canonical state hashing and snapshot schema 5;
  use additive event schema 25 and retain older event/snapshot readers.
- Build axis metrics from the viewport model and sparse overrides. Canvas draw,
  hit-testing, scroll-to-cell, selection geometry and editor placement consume
  the same prefix metrics. Resize is available from row/column headers and an
  accessible size panel. Header clicks select the complete column.

## Consequences

Size state follows the logical axis across structural changes and survives
reload. Metrics use memory proportional to axis extent, not DOM elements. The
prefix index is rebuilt when an axis or its overrides change; visible rendering
still scales with viewport plus overscan. Browser and Linux/remote deployment
gates remain separate.
