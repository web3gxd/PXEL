# ADR-062: Bounded Unicode text functions

Date: 2026-09-30. Status: ACCEPTED FOR THE VERIFIED TEXT SLICE.

## Contract

| Function      | Arguments            | Rule                                                                                            |
| ------------- | -------------------- | ----------------------------------------------------------------------------------------------- |
| CONCAT        | 1–253 scalars/ranges | Concatenate in argument/row-major range order; blank contributes empty text                     |
| LEFT / RIGHT  | text, optional count | Default count 1; truncate fractional count; negative count VALUE; oversize count clamps to text |
| MID           | text, start, count   | One-based truncated start; start <1 or negative count VALUE; beyond end empty                   |
| LEN           | text                 | Count Unicode scalar values                                                                     |
| TRIM          | text                 | Remove leading/trailing ASCII spaces and collapse internal ASCII space runs only                |
| LOWER / UPPER | text                 | Rust pinned Unicode full case mapping; locale-independent                                       |

Text coercion uses the existing arithmetic-concatenation profile: finite number
shortest decimal, boolean TRUE/FALSE, blank empty, spreadsheet errors propagated.
Numeric indices use existing scalar arithmetic coercion. DateTime formatting,
arrays, implicit intersection and locale-specific numeric/case formatting are
not supported; return typed spreadsheet errors rather than hidden defaults.

Character indexing follows Unicode scalar values, corresponding to modern Excel
surrogate-pair counting. Combining marks, variation selectors and ZWJ remain
separate, not grapheme clusters. Legacy Excel UTF-16 counting is not supported.
See Microsoft [LEFT](https://support.microsoft.com/en-us/excel/functions/left-function),
[MID](https://support.microsoft.com/en-us/excel/functions/mid-function),
[compatibility versions](https://support.microsoft.com/en-gb/excel/compatibility-versions),
[TRIM](https://support.microsoft.com/en-us/excel/functions/trim-function),
[CONCAT](https://support.microsoft.com/en-us/excel/functions/concat-function),
[LOWER](https://support.microsoft.com/en-us/excel/functions/lower-function) and
[UPPER](https://support.microsoft.com/en-us/excel/functions/upper-function).
This is a documented PXEL profile, not proof of all Excel locale behavior.

Input/output work is charged per scanned Unicode scalar; existing 32KiB UTF-8
evaluator text cap is retained, returning CalculationLimitExceeded. CONCAT also
has a 32767-character logical cap returning VALUE; resource failure is never
swallowed by IFERROR. Thus multibyte texts can hit the byte resource cap before
the Excel character cap. Bounds are independent of huge numeric indices.
Range visits validate coordinates, geometry, work and exact resolver visit count.
Sparse ranges include blanks in fixed row-major workbook order, without skipped
error cells or a million dependency edges. No new dependency or domain DTO.

Engine identity advances to0.6.0. Pinned Rust toolchain owns Unicode tables;
changing case results after a toolchain/table update requires an engine identity
change and differential corpus. Old persisted contexts require explicit epoch
advance. AST/event/snapshot schemas remain unchanged.

## Evidence required

ASCII/Cyrillic/emoji/combining Unicode fixtures, NBSP and tabs preserved by TRIM,
case expansion, default/fractional/negative/huge indices, range order/errors,
arity/coercion/resource limits and generated Unicode properties. Native/browser
expected/hash and stateful patch parity; live durable calculation/UI reload;
range benchmark. Manual Excel provenance remains unverified until run in Excel.

Local proof: formula/WASM unit suites, four text tests including generated Unicode
split, resource/index/case-expansion regression; 284 shared expected/native/browser
cases and 1001 stateful books/2002 patches converged. Live PostgreSQL text formula
and UI dependent-edit/reload passed at DPR1/2. Independent review caught uncharged
numeric text index coercion; it is capped/scanned before parsing and re-reviewed.
See [benchmark conditions](../benchmarks/2026-09-30-text-local.md).
