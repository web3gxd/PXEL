# ADR-112: fixed-version owner formula profiling

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-06).

Owner analysis captures a confirmed Workbook/version and persisted calculation
context, creates its own CalculationProjection and computes without publishing
results, changing head/epoch or affecting Undo/Redo. The pure calculation API
accepts an observer for formula start/end and work counters; the server executor
observer owns monotonic clocks. Normal calculation uses a no-op observer.

Profiler DTO 1 includes graph preparation and total durations, formula durations
and work units, graph/evaluation work, result bytes and dependency metadata.
Static dependencies remain explicitly potential, including conservative branches
and cycle-blocked descendants. Graph output is bounded to 500 nodes and 2000
edges with an explicit truncation flag. Reports carry version/context and become
stale after edits. The UI offers slow-formula rows, time bars, an interactive
selected dependency chain and an accessible text equivalent.

`DocumentSession::read_confirmed` captures head without initializing a missing
volatile context. Analysis of a volatile book without a persisted context rejects
with `profile_context_missing`; ordinary editor reading retains its established
bootstrap behavior. Diagnostics also use the pure confirmed capture.

Admission, calculation work budgets, final ACL delivery lease and technical-only
logs apply. No formula/document contents are logged. Whole-project and external
gates are recorded separately. Three pure observer/graph tests, 25 calculation
regressions, one actor confirmed-read test and one server executor test passed.
Chromium passed 12/12 DPR1/DPR2, including exact normal calculation hash, frozen
volatile context, potential branches/cycles, owner-only authorization, truncation,
stale reports and keyboard 390/1440 light/dark. Typecheck/lint, all 251 frontend
tests and all-target server/WASM Clippy passed. Final whole-project and manual
NVDA/deployment gates remain open.
