# ADR-079: atomic guarded range undo and redo

Status: ACCEPTED FOR GUARDED INTERACTIVE RANGE HISTORY (2026-09-30).

## Context and previous decision

ADR077 supports own single-cell writes and compensations. Schema10 value batches
and schema14 frozen copies contain no destination preimages and cannot supply
exact inverses. Old events retain their representation and remain history barriers.
ADR078 fixes final-capacity validation used by private atomic candidates.

## Decision

New interactive SetCellValues and CopyRange commands produce RangeWrite events.
Each distinct stable destination records normalized content and its exact previous
content, including bound formulas and blank-as-absence. Schema17 identifies an
authored range (SetValues or Copy); schema18 identifies its compensation with
target operation ID and version. Wire commands remain version1. Authored plain
batches retain the existing SetCellValues canonical patch; mixed copies and range
inverses use the existing authorized projection-resync path. Client ASTs and
source rereads are never used for redo. Imported legacy batches are unchanged.

UndoCell remains an inverse intent and may target an own eligible range event.
The actor derives the complete inverse footprint, checks all live destinations,
rejects historical formula result/preimage when axes changed after the target,
then validates the private candidate. Deleted destinations return UndoConflict
before formula-axis refusal. Undo and redo create new durable versions.

Inside the document-lock/CAS transaction, verify exact target identity/version,
scalar-versus-range shape, ordered complete destination footprint, and swapped
content/preimage pairs. Scan the contiguous canonical tail once. Each affected
address has its own operation-ID/version stack; exact own inverse pairs cancel
on that address. Scalar and range events participate in the same reduction.
A foreign or legacy touch remains a hard barrier even if later compensated.
Every destination must have an empty effective tail for acceptance. Missing,
extra, reordered or forged inverse destinations reject atomically.

Limits:1–1000 distinct cells;512KiB total incoming content text and512KiB total
preimage text,4MiB persisted envelope. A new authored range exceeding the preimage
limit is rejected as a whole, without accepting an action whose supported inverse
cannot fit. History is bounded by10000versions,64MiB fetched payload bytes and
100000 matching-address updates, including cancellations. Exceeding a history
budget returns UndoWindowExceeded without a new event or version. These limits
bound CPU work and temporary stacks under the transaction lock; they are not
performance promises. Queue/frame/calculation budgets remain independently enforced.

The existing local100-entry acknowledged undo/redo stacks now include accepted
interactive batches and copies. Pending inverse results are not confirmed content;
new acknowledged edits clear redo. Reload resets local history as before.
Structural, style, durable discovered history, cut/move and sorting remain open.

## Required evidence

Domain and codec roundtrips/preimage atomicity; genuine PostgreSQL exact-footprint
and mixed-history guards; foreign-one-destination conflicts; formula restoration,
axis/deletion refusal; idempotent retries after further edits; replay/restart
content and calculated hashes; browser batch/copy undo/redo at DPR1/2; history
budget rejection without head advancement. Keep legacy golden hashes and protocol
checks green. Actual commands/results and performance limits belong in RUNBOOK.

## Recorded local result

Codec29tests, domain/collab/formula suites and server17unit tests passed. Genuine
PostgreSQL4range-guard tests passed together; a separate1test proved the64MiB
independent-tail boundary. Existing scalar undo6tests passed. Full HTTP68cases
passed, followed by2new foreign/axis regressions separately. Chromium98DPR1/2
cases passed, including TSV/mixed-range undo/redo and remote convergence.
Hard-restart2cases passed with persisted range compensation/redo and exact content
and calculated hashes. Shared native/browser corpus554cases and1010workbooks/
2020patches converged. Frontend87unit, typecheck/lint/build, SQLx/protobuf/clippy
and format checks passed. No full deployment/MVP gate is closed by this slice.
