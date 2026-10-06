# ADR-117: PXEL editor menus and shared control design

Status: ACCEPTED — implemented; local verification recorded in RUNBOOK.

The editor uses a compact title row, a seven-group disclosure navigation and one
scrollable formatting toolbar. Groups are File, Edit, View, Cells, Format, Data
and Tools. Existing actions keep their capability/pending/connection guards and
call the existing document operations. Merge/unmerge have local Phosphor icons in
the toolbar and duplicate entries under Format. Disabled placeholder top-level
menus are removed. No wire, persisted codec, DTO or durable operation changes.

Menu popups use native buttons in labelled groups, with Tab, arrow, Home/End and
Escape navigation. Escape restores the opener; opening with the mouse preserves
the editor's unfinished input until an action is chosen. Stateful library
navigation and its unsaved-input modal live outside the conditional popups.
Selecting a navigation action arms its existing guard before moving focus.
The previous menu opener remains available for modal focus restoration.

Cells replaces the former Insert group. Row/column insertion, deletion and size
actions move from Edit/Insert/Format into nested Add/Delete/Format disclosures.
Right opens a child and focuses its first enabled action; Left returns to its
parent, Escape closes the deepest expanded level before the root. The panel
scrolls within the viewport and nested content stays inside it for touch/mobile.
All labels and icons are local. Existing single-axis operations and guards are
reused; the UI introduces no new server operations.

The requested full menu also lists unsupported cell shifts, autofit/default
width, axis/sheet visibility, sheet renaming/move/copy/tab colour as explicitly
disabled items with an explanation. These are not implemented functionality and
must not be recorded as completed backend work. Creating a sheet is supported.

CSS semantic control tokens define hover, pressed and focus colours, radii and
popover shadows in the existing light green PXEL palette. Feature dialogs share
the surface, fields and button states. Icon-only actions keep accessible labels.
The formatting row hides its native scrollbar while retaining horizontal
trackpad/wheel, keyboard focus scrolling and programmatic scroll behaviour.

Checkboxes retain native input semantics with a shared 20px green appearance,
visible keyboard focus, checked/indeterminate and disabled states. Forced-colour
mode restores the native appearance. Dialog action rows wrap with a 10px gap;
privacy findings align the cell address beside its checkbox, with the exclusion
action on a second row at narrow widths. The bulk exclusion title is a heading.
The shared modal close action centres a local SVG icon. Repeated privacy analysis
preserves selected fingerprints still present in the refreshed report; saving
changed policy continues to clear the selection explicitly.

Shared modals now have a semantic title icon and divided header, a body container,
stacked top-level fields, styled error messages and separated footer actions.
Style and shortcut dialogs share the same header/close treatment. Existing
Markdown import keeps its already branded heading, preview, focus and controls.
Native dialog lifecycle and clipboard disclosure boundaries remain intact.

The connection indicator is a square button with an accessible live status. Its
popover opens on hover, focus or activation and closes on leave, blur or Escape.
It shows connection status, confirmed version, pending operation count and
calculation freshness. WebSocket response time is measured from Hello send to
Welcome processing by the existing client; it is explicitly labelled as the
last connection measurement, not a continuously sampled ping. No extra server
requests or protocol messages are introduced. Retry remains available for
terminal recoverable connection states. No content or credentials are logged.

Browser helpers now navigate the grouped menus instead of assuming every action
is permanently visible. Local checks and screenshots cover 390/681/1440 widths
and DPR1/DPR2. A real NVDA walkthrough and external deployment gates remain open.
