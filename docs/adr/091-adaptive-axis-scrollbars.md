# ADR-091: adaptive axis scrollbars

**Status:** Accepted; locally verified (2026-10-02)

2026-10-06: ADR-122 adds an explicit smart/ordinary preference, live confirmed
used-range cache invalidation and narrower thumbs. The adaptive formula and
linear wheel/keyboard semantics below remain unchanged.

## Context

The viewport is backed by stable axis IDs and variable-size prefix metrics. The
native scrollbar maps track position linearly across the full allocated extent,
so a mostly empty 100,000-row sheet leaves little control resolution among the
populated rows. Scrolling with wheel, touchpad, navigation keys and `scrollTo`
must keep their existing linear pixel semantics.

## Decision

- Keep the browser's scroll container and axis metrics as the source of sheet
  offsets. Replace only its visual scrollbar with accessible vertical and
  horizontal tracks. The tracks begin after the fixed row/column headers; row
  and column prefix metrics retain variable sizes.
- Compute an effective used end independently for each axis from nonblank
  values (including formulas) and nondefault styles. Use the nearest-rank 95th
  percentile of unique occupied positions. Values beyond that robust core remain
  reachable through the empty-tail segment.
- For maximum scroll offset `M`, effective used scroll offset `U`, and normalized
  track position `p`, use ordinary linear mapping when `M=0`, `U=0`, or
  `U/M >= 0.9`. Otherwise choose a used-segment share `S` from
  `B = 0.65 + 0.30 * (1 - sqrt(U/M))`. Above ratio `0.6`, smoothly fade `B` to
  the ratio itself by ratio `0.9`; this makes the mapping converge to the
  identity as the used range approaches the extent.
- Map the first segment by `offset = p * U / S` for `p <= S`. Map the tail by
  `offset = U + (p-S) * (M-U) / (1-S)` otherwise. The inverse uses the same
  breakpoint and segment slopes. Both segments meet at `(S,U)`, so the mapping
  is continuous, strictly increasing for a movable axis, and exactly reversible.
- Apply this nonlinear mapping only to track clicks and pointer drags. Wheel and
  touchpad continue to scroll the DOM container directly. Keyboard on the
  scrollbar moves by row/column size or viewport size; Home/End go to the sheet
  edges. Thumb size remains proportional to the viewport with a 32 CSS-pixel
  minimum. During drag, expose the current row or column in a tooltip and ARIA
  value text.
- Cache used-range extraction by immutable sheet projection. Do not create DOM
  cells or derive scroll range from materialized elements.

## Verification and limits

Unit tests cover monotonicity, inverse round trips, the used/empty boundary,
outlier tails, empty/full ranges and extreme offsets. Browser tests exercise both
axes, drag tooltips, minimum thumb size, linear wheel/PageDown, adaptive empty-tail
access and model sizes of 20, 1,000 and 100,000 used rows at DPR 1 and 2.

The model supports fixed headers and variable row/column sizes. The current
workbook has no frozen data panes beyond its always-visible row/column headers.
This decision does not add freeze-pane behavior or claim performance validation
for 100,000 populated cells.
