# ADR-065: Deterministic volatile functions in an explicit epoch

Date: 2026-09-30. Status: ACCEPTED FOR EXPLICIT-EPOCH EVALUATION ONLY.

NOW/TODAY/RAND have zero arguments; extra arguments return VALUE before reading
context. A trusted evaluator context supplies persisted CalculationContext,
DocumentId and stable CellAddress. Bound evaluation injects these from the
workbook/formula origin. Missing context or engine/profile mismatch returns
typed UnsupportedCompatibilityFeature, never a guessed clock/seed/value.

NOW returns numeric serial day + local milliseconds-of-day /86400000. TODAY
returns serial day from stored civil day. Fixed UTC offset is from the validated
context; no host timezone/clock. The Gregorian calendar profile1900/1904 maps
Unix day0 to25569/24107 respectively. Out-of-calendar values return NUM; no
automatic date formatting. Both computations have bounded work.

RAND derivation version1: SHA256 bytes of `PXEL-RAND\0\x01`, document16bytes,
epoch u64big-endian, sheet/row/column16bytes each, AST NodeId u64big-endian,
seed32bytes. Take the first64 digest bits in big-endian order, shift right11,
and multiply by2^-53. Zero is normalized. Stable AST arena index is the call
identity; distinct RAND nodes derive independently, repeated evaluation in the same
epoch is identical. No traversal/thread-dependent generator and no promise of
Excel's random sequence or cryptographic unpredictability. Hash work charged.

The epoch is explicitly advanced with the already durable authorized recalculation
command in this slice. It dirties all formulas and supersedes old context/results;
replay uses stored inputs unchanged. Automatic Open/Timer/VolatileDependency
epoch admission/coalescing is required subsequent work, not silently simulated
inside the evaluator. Existing context-less books require explicit recalculation
before these functions; that limitation keeps this profile partially supported.

Engine0.8.0; old contexts require explicit migration epoch, no persisted
AST/event/snapshot schema rewrite. Shared stateful expected fixtures cover both
date systems where available, UTC/day boundaries, multiple RAND nodes and lazy
branches, fixed seed/epoch/cell effects, cancellation/budget, browser parity,
live epoch change, reload/replay and native snapshot recovery.

Sources: Microsoft Support [NOW](https://support.microsoft.com/en-us/excel/functions/now-function),
[TODAY](https://support.microsoft.com/en-us/excel/functions/today-function),
[RAND](https://support.microsoft.com/en-us/excel/functions/rand-function), retrieved
2026-09-30. Expected random samples are PXEL derivation goldens, never Excel
verification. Timer/open/locale/date-format/Excel manual gates remain open.

Verification found two real gaps: epoch reconciliation previously copied context
without making cached formulas dirty; it now charges and marks every current
formula dirty when context changes, including coalesced reconciliation. Native
regression checks recalculation, unchanged-epoch no-work and snapshot restoration.
Browser testing found an unrelated receipt/resync race: a validated commit receipt
now retires its operation ID before a refresh can be superseded. Lost responses
still retain the ID. Both fixes have regression tests and read-only review.
