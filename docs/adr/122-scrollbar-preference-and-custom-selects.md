# ADR-122: explicit scrollbar mode and custom selects

Status: ACCEPTED; local verification recorded in RUNBOOK.

## Decision

The browser preference `pxel.scrollbar.v1` stores either `smart` (the existing
default) or `ordinary`. Tools / Settings exposes the choice, independently of
shortcut assignments. Both modes use the same accessible custom tracks. Ordinary
mode always maps the full extent linearly; smart mode retains ADR-091's adaptive
used-range mapping, including its mathematically linear empty/full cases. Wheel,
touchpad and keyboard retain linear pixel semantics in both modes. Switching
mode does not move the viewport or commit a document version. Blocked storage
keeps the choice for the current session.

Used-range extraction reads the live confirmed cell map, not `sheet.cells`, which
is the last HTTP snapshot during incremental canonical writes. A projection is
cached by store, sheet identity and committed version. Pending operations and
calculated results do not enter this projection. Both axes share the cache;
ordinary mapping skips used-range extraction. Canonical content, style and axis
updates invalidate the cache without requiring a reload.

Both modes accept ADR-119's AxisLayout geometry overlay, so local resize
previews and pending axis sizes update track extent immediately. The used-range
content cache remains based on the committed sheet and version; resize redraws
do not clone the sheet or rescan authored cells.

Tracks are 10 CSS pixels across; thumbs are 8 pixels with a 3-pixel radius.
The 32-pixel minimum applies only along the scroll direction, fixing the former
CSS rule that accidentally made the vertical thumb 32 pixels wide.

All product select controls use a shared labelled combobox and listbox. Options
are declarative data and are rendered only while open; no native select/option
elements remain in the product DOM. A manual HTML popover keeps the listbox in
the top layer while retaining its dialog ancestry and clipboard disclosure
boundary. Placement follows its trigger, flips above when needed and stays
within the viewport. Disabled controls/options retain their guards. Arrow keys,
Home/End, Enter/Space, typeahead, Escape, Tab and outside interaction are handled;
navigation alone does not commit a choice. Focus stays on the trigger with
`aria-activedescendant`; Escape closes the popup before its containing dialog.

Formatting, view filters, sort keys, version-history filters, PXEL import/export
and disclosure scope reuse their previous actions and capability guards. Test
helpers select through the actual popup and identify the combobox separately
from its equally named listbox. No dependencies, wire messages, persistence
codecs or production server APIs are added.

## Operational cleanup

The requested one-time cleanup covers both explicitly selected local databases.
Private `pg_dump` archives are made and checked before changes. The encrypted
database is reconstructed through the authenticated journal/snapshot reader;
the legacy database uses a read-only plaintext journal replay, validating
checksums, canonical event hashes, identities and contiguous committed versions.
Unknown formats are retained. A book is empty only if every sheet has no authored
cells, nondefault styles or merges and there are no comment events.

The product currently has no delete operation and its journals prohibit erasure.
Validated empty books are therefore removed from the active library using the
existing `quarantined` state. An exclusive maintenance fence, head-version CAS,
comment recheck where supported, access-epoch increment and synchronous commit
guard the change. Journals, keys and blobs remain available for recovery. This is
administrative removal from the active library, not physical deletion or a new
user-facing trash/restore feature. No immutable-journal triggers or migration
metadata are changed. Temporary operator scripts remain in ignored private
`.local`; they do not become a production maintenance API.

Manual NVDA, other browser engines and external deployment/CI remain open gates.
