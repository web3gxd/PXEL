# ADR-111: shared formula syntax and hardcode diagnostics

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-06).

The shared diagnostic AST provides leaf spans for references, numeric, text and
boolean constants. Spans remain UTF-8 byte offsets; the browser explicitly
converts validated boundaries to UTF-16 offsets. Highlighting never rewrites input
or changes calculation. Invalid syntax has no partial highlighting report.

Series are grouped by stable sheet and visual row/column position. Only series
with at least 100 formulas and a dominant normalized template of at least 90%
produce warnings. Relative references normalize against each formula origin;
absolute references retain positions. A rare differing template with different
literal constants is a heuristic warning; repeated normal constants are quiet.
Every warning has an explanation and a concrete dismissible fingerprint.

Native analysis captures a readable confirmed book and version, runs bounded pure
analysis on a separate projection under CPU admission, and returns diagnostics
DTO 1. No authored commit, calculation epoch or Undo change occurs. Browser
syntax uses the same Rust code through WASM. Reports become stale on head change.
IO and clocks remain outside the formula library. Fingerprint dismissals are
bounded encrypted personal-view settings, separate from authored events.

Two native tests cover Unicode spans, mixed references, 99/100 and exact 90%
thresholds and uniform constants. One native WASM adapter test passed. Two
PostgreSQL tests cover encrypted personal views including a dismissal; hard
restart restored the dismissal with filters and frozen rows. Chromium passed
10/10 cases at DPR1/DPR2, including 100 real durable formulas and unchanged head,
context and Undo/Redo after analysis/dismissal. Typecheck, lint, all 249 frontend
tests and server/WASM all-target Clippy passed. Final whole-project and manual
NVDA/deployment gates remain separate; details are in RUNBOOK.
