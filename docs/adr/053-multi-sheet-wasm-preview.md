# ADR-053: Stateful WASM preview for bounded multi-sheet books

Date: 2026-09-29. Status: ACCEPTED FOR THE MULTI-SHEET PREVIEW SLICE.

## Context

ADR-052 disabled local preview whenever an imported book had more than one sheet. The domain, server calculation projection and browser's confirmed projection already held all sheets, so an edit on one sheet could not preview a dependent formula on another even though the shared engine supported cross-sheet references.

## Decision

Stateful preview init schema 2 carries an ordered 1–64-sheet authored projection. Schema 1 remains accepted for exactly one sheet. The adapter first validates and hydrates every sheet axis, then binds sparse formula cells against that complete axis state. Total initial cells remain capped at 100,000; each sheet keeps existing row/column bounds and the serialized init stays within 32 MiB. Patches retain schema 1 and stable sheet/row/column IDs, with at most 1,000 cells and 1 MiB per message. The result schema is unchanged. Unknown versions or duplicate/invalid IDs fail before a mutable preview is published.

The browser sends the full sparse projection only on init/reinit. It caches a row-ID-to-sheet-ID lookup for subsequent patches, so an edit on an inactive sheet is addressed to its owner. Version and pending revision still fence stale preview results. The server remains authoritative; preview is never a confirmed value. Axis changes still require projection refresh and worker reinit rather than a misleading cell-only patch.

## Verification and limits

Rust stateful tests cover cross-sheet incremental recalculation and compare the result hash with the native calculation core. The Chromium differential corpus includes the same two-sheet book with two patches and compares full native/WASM outputs. A web store test checks init sheet contents and patch sheet ownership. This is convergence compatibility for the tested fixture, not Excel golden parity or a measured large multi-sheet UI latency. Cross-sheet copy/move semantics and dynamic structural changes remain separate work.

A subsequent synthetic XLSX fixture exercises `Plan!A1+2` through local import, server calculation, browser edit on Plan, recalculation on Budget, export and reimport. The deliberately false XLSX formula cache is ignored. Export emits the canonical quoted form `=('Plan'!A1+2)`; the authored value and sheet target survive the round trip. This tests one bounded formula and package shape, not general Excel compatibility.
