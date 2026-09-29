# ADR-025: Atomic plain-value paste

Date: 2026-09-29. Status: ACCEPTED FOR THE SINGLE-SHEET VALUE-PASTE SLICE.

## Context

Sending one command per pasted cell creates partial ranges and many document versions. Formula copy requires separate relative-reference binding rules; silently treating pasted formula text as values would corrupt intent.

## Decision

`SetCellValues` persists 1–1,000 distinct stable addresses in one schema-10 event and one CAS version. The entire batch is validated and applied to a private candidate before commit. An invalid coordinate, duplicate address, oversized value or total text over 512 KiB rejects the entire batch. The wire command is also capped at 1 MiB. The server trusts neither a client AST nor a client-calculated result. A retry with the same operation ID returns the durable receipt; replay uses the original schema-10 event.

The Canvas clipboard adapter accepts plain TSV. It overlays all pending writes immediately, then refreshes from the confirmed projection after the canonical batch arrives. Pasted fields beginning with `=` are rejected with a visible message until formula copy/binding semantics are implemented. The current adapter copies one selected cell as plain text. Rich clipboard, multi-cell selection, formula copy, cut/move and undo are separate slices.

## Consequences

This command covers value paste without a partial-commit window. It does not claim Excel-compatible formula paste. One large event remains under bounded wire, decoded and calculation budgets. The client uses a projection refresh after commit rather than applying a batch locally; this is correct but may cost an HTTP round trip. The 1,000-write candidate benchmark measures domain work only.

## Verification

Domain atomicity/replay and codec schema/fingerprint tests, PostgreSQL/WebSocket commit/retry/rejection/formula invalidation, Vitest optimistic overlay and browser DPR 1/2 paste/reload cover this slice. Actual commands and measured result are in RUNBOOK and the benchmark report.
