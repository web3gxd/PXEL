# ADR-110: atomic sorting of a confirmed range

Status: IMPLEMENTED AND LOCALLY VERIFIED (2026-10-06).

## Decision

`SortRange` is a separate versioned command, normalized into one explicit
`CellBatchIntent::SortRange`. The batch freezes the selected geometry, ordered
keys, source row permutation, transformed contents, formatting and exact
preimages. One candidate, PostgreSQL head CAS, durable event and Undo/Redo target
cover the complete sort. Existing batch families retain their previous limits;
sort uses independent authored/inverse event families 37/38.

The selected rectangle is limited to 100,000 cells and the existing 4 MiB
durable envelope cap. The header prefix is excluded from permutation. Any merge
intersecting the sorted body rejects the whole command. Hidden rows remain part
of the range. Sorting does not change row IDs, axis sizes or outside formulas.

Keys use confirmed scalars at the exact input version and calculation context.
Numbers compare numerically; strings use Unicode scalar ordering; booleans sort
after strings. Blank/empty strings and errors always follow scalar values,
independently of direction. Equal key tuples retain the original row order.
An incomplete or stale calculation rejects sorting rather than using a preview.

Relative and mixed formula references use the existing shared formula copy
mechanism between source and destination row positions. Absolute references
remain absolute. Ordinary dependency recalculation handles outside dependents.
Planning runs in a bounded server CPU executor, without clocks or IO in the
domain/formula libraries. Publication follows durable commit.

The UI shows explicit range boundaries, header and independent key directions.
For a single-column selection it may suggest a connected table rectangle, show
a separate preview and require confirmation. It never changes the original
selection automatically. Pending client operations must finish before sorting.

## Verification

Four domain, two formula, three codec, one executor and one wire-admission tests
passed. Two PostgreSQL tests cover exact preimages, formatting, concurrent CAS,
receipt retry, encrypted payload and Undo/Redo replay. Chromium passed 12/12
cases at DPR1/DPR2, including multi-key formulas, outside dependents, hidden rows,
explicit range confirmation and 390/1440 light/dark keyboard dialogs. A hard
restart scenario passed, including sorting, restart, Undo, another restart and
Redo. Typecheck and all 247 frontend unit tests passed. Whole-project final
checks, manual NVDA and external deployment gates remain separate. Detailed
outcomes and corrected failures are recorded in RUNBOOK.md.
