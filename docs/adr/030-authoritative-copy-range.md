# ADR-030: Freeze a mixed range copy in one authoritative event

Date: 2026-09-29. Status: ACCEPTED FOR A PHASE-4 SLICE.

## Context

The value-only TSV batch cannot preserve a rectangle containing formulas. Replaying a copy by rereading its source would make the result depend on later edits. Client-authored bound ASTs are not trusted, and the wire v1 canonical-operation union cannot safely add a representation that old clients understand as a direct patch.

## Decision

`CopyRange` is an intent with stable source and destination anchors, row/column counts and an exact document `base_version`. The document actor accepts at most 1,000 cells, within one sheet, only while `base_version` equals its current authoritative head. It reads the entire source from that head before writing any destination, so overlapping copies use the original source. Formula text is translated by the Rust formula copier and bound again on the server at each destination. Values, formulas and blanks become a frozen list of destination writes in one schema-14 persisted event, one candidate application and one PostgreSQL CAS commit. Replay never reads the source.

The source-version check is conservative: a stale copy gets the non-retryable `SourceVersionChanged` code with `resync_required`, and the client refreshes before asking the user to copy again. The event fingerprint covers the original source/destination intent and dimensions; the event checksum and domain hash cover the frozen operation. Unknown schema versions fail atomically. The value-only TSV path remains for plain ranges; the internal rich clipboard uses the new command when a multi-cell range contains a formula.

The current wire v1 has no canonical `CopyRange` patch. A committed notification therefore carries its version/hash without a direct operation; clients fetch the authorized projection. Replay also uses the projection fallback for this operation. This costs an HTTP read and means this slice does not meet the eventual optimistic range-copy latency target. A future wire revision can add an explicit bounded patch, with old-client fallback retained.

## Boundaries and follow-up

`CopyRange` does not implement cut/move, cross-sheet copy, merged cells, styles, batch undo or conflict-aware source rebase. The existing command and persisted-event size limits still apply. Add a direct canonical patch only after old-client behavior and broadcast byte limits are specified and tested. Before loosening the exact-head rule, provide historical source-axis projections and formula-reference transforms for every intervening structural operation.

Verification includes domain overlap/blank semantics, schema-14 codec round-trip, PostgreSQL/WebSocket commit/retry/stale-source/overlap tests, hard-restart replay, a two-browser convergence test and a frontend unit test for the head-hint fallback. The test results belong in RUNBOOK rather than this decision record.
