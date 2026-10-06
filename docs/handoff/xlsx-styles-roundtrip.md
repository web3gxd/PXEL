# Handoff: XLSX cell-style round trip

Status: **bounded profile-1 round trip locally verified (2026-10-01)**  
Prepared: 2026-09-30; work resumed 2026-10-01  
Related decisions: [ADR-087](../adr/087-cell-formatting.md), [ADR-088](../adr/088-xlsx-cell-style-profile.md), [ADR-098](../adr/098-xlsx-shared-formulas.md)

## Goal

Add a bounded XLSX import/export path for the existing PXEL profile-1 cell styles. A workbook that contains supported styles must survive import, journal replay/recovery, export, and reimport without losing cell values, formulas, or style leaves. The style-specific HTTP 422 was removed after the gates below passed locally. The current import entry point remains the development-only CLI; browser upload is not included.

This handoff records the completed bounded scope and its remaining limits. Do not imply general Excel formatting compatibility.

## Current verified boundary

- Sparse collaborative formatting is implemented for 16 typed style properties, including blank cells, durable history, and native/WASM preview.
- The scalar XLSX reader/writer supports the previously recorded scalar profile.
- Styled XLSX export returns the bounded profile through the authorized route. A real browser download check verifies the file is delivered at DPR 1 and 2 without changing the canonical projection.
- Style-aware XLSX import/export and reimport are locally verified against PostgreSQL, browser download and a hard-restart recovery flow.
- No persistent journal codec migration is needed for the normalized XLSX IR. It is an import/export boundary format.
- The schema-3 IR has sorted sparse style records independent from content,
  including styled blank coordinates. Parent validation rechecks schema,
  ordering, bounds, canonical defaults and content/style union budgets.
- Excel shared-formula masters are represented as formulas and followers as
  validated direct-master references. Import expands followers with the existing
  formula-copy engine before server binding; export writes ordinary formulas.
  See [ADR-098](../adr/098-xlsx-shared-formulas.md).
- Worker framing is version 4. OOXML styles resolve only through the workbook
  relationship. The bounded parser maps the 16 profile leaves, reports valid
  unsupported components, and rejects malformed references/counts/indices.
- The importer converts validated style records to existing durable `StyleWrite`
  events with exact preimages; style-only import is covered by a replay test.
- The OOXML writer interns styles deterministically and emits `s` for both
  populated and blank styled cells. Native tests round-trip all leaves, formulas,
  values, blank styles, and shared records across sheets.
- The HTTP export projection includes sparse supported styles, including blank
  cells. The style-specific 422 guard has been removed; unsupported export
  profiles still fail with a typed error.
- A Microsoft Excel 16 generated profile fixture now exercises two sheets,
  Unicode, quoted/multiline cell values, formulas, all 16 style leaves and a
  merge through normalization and PXEL export/re-import. This uncovered and
  fixed SpreadsheetML `_xHHHH_` text escapes; it extends this bounded profile
  but does not claim general OOXML/Excel compatibility.

## Non-goals

Do not add row or column styles, conditional formatting, table styles, themes, gradients, arbitrary format strings, workbook-level style inheritance, or full Excel rendering fidelity. Do not introduce an XLSX spreadsheet framework. Keep using the isolated XLSX process and the existing ZIP/XML libraries. Do not reintroduce the styled-export guard without a demonstrated compatibility regression.

## Contracts to preserve

1. Parse OOXML into a versioned normalized IR; do not assign stable domain IDs in the XLSX crate and do not expose OOXML style indices outside the parser.
2. Resolve `styles.xml` only through workbook relationships. Reject malformed relationships, invalid indices, invalid declared counts, duplicate custom format IDs, and malformed values. Never silently map invalid data to default style.
3. Parent-side validation must repeat all child IR schema, field, ordering, address, union-size, and byte-budget checks.
4. Preserve content and style as independent sparse properties. A style-only blank cell is included in sheet extent and in the content/style union limit.
5. Unsupported-but-well-formed style components produce a typed compatibility report. Under `Reject`, reject before creating events. Under `AcceptReported`, omit only the affected unsupported component and retain valid supported components. Malformed input always rejects.
6. Convert normalized styles to server-created `StyleWrite` operations over server-assigned stable row/column IDs. The current sparse planner emits one-cell operations; import creation and all writes remain one atomic durable import transaction. Bound cells, batches, events, encoded bytes, formula binding work, and calculation budget before commit.
7. Export the union of content and style coordinates. Use deterministic style-table interning and deterministic ordering. Write an `s` index for styled blank cells. Do not alter numeric values or formula source.
8. Unsupported report-bearing workbooks and unrepresentable style values remain rejected by export; never silently drop them.
9. Do not log workbook contents, formulas, imported names, or cell values.

## Normalized IR and worker protocol

- Bump the local XLSX worker framing protocol from v3 to v4 when the style-aware IR is introduced.
- Emit normalized IR schema 3 with `schema_version`, sorted sparse content cells, sorted sparse style records, shared-formula master references, and bounded typed compatibility issues.
- Keep wire protocol and persisted event codecs independent. This IR is neither a durable operation format nor a canonical state hash.
- Accept older schemas only in an explicit, tested compatibility decoder if already-deployed local clients or artifacts require it. New worker output is always schema 3. Unknown schemas fail atomically.
- Keep the worker response under its existing frame cap. Any budget change needs measured fixture sizes and must remain below the cap.
- Revalidate the decoded IR in the parent before preview counts, import planning, or export.

## Supported style profile

Map only the existing 16 PXEL leaves:

- Font family and hundredth-point size; bold, italic, underline, strike, RGB font color.
- Solid RGB fill.
- Top, right, bottom, and left borders using the six existing line kinds and explicit RGB colors.
- Horizontal/vertical alignment and wrap.
- The seven existing number formats: General, Integer, Decimal2, Percent0, Percent2, Scientific2, and DateIso.

Use exact enumerated built-in number-format IDs and exact approved custom format strings; DateIso is only `yyyy-mm-dd`. Parse font sizes as decimal hundredth-points without an intermediate floating-point conversion. Preserve the distinction between absent/default style and a non-default style; a fully default style must not create a sparse Domain entry.

Report unsupported theme/indexed/automatic/tinted colors, non-solid fills, unrecognized border variants, unsupported custom formats, and nonzero inheritance. The report must identify that style compatibility was reduced without including imported document content. A damaged table or reference is a package error, not a lossy compatibility issue.

## Implementation sequence

### A. Freeze contracts and fixture matrix

- Review ADR-088 and this handoff against the current normalized reader, process worker, import planner, journal limits, and projection exporter.
- Add a fixture matrix for all 16 leaves across populated cells, formula cells, and styled blanks; include repeated/shared style records and multiple sheets.
- Add malicious/malformed cases for relationship targets, style index bounds, declared table counts, custom format duplicates, XML depth, and parent IR tampering.

Progress: schema-3 style records, parent validation, worker v4, relationship-
bound style parser, deterministic native writer, malformed OOXML tests and the
16-leaf native fixture matrix are implemented and verified.

### B. Normalized model and parser

- Add versioned sparse style records with closed enums and parent-side validation.
- Resolve styles through package relationships; cap font/fill/border/format/xf tables at 4,096 entries each.
- Parse supported values without heuristic mapping. Ensure unsupported values create typed issues and invalid indices fail.
- Include style-only cells in the 100,000 per-sheet union-address cap and the 100,000 workbook style cap.
- Bump worker frame version and test parent/child mismatch and unknown-version rejection.

Progress: parser/model bullets above are implemented. The 64 MiB response cap is
unchanged. Tests cover malformed relationships, indices, declared counts,
duplicate custom formats, style tables, DTDs, parent IR boundaries, unknown
worker versions and compatibility reports. A dedicated coverage-guided fuzz
harness is outside this handoff.

### C. Import journal path

- Extend import planning to calculate sheet extent from both content and style coordinates.
- Bind formulas server-side against the candidate workbook; do not accept client-provided bound AST as authoritative.
- Convert supported leaves to validated `StyleWrite` batches of at most 1,000 addresses, with stable server IDs and reset semantics.
- Apply the same preflight quotas to total union addresses, event count, decoded and persisted bytes, and formula/calc work.
- Prove that any validation, authorization, storage, or CAS failure leaves no partially-created workbook or import events.

Progress: the importer plans style-only extents, binds formulas server-side,
and produces guarded style events inside one `create_imported` transaction.
PostgreSQL tests verify style-event idempotency and rollback when a trigger fails
on the style event after candidate validation. Integration tests cover all 16
leaves, a styled blank and content/formula round trip.

### D. Deterministic exporter

- Extend package content types and workbook relationships for the style part.
- Intern identical font/fill/border/number-format records deterministically and use stable indices.
- Apply style indices to both non-empty and blank styled cells.
- Roundtrip the supported profile through the same package reader and compare normalized IR and resulting domain leaves.
- Keep typed 422 in place until the complete flow is green.

Progress: native deterministic stylesheet interning/writing and reader round-trip
are implemented. Server projection exports sparse supported styles, including
styled blanks. The style-specific 422 guard was removed after the runtime gate
passed.

### E. Runtime and recovery gate

- Verify import → durable commit → hard restart/snapshot replay → XLSX export → reimport against a real PostgreSQL-backed server.
- Verify checksums and state hashes using canonical Domain encodings, never protobuf/ZIP bytes.
- Verify export authorization, size limits, errors, and audit behavior; ensure response/logging paths do not disclose document contents.
- Remove the styled-export 422 only after native package tests, journal/recovery tests, and browser download tests pass.

Progress: import → PostgreSQL commit → hard restart/journal replay → XLSX export
→ reimport passes for the styled fixture. Recovery verifies the replay count and
projection. A real browser download and an integration reimport preserve values,
formula source, all 16 leaves and the styled blank. The full Playwright suite
passes at DPR 1 and 2.

## Acceptance gates

- Native parser/exporter roundtrip passes the supported styled fixtures, including a styled blank and multiple sheets.
- Every style leaf survives import, snapshot/replay, export, and reimport with unchanged content and formulas.
- Unsupported formatting has exact typed compatibility-report assertions for both loss policies.
- Invalid relationships, counts, style IDs, schemas, oversized styles/tables/IR, and malformed XML fail before persistence.
- Import remains atomic under a forced failure after candidate planning and before durable commit.
- A real authorized browser download returns XLSX with style indices for populated and blank styled cells; a second import produces the same normalized profile.
- Native and browser grid display the same imported style leaves; calculated values and authored formulas remain independent from formatting.
- Full existing Rust, frontend, recovery, and Playwright suites pass after the change. Record actual counts, machine/topology, build mode, commands, and limitations in RUNBOOK and the benchmark log.

## Known boundaries after this handoff

Passing this stage closes only the bounded style round trip. It does not establish full XLSX compatibility, Excel pixel parity, browser file upload, Linux/Compose/remote CI readiness, S3 snapshot readiness, broad import resource-isolation guarantees, or the remaining collaboration/load/production gates. Track those independently in the roadmap.
