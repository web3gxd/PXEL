# ADR-052: Client-selected sheet without a new document operation

Date: 2026-09-29. Status: ACCEPTED FOR THE MULTI-SHEET VIEW SLICE.

## Context

Imported books can now contain multiple durable sheets. The server projection returns every sheet, but the browser rejected more than one and displayed a hard-coded first-sheet tab. Sheet selection is a local viewport choice; making it a durable operation would add versions to the document without changing authored data.

## Decision

The browser validates an ordered 1–64-sheet projection and keeps `activeSheetId` separate from the confirmed document. Footer buttons select by stable ID. The confirmed cell map and server calculation assembly cover all sheets; their row/column IDs are globally unique. Canonical cell patches address the sheet in the server event, regardless of which tab is visible. Changing the active sheet resets local selection, scroll and presence view, and commits an in-progress edit before switching. An in-session projection refresh preserves the selected ID if it still exists; a full page reload starts on the first sheet.

The stateful WASM preview protocol currently accepts one sheet. For a multi-sheet book, the browser disposes that preview worker and displays only server-authoritative calculation results. This is a visible performance limitation, not a substitute evaluator. Native/WASM multi-sheet preview parity requires a separate change to the worker's versioned init/patch schema and fixtures.

## Consequences

Tab selection creates no durable event. Other participants see the selected sheet through ephemeral presence only. Add/delete/rename/reorder sheet controls still need commands and conflict semantics. The client must reject cross-sheet formula clipboard payloads until copy translation is specified; the scalar range copy already carries a sheet ID. A browser test opens a real imported two-sheet document, changes tabs and reads a cell on the second sheet.
