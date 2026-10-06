# Handoff: stable, durable merged cell ranges

Status: bounded implementation and local gates completed 2026-10-02.
Related decision: [ADR-096](../adr/096-merged-cells.md). Deployment,
Linux/Compose/remote-CI, broad Excel compatibility, and production gates remain
open.

## Goal

Add rectangular cell merges without weakening the server-authoritative document,
stable axis identity, durable replay, or guarded history contracts. A merge must
survive reload/recovery, render as one cell, interoperate with the bounded XLSX
profile, and undo only while its own merge state is still current.

## Contract

- A merge is a same-sheet rectangle identified by stable first/last row and
  column IDs. Its visible anchor is the top-left cell in current axis order.
- Rectangles must be contiguous, non-empty, bounded, and disjoint. At most
  10,000 ranges, 10,000 cells per range, and 100,000 covered cells in total
  are admitted per sheet to bound projection and Canvas indexing work.
- Merge is rejected if any covered non-anchor cell has content or non-default
  formatting. No data is silently discarded. The anchor retains its content and
  formatting. Unmerge changes only merge metadata.
- Cell writes and style writes to covered non-anchor cells are rejected until
  the range is unmerged. Formula references keep their stable targets; there is
  no implicit formula rewrite or redirect to the anchor.
- Structural row/column edits that touch a merged rectangle are rejected until
  an explicit, tested range transformation contract is added. Inserts before or
  after a range preserve its stable bounds.
- Merge/unmerge are authored, typed operations with exact previous-state guards.
  One action is one durable event and one Undo/Redo target. Old event and
  snapshot meanings remain unchanged; new schemas are additive.
- Snapshots and the canonical domain hash include sorted merge metadata. Restore
  validates geometry, overlap, IDs, cell/style constraints, and quotas before
  accepting the snapshot.
- The browser shows merged cells as a single visual/editor hit target while
  selection/range geometry continues to use stable axes. Canvas projection does
  not materialize merged DOM cells.
- XLSX import/export maps bounded OOXML `mergeCells` ranges into the same
  canonical model. Unsupported or malformed ranges produce a typed report or
  reject atomically; they are never silently dropped.

## Incremental implementation and gates

1. Domain types, validation, guarded merge state, hash/snapshot, axis/edit guards;
   native tests for overlap, hidden data/style, bounds, unmerge, stale history,
   and stable IDs.
2. Durable codec, snapshot codec, journal provenance and migrations; replay,
   hash, same-operation retry, Undo/Redo, and hard-restart tests on PostgreSQL.
3. Wire command/projection and Canvas merge rendering, hit testing, editor,
   keyboard selection, toolbar action, and browser Undo/Redo tests.
4. OOXML normalization, import report and deterministic export/reimport fixtures;
   supported profile tests for horizontal/vertical/rectangular merges, invalid
   and overlapping ranges, formulas, styles, and blank anchors.

Stages 1–4 are implemented and locally verified: domain/hash/snapshot, guarded
durable event plus PostgreSQL replay/Undo/Redo, client projection/Canvas/hit
target, and bounded OOXML import/export. The UI and XLSX path are enabled only
after durability tests passed. Exact commands and limitations are in RUNBOOK.

## Out of scope for this bounded slice

Row/column insertion through a merged range, automatic unmerge, table objects,
array formulas, arbitrary OOXML extensions, and broad Excel pixel parity. These
need separate contracts and must not be implied by profile-1 merge support.

## Local verification

- `cargo test --locked --workspace --all-features`: passed after correcting two
  stale schema assertions; its default run kept 4 storage timing tests and 8 S3
  contract tests ignored because they need separate timing setup or an endpoint.
  The local S3 contract recipe was run separately and passed 8/8 below.
- `cargo clippy --locked --workspace --all-features --all-targets -- -D warnings`:
  passed after cleanup in the XLSX range validator/parser signature.
- XLSX package: 20/20 tests, including bounded external merge parsing and own
  OOXML export/parse round trip. PostgreSQL journal merge commit, retry,
  Undo/Redo, and replay are included in the workspace suite.
- The optional S3 adapter contract passed 8/8 against local Moto 5.2.3 over
  loopback with a temporary bucket and dummy credentials; this verifies the
  adapter contract, not a production S3 service/configuration.
- Chromium focused merge interaction: 2/2 at DPR1/DPR2, covering one hit target,
  reload, Undo and Redo. The full 190-case suite passed on rerun at DPR1/DPR2
  (14.6 minutes). An earlier full run had one DPR1 Move Redo timeout; it passed
  in the complete rerun and in eleven focused DPR1/DPR2 retries. Both outcomes
  and the first trace are recorded in RUNBOOK.
- Follow-up recovery tests against fresh local PostgreSQL clusters passed: an
  immediate stop recovered merged snapshot v3 plus one committed tail event;
  physical `pg_basebackup`/`pg_verifybackup` restore recovered v5 from snapshot
  v3 plus two tail events; named-point PITR recovered v4 from merged snapshot
  v3 plus one tail event while excluding v5. Restored projections and historical
  v3 retained the stable-ID merge range and canonical state hash.

The conservative profile rejects structural edits intersecting a merge, and
rejects merges hiding non-empty content or formatting. Linux/Compose and remote
CI remain unavailable on this Windows host; broad Excel golden compatibility,
OS-isolated browser upload, and production storage/deployment are not claimed.
