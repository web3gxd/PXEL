# ADR-121: local action assistant and drag autofill

Status: ACCEPTED (2026-10-06).

## Decision

The user requested a separate, multifunctional assistant engine with a complete
off switch and an Excel-like fill handle. `AssistantEngine` is a framework-free,
session-only module. It retains at most eight action kinds and coordinates,
never cell contents. Three consecutive edits in adjacent cells detect repeated
scalars, constant numeric steps or formula-entry patterns. Repeated copy, paste,
formatting and Undo/Redo actions produce contextual advice. No external service,
telemetry, new dependency or document-level preference is introduced.

The assistant panel shows the actual configurable shortcut registry bindings,
opens the existing help panel and offers explicit continuation into a user-chosen
number of empty cells. It rechecks the current suggestion and refuses occupied
destinations. Advice never writes automatically. Dismissals last until the next
sheet/document session. Disable clears all observations and prevents collection
and analysis; only the boolean preference is stored in browser localStorage.
Re-enable starts fresh. Sheet/document/axis changes, disposal and rejected
commands clear observations. Protected projections do not feed the engine.

Drag autofill is independent of this engine and remains available when the
assistant is off. A round DOM handle on the lower-right selection corner uses
pointer capture, a separate destination preview, edge autoscroll and viewport
geometry including frozen rows and custom axis sizes. Keyboard arrows plan the
fill, Enter applies it, Escape cancels. Pointer cancel/lost capture, a changed
sheet/head, editing or pending commands prevent application. The preview never
changes confirmed or optimistic cell contents.

One gesture extends along its dominant axis, in any of four directions. A
one-dimensional finite arithmetic numeric seed of at least two values continues
its step, including decimals and descending rows. Generated decimal values use
15 significant digits to remove binary floating-point tails. Other seeds repeat
complete blocks; the preview snaps to whole tiles at sheet bounds. Partial
text/formula blocks, dates and inferred text transformations are not supported.
The limit is 1,000 destination cells per operation. Filtered sheets, active
privacy masking, read-only/copy-restricted projections and sheets with merges
do not expose autofill in this slice. Destination styles remain independent.

Content/formula repetition reuses ADR-118's exact-head RepeatRange intent; the
server reads the frozen seed, shifts relative references and preserves absolute
references. Numeric continuation submits one existing scalar batch. Both use
existing candidate validation, PostgreSQL CAS/durable commit, publication and
author Undo/Redo. Numeric generation is a client-authored batch, not a new
server-calculated result. No Rust boundaries, wire or persisted codecs, migrations
or storage model are changed.

Pointer event behavior follows [MDN pointer capture](https://developer.mozilla.org/en-US/docs/Web/API/Element/setPointerCapture)
and [React DOM event documentation](https://react.dev/reference/react-dom/components/common).

## Verification

Unit tests cover disable/re-enable, dismissals, multiple advice families,
nonadjacent and inconsistent edits, all four directions, complete-block and
resource bounds, decimals, overflow, confirmed/pending separation, version
guards, read-only behavior and refusal to overwrite occupied cells.

Real Chromium/PostgreSQL scenarios verify formula reference shifts, one durable
version, reload and Undo/Redo; numeric continuation; keyboard commit/cancel;
helper action and persistent off state; four directions; and a concurrent edit
cancelling a captured gesture. Results and deployment gates are in RUNBOOK.
