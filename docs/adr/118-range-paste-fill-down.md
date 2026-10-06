# ADR-118: selection-aware paste and atomic fill down

Status: ACCEPTED (2026-10-06).

## Decision

Paste starts at the normalized top-left of a selection. A single selected cell
accepts the original clipboard rectangle. A larger selection repeats the source
block only when both selection dimensions are exact multiples of that block.
Incompatible sizes reject the whole paste with a visible error. TSV blanks,
including the existing internal trailing-blank codec, participate in repetition.
The existing 1,000-write and text/frame bounds remain in force before admission.

Ctrl+D (also Cmd+D) copies the top row of the normalized selection to all rows
below it, across all selected columns. A single selected row copies the row above;
a single row at the top of the sheet is a no-op. Cell editing and
pending commands prevent fill down. The top row is untouched; blank source cells
clear destination contents. Existing destination formatting stays independent.

Scalar TSV still submits one existing SetCellValues batch. Repeated internal
formula/range copies and fill down submit a new wire RepeatRangeCommand intent.
It carries the existing stable-ID CopyRange source block and explicit destination
dimensions. A separate DocumentCommand field makes older servers reject unknown
repetition instead of silently accepting a single copy.

The domain CopyRange intent adds an optional destination extent. None retains
the previous command fingerprint byte sequence; Some includes a discriminator
and both dimensions so receipt retries bind the entire intent. No persisted
event/snapshot version or database schema changes. The collab normalizer requires
the exact source head and checks positive, divisible, bounded geometry. It reads
every tile from the original workbook, transforms relative formula references,
keeps absolute references, and freezes one existing guarded RangeWrite with exact
destination preimages. Candidate validation, PostgreSQL CAS, durable commit,
publication, and author Undo/Redo continue through the existing path.

Cut/move semantics and the separate parallel worktree's PD mode are unchanged.
External TSV formula admission and copying cell formatting remain outside this
slice's content-copy contract.

## Verification

Store regressions cover reversed selections, scalar and row repetition,
incompatible dimensions, formula clipboard version checks and one fill command.
Native tests cover exact formula transformation, stale heads, invalid/overflowing
extents, and overlapping tiles reading frozen source values and blanks. Wire
tests reject malformed extents and competing intents; codec tests distinguish
ordinary copy and different repeated destination extents.

Real Chromium/PostgreSQL tests cover scalar selection paste, mixed formula/value/
blank fill down, internal formula repetition, two-dimensional mixed-block tiling,
one durable version per action, reload, Undo/Redo and browser errors at DPR1/2.
Actual commands/results and remaining deployment gates are recorded in RUNBOOK.

## Merge reconciliation (2026-10-06)

This ADR was numbered 089 on feat/two; merging into main revealed an existing
ADR-089. It is now ADR-118. RepeatRange uses wire field 18; fields 13–17 retain
the existing axis/batch/merge/frozen/sort contracts. Generated descriptors and
TypeScript are rebuilt from the combined schema. Ordinary CopyRange retains
styled copy. RepeatRange preserves destination formatting and normalizes to
RangeWrite(Copy), including a one-row fill. Explicit fill extents always use
RepeatRange; content preimages and durable Undo/Redo are preserved. Clipboard
matrices repeat after bounded quoted-TSV parsing, retaining Markdown literal
and alignment handling and current authorization/anonymization guards.
