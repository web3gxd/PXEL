# ADR-101: Excel numeric literal precision

**Status:** Accepted for the observed Excel 16 numeric-constant and selected
rounding cases (2026-10-02).

## Context

The formula lexer parsed decimal constants directly to binary64. A direct Excel
COM comparison showed that Excel normalizes entered formula constants to at
most 15 significant decimal digits before evaluating them. For example,
`=ROUNDUP(1.0000000000000002,0)` is stored by Excel as `=ROUNDUP(1,0)` and
returns `1`; the old PXEL parser returned `2`.

The same capture covered selected `ROUND`, `ROUNDUP` and `ROUNDDOWN` cases,
large integers, scientific notation and a subnormal-scale decimal. It is evidence
for a bounded profile, not general Excel floating-point parity.

## Decision

- Before converting a formula numeric token to binary64, preserve its exponent
  and decimal point and replace mantissa digits after the 15th significant
  decimal digit with zero. Leading zeroes do not count. Numeric cell inputs and
  computed results are unaffected.
- Keep finite-number validation and normalized-zero behavior after parsing.
- Advance formula engine identity to `pxel-formula-0.16.0`. Calculation
  contexts persisted with `0.15.0` are incompatible and must use the existing
  authorized calculation-epoch migration path; historical events are not
  rewritten. Formula AST and wire/persistence schemas do not change.
- Keep a separate Excel COM fixture with normalized formula text, Excel build,
  OS, locale and observation date. Apply a `1e-30` absolute tolerance only to
  the recorded tiny value where Excel and Rust differ by one ULP.
- Preserve raw evaluator JSON in the native fixture runner. Parsing its output
  to `serde_json::Value` and serializing it again changed one floating-point
  value by one ULP before the browser comparison.

## Evidence and limits

`fixtures/formulas/excel-number-precision-2026-10-02.json` contains 26 cases
captured by `scripts/capture-excel-number-precision.ps1` with Microsoft Excel
for Microsoft 365 build `16.0.20430.20092`, Windows build `26100` and locale
`en-US`. The Rust formula crate test passed. The native Rust and actual browser
WASM Worker agreed across the combined 595-case corpus, including all 26 new
Excel observations, after the fixture runner preserved the original JSON
numbers. The selected observations do not cover every decimal conversion,
operation result, Excel release, locale or platform. The tolerance is not a
general native/WASM convergence allowance; normalized result hashes still
match exactly.

## Consequences

This is a lexer semantic change, so formula engine identity changes and old
calculated contexts require recalculation. No operation, AST schema, wire
schema or durable codec changes. Full Excel numeric parity, broad criteria and
locale compatibility remain open roadmap work.
