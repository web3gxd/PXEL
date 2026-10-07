# PXEL: ten functional increments

Approved scope: the user's request on 2026-10-07 to implement all ten audit
recommendations, sequentially, with real end-to-end verification. This document
tracks the whole objective; a completed sub-slice does not complete its step.

## Contracts and verification

Keep the modular monolith, PostgreSQL authority, stable identities, validated
candidate/CAS/durable commit before publication, independent wire/storage
versions and separation of authored/derived/optimistic state. No content or
credentials in logs, plaintext upload files, production persistence mocks or
unsafe Rust. Existing Excel-subset semantics remain compatible.

Source: `apps/server`, `apps/web/src`, domain/formula/storage/collab crates.
Tests: Rust unit/codec/PostgreSQL, Vitest, `apps/web/e2e`, integration and
recovery suites. Use synthetic documents in isolated PostgreSQL and separate
application ports; do not modify the existing demo books. Every applicable new
behavior gets a failing regression followed by a passing check.

Commands (PowerShell: first `. ./scripts/env.ps1`):

- `cargo test --locked -p pxel-server --features dev-auth`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo fmt --all --check`
- `pnpm test`, `pnpm lint`, `pnpm typecheck`, `pnpm build`
- `pnpm --filter @pxel/web exec playwright test --config playwright.config.ts`
- `just check`, `just sqlx-check`, `just audit`, `just formula-parity`
- Live integration/recovery commands from README, selected for each change.

Existing real authentication/ACL/disclosure and design-system patterns are the
code-style examples to follow. New DTOs deny unknown fields and fail atomically
on unknown versions; typed errors contain no user contents. Changes to existing
schemas and interfaces are documented in ADRs before implementation.

## Acceptance for the ten steps

1. **Browser XLSX/CSV import.** File selection, complete pre-publication
   compatibility report, sheet preview/selection and CSV type controls; atomic
   fresh-book creation, retry idempotency, formula/style preservation, malformed
   and oversized rejection. XLSX worker resource/filesystem/network controls
   and malware policy must be resolved before claiming public upload ready.
   First sub-slice: bounded CSV parsing, typed preview and atomic publication.
2. **Number formats and workbook locale.** Currency/grouping/precision,
   dates/time/durations and user masks; display never mutates numeric values;
   native/WASM/server/import/export/restart preserve the selected contract.
3. **Data validation.** Literal/range lists, numbers, dates and required fields;
   warn/reject behavior on server and UI, including paste/fill/structural edits;
   cell dropdowns/checkboxes persist and collaborate.
4. **Conditional formatting.** Ordered saved rules, comparisons/duplicates,
   formulas/color scales; derived styling remains separate from manual styles;
   recalculation, reference edits, restart and Undo preserve the rule contract.
5. **Array formulas.** FILTER/SORT/UNIQUE/SEQUENCE, ARRAYFORMULA and QUERY;
   deterministic bounded spill, collision errors, dependency invalidation,
   native/browser parity and independent versioned semantics.
6. **Named ranges and formula assistance.** Stable range identity, management,
   function/name completion and argument guidance; structural transformations,
   rename/delete errors, copy and recovery covered end to end.
7. **Protected sheets/ranges.** Server-authorized edit regions; all direct,
   paste/fill/sort/move/merge/structural paths enforce restrictions. A foreign
   editor cannot bypass UI restrictions through the protocol.
8. **Named versions and restoration.** Named checkpoints and restore as a new
   durable guarded operation, with concurrent-head checks, preserved journal,
   preview/diff, ACL and recovery coverage.
9. **Pivot tables.** Saved row/column/value/filter specification and derived
   SUM/COUNT/AVERAGE/MIN/MAX results; source edits refresh results, structural
   changes keep valid references, restart/collaboration preserve specification.
10. **Range-linked charts.** Saved bar/line/pie charts, series/axes/legend and
    position/size; confirmed values drive updates, privacy/disclosure controls
    apply, restart/collaboration preserve chart definitions.

## Completion evidence

Each step needs implementation, targeted E2E with real server/PostgreSQL,
meaningful lower-level tests, applicable lint/build/parity/recovery checks,
review, RUNBOOK results and a committed independently reviewable increment.
Linux/Compose/remote CI, production authentication/deployment and manual NVDA
remain explicitly open until actually verified. Do not mark ten-step completion
from an early browser prototype or a subset of green tests.

Status: all ten steps remain open. Step 1's CSV import/export sub-slices are
implemented, with browser/PostgreSQL acceptance and combined workspace
verification. XLSX selection/public-upload safety remains before
step 1 can be completed. See ADR-128, ADR-129 and RUNBOOK.

XLSX preparation: ADR-130 moves the existing worker's input from filesystem
paths to bounded package bytes. Local unit/child CLI checks passed; it does not
complete OS sandbox, malware policy or browser XLSX publication.

ADR-131's standalone WASI guest boundary has Windows tests, including native
fixture parity and positive capability controls. The operator CLI adapter is
integrated (ADR-132), active-content part rejection is extended (ADR-133), and
host resource caps are a verification candidate (ADR-134). Browser XLSX upload
is still not enabled; readiness/concurrency, sheet selection and atomic
publication remain open.
