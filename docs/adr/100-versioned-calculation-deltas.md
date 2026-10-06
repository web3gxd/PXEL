# ADR-100: Versioned calculation deltas

**Status:** Accepted

**Date:** 2026-10-02

## Context

The formula evaluator already recalculates the dirty dependency closure and
produces changed and cleared result addresses. Collaboration retained that
patch, but the WebSocket layer discarded it, sent the full formula-result map,
and the browser cleared its calculation cache on each confirmed head. A
single-formula update therefore transferred and reassembled every result.

Calculation remains server-authoritative and separate from authored operations.
The existing full result hash continues to identify the complete calculated
state.

## Decision

- Add optional `base_input_version` to `RequestCalculation`. Clients request a
  delta only when they have a complete calculated map for that exact version.
- Add optional `base_input_version`, `evaluated_cells` and `dirty_cells` to
  `CalculationCommitted`. A delta is emitted only if the requested base equals
  the calculation worker's actual prior calculated version and precedes the
  response version. A missing or mismatched base receives a full snapshot.
- Keep the response atomic across ordered, bounded chunks. Each chunk carries
  the same snapshot/delta mode, base and evaluator metrics. The final chunk
  alone carries the hash of the complete calculated state.
- In a delta, a `CalculatedCell` with no value clears a formula result removed
  from the current head. Full snapshots must contain a value for every formula.
- The browser stages a full snapshot in an empty map or a delta in a copy of
  the exact-base cache. It validates addresses against the current confirmed
  formula set, rejects duplicate cells and mixed chunk metadata, applies clears
  only to cached non-formula cells, and publishes only after the final formula
  count and hash metadata validate. A failed or stale patch cannot replace the
  last confirmed result map.
- Older clients omit the new request field and continue receiving full
  snapshots. New clients continue accepting older full-snapshot messages.
  Wire fields are additive; durable codecs and operation acknowledgements do
  not change.
- Browser instrumentation reports server `evaluated_cells` and `dirty_cells`
  separately from calculation patch latency. `evaluated_cells` counts formula
  evaluations; `dirty_cells` is the worker's invalidated set size.

## Consequences

Matching-base edits send only changed and cleared results; a calculation with
no changed outputs can send an empty delta and still advance the calculated
version and full-state hash. Reconnects, cache misses, rebuilds and skipped
versions safely fall back to full snapshots.

The evaluator still hashes the complete result map and keeps the complete map
for fallback responses. This ADR removes full-result WebSocket transfer on a
matching base; it does not claim sublinear full-state hashing or make a missed
base cheaper than a full snapshot.

The payload budget remains 256 KiB per frame, and the browser still limits a
calculation assembly to 32 MiB and 100,000 formula results. Existing
calculation input-version, context, epoch and sequence checks remain required.
