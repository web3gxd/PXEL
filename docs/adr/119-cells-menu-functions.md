# ADR-119: durable Cells menu operations

Status: ACCEPTED; local verification recorded in RUNBOOK, 2026-10-06.

The fourteen unavailable Cells actions become working operations in this slice.
Existing permissions, online and pending guards remain. Dialogs use the shared
Modal and never alter confirmed state before the durable server acknowledgement.

1. Sheet settings carry name, visible tab ordering, hidden state, default column
   width, optional RGB tab colour and stable-ID hidden rows/columns. One guarded
   operation stores the exact before/after image and ordering. At least one sheet
   remains visible. Sheet copy creates new stable IDs and translates local bound
   references atomically; external sheet references remain external.
2. Row/column autofit measures displayed content using local Canvas fonts, handles
   explicit line breaks and wrapping, and reads live confirmed cells after WebSocket
   writes. A bounded batch uses existing axis-size commands. Default width changes columns without explicit overrides.
3. Cell insertion/deletion shifts the selected stripe right/down or left/up using
   frozen guarded content/style batches and existing move-reference semantics.
   Merge intersections, overwritten occupied trailing cells, unsupported formula
   transformations and existing operation budgets refuse the complete command.
4. Settings, shifts and copies use distinct versioned wire intents. Persisted
   events and snapshots have independent additive versions; legacy images with
   default settings retain their canonical hashes. Metadata changes use full
   authorized projection resync; no optimistic sheet mutation is introduced.

Hidden memberships retain retired stable IDs so axis Undo revives the same
visibility; public redacted projections expose only live hidden memberships.
Showing more than 1000 hidden axes uses repeatable bounded commands and labels
the batch in the menu. Each batch still commits atomically; hiding retains the
existing 1000-axis limit.
Inherited sheet names retain the old seed validation contract. New rename and
copy commands enforce the narrower interactive name profile.

The complete allocated suffix, including blank cells and inbound formula updates,
counts against the existing 1000-write limit. Deletion refuses surviving formula
references into the erased selection. Settings and sheet copies are history
barriers; shifts support guarded Undo/Redo. Explicit line breaks remain visible
after autofit even when automatic wrapping is off.

Authored projection8 and WASM init6 carry full layout; redacted projection2 carries
only effective layout. Snapshot8 and event39–42 have independent readers. PXEL
file schema2 retains layout and reads schema1. The bounded XLSX profile refuses
settings it cannot represent, avoiding silent data loss.

Verification sequence: settings/copy domain-codec-journal tests; shift planner
and history tests; frontend command/decoder/autofit/geometry tests; full tests,
lint, typecheck, protocol and WASM generation; isolated PostgreSQL and Chromium
DPR1/DPR2 with reload and peer convergence. Actual results belong in RUNBOOK.
Linux/Compose/remote CI, production deployment and NVDA remain external gates.
