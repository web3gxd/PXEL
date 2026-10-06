# ADR-124: privacy masks and selection interactions

Date: 2026-10-06. Requested local UI corrections; no new dependency or authored
state format is introduced.

Enabling recognition selects all six detection types. Individual types remain
editable; disabling recognition preserves the selections for later adjustment.
Saving uses the existing privacy policy flow.

Revealed values stay clipped to the visible cell rectangle, including headers
and frozen panes. Privacy overlays commit synchronously within the canvas draw
animation frame so canvas and DOM use the same scroll position. Selected masks
use a matching green fill without hiding the cell borders. A multi-cell range
keeps its outer outline and gives the active cell a stronger fill; whole-axis
selections omit the additional active-cell/merged-range outline.

Double-clicking a sheet tab opens the existing rename dialog for that sheet,
with the same online/edit/confirmation guards as the menu action. Validation,
durable rename and Undo remain the existing sheet-settings behavior.
