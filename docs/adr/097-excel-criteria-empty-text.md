# ADR-097: Excel verified empty-text criteria behavior

Status: ACCEPTED for the locally verified scalar, native/WASM parity, browser
application and live collaboration cases (2026-10-02).

## Context

ADR-074/075 left empty-text wildcard and inequality behavior provisional. Direct
comparison against Microsoft Excel showed that the existing evaluator treated a
formula result of `""` as empty for both `"<>"` and `"*"`, while Excel treats
those criteria differently. A blank cell and a formula returning empty text are
distinct `Value` variants in PXEL and must remain so during criteria matching.

## Decision

- For empty criteria, `"<>"` excludes a genuinely blank cell but includes a
  formula cell whose calculated result is empty text.
- The wildcard criterion `"*"` does not match empty text. Other wildcard
  matching and comparison rules are unchanged.
- Store the observed formulas/results in the dedicated Excel golden fixture;
  keep the shared derived corpus distinct and explicitly record expectations
  updated by this evidence.
- Advance formula engine identity to `pxel-formula-0.15.0`. Existing persisted
  calculation contexts from 0.14.0 are incompatible and must be recalculated
  under the existing version-mismatch path before their results are published.

## Evidence and limits

The fixture was evaluated by an isolated Microsoft Excel COM process using
Microsoft Excel 16.0.20326.20158, Windows 11 Pro build 26100, locale en-US,
1900 date system. It covers COUNTIF, SUMIF, COUNTIFS and SUMIFS over a small
range containing a number, numeric text, a genuinely blank cell, a formula
returning empty text, a boolean, an error, and numeric sum values. It verifies
15 outputs and is not a general Excel compatibility claim. Other criteria
coercion, Unicode collation, locale behavior, arrays and Excel edition parity
remain provisional.

## Consequences

The change is confined to scalar criteria matching and formula identity. There
is no wire schema or persisted operation change. Browser clients reject old
calculation patches by engine identity; the server recalculates to establish a
current context. Native formula tests, web unit/type/lint/build checks, target
WASM compilation, and a 569-case Chromium native/WASM Worker parity run passed.
After the identity change, the isolated PostgreSQL/WebSocket integration suite
passed 80/80 and the full Chromium browser E2E suite passed 190/190 across DPR1
and DPR2. The integration run exposed and verified a fix for normalized
MoveRange history and stale formula binding; details are recorded in RUNBOOK.md.
These are local Windows gates, not Linux/Compose, remote CI, production,
performance-load or broad Excel compatibility evidence.
