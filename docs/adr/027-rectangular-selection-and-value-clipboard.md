# ADR-027: Rectangular selection and plain-value clipboard

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET VALUE-RANGE SLICE.

## Context

ADR-025 commits TSV values atomically, but the UI previously had only one selected position. A server `CopyRange` command would have no usable source rectangle in the product. The browser also needs to preserve a blank cell at the trailing edge of a copied range; a final newline in plain TSV is ambiguous.

## Decision

The UI stores an anchor and active focus. Shift+arrows and pointer drag extend a rectangle; ordinary movement resets the anchor. Pointer moves are coalesced through one animation frame. Canvas highlights the rectangle without adding cell DOM elements, and a bounded accessibility output announces A1 bounds. Presence sends stable-ID anchor and focus. When axes change, both endpoints are remapped by stable row/column IDs; a deleted endpoint falls back to the nearest valid position.

Copy of up to 1,000 plain value cells creates TSV in row-major order. Formula-containing multi-cell selections, text with tabs/newlines and text beginning with `=` are rejected visibly because the current TSV parser cannot preserve their semantics. Single-cell formula copy remains ADR-026. The internal clipboard format carries exact TSV text, preserving a trailing blank row or cell; external plain TSV still trims one terminal newline. Paste at the selection's top-left uses ADR-025's one-version durable `SetCellValues` batch. Copied content is a client-captured value snapshot, not a server-side read-dependent `CopyRange` operation.

## Limits

This slice does not implement canonical `CopyRange` for mixed formulas/styles, cut/move, undo, multi-sheet selection, autofill or rich external clipboard. It does not claim source-side concurrency semantics beyond the values captured by the client at copy time. Empty cells remain sparse in storage. The existing 1,000-cell/512 KiB limits also bound range copy; larger selections show a typed UI error.

## Verification

Vitest covers rectangle bounds, trailing blank value copy, formula refusal and stable-ID remapping. Browser DPR 1/2 uses actual keyboard clipboard and pointer drag, verifies one batch version after paste/reload and checks that no gridcell DOM nodes were created. The underlying batch candidate benchmark remains [the measured schema-10 benchmark](../benchmarks/2026-09-29-value-batch-local.md); this slice adds no new domain algorithm.
