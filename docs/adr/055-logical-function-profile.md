# ADR-055: Bounded AND, OR and NOT

Date: 2026-09-29. Status: ACCEPTED FOR THIS FORMULA SLICE.

## Context

The shared evaluator supports IF/IFS/IFERROR but treats AND/OR/NOT as unknown functions. The first logical group is incomplete, and ordinary conditional XLSX formulas cannot calculate. Adding recognized functions changes calculated semantics even though the AST and authored-state codecs remain valid.

## Decision

AND and OR take 1–255 arguments; NOT takes exactly one scalar argument. AND/OR visit direct arguments from left to right and reference ranges in fixed row-major order. Referenced text and blanks are ignored; booleans and finite numbers participate (zero is false). No participating values yields VALUE. Direct arguments use the existing logical coercion: blank is false; TRUE/FALSE text is accepted; other text yields VALUE. NOT inverts this scalar coercion, including blank references. Array values and implicit intersection remain outside the profile.

A decisive boolean does not short-circuit AND/OR: later spreadsheet errors still determine the result. The first spreadsheet error in traversal order is retained. Every range visit is charged and its visitor count is checked. Resolver, cancellation and work-limit failures remain typed engine errors and cannot be hidden by IFERROR or a recorded spreadsheet error. This deterministic error policy is an explicit subset contract until an Excel golden corpus verifies the edge cases.

The function specifications derive from Microsoft [AND](https://support.microsoft.com/en-us/excel/functions/and-function), [OR](https://support.microsoft.com/en-us/excel/functions/or-function) and [NOT](https://support.microsoft.com/en-us/excel/functions/not-function). Derived fixtures are marked `verified_in_excel: false`; documentation alone is not Excel execution evidence.

Bump calculation engine identity from `pxel-formula-0.1.0` to `pxel-formula-0.2.0`; AST schema, compatibility profile, event and snapshot bytes are unchanged. Native and WASM must advertise the new identity, and browser guards reject old-engine calculation messages. Historical persisted contexts retain their original engine identity and replay/hash. They are not silently rewritten: the existing authorized `POST /api/documents/{id}/recalculate` appends a new context epoch for the current engine. Until that advance, an old-engine context produces a controlled invalid-projection calculation failure. Context-free books calculate with the new engine without a content migration.

## Verification and consequences

Use the common fixture corpus for native/browser outputs and hashes, bounded scalar and resolver tests, live PostgreSQL command/recalculation tests, and real 100k-range Criterion benchmarks. No formula-source or cell-value logging is added. No runtime dependency is added. [ADR-056](056-explicit-recalculation-ui.md) subsequently adds the UI action for explicit epoch advancement; automated upgrade orchestration remains separate work. The phase-2 broader math/text/date/lookup profile and manual Excel verification remain open.
