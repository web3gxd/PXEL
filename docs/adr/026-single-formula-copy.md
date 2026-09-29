# ADR-026: Single-cell formula copy through shared Rust syntax

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-CELL FORMULA-COPY SLICE.

## Context

ADR-025 deliberately rejected formula text in TSV paste because a copied formula's relative references must move while absolute axes remain fixed. The authored source can also display an obsolete A1 coordinate after a structural edit. The current displayed formula text is the correct copy input for the current axis projection.

## Decision

The pure `spreadsheet-formula::copy_formula_text` parses current formula text, shifts each relative row and column independently in zero-based Excel address space, keeps absolute axes unchanged, and returns canonical text. A reference shifted beyond the address space becomes `#REF!`. A thin WASM export runs this exact routine in a dedicated browser Worker; no workbook is serialized for the copy. The browser's internal clipboard format records the source document/version, current formula text and origin coordinate. Paste refuses an older document version and checks its destination axes again after the asynchronous Worker response.

The transformed text is sent through the existing `SetCellFormula` command. The server reparses and binds it to stable IDs and applies its normal authorization, candidate, CAS, replay and idempotency rules. The client-produced text is an intent, not a trusted AST or canonical calculation result. No persisted schema changes are needed. The client permits only one copy Worker request at a time and the Worker call has a five-second timeout.

## Scope and limits

This slice copies one formula cell within the same document. Plain TSV value paste remains atomic via schema 10. Multi-cell formula paste, selection rectangles, rich cross-document clipboard, cut/move and undo are not covered. A version change between copy and paste asks the user to copy again instead of silently interpreting old A1 coordinates. A fresh Worker is created per formula paste; its local latency is measured separately. The ordinary native/Worker copy routine shares source code, while Excel parity beyond the listed fixtures is not claimed.

## Verification

Native tests cover mixed `$` modes, sheet names, ranges, boundaries and invalid targets. Vitest covers current-text selection and stale clipboard rejection. Browser DPR 1/2 covers real Ctrl+C/Ctrl+V clipboard use, Worker transformation, server-bound commit, formula result and reload. The benchmark records Worker startup plus cached WASM initialization and JS reply on one local machine.
