# ADR-093: Atomic cell content and style batches

Status: accepted and locally verified, 2026-10-02.

## Context

Markdown GFM table alignment is authored formatting. Sending imported values as
one range write and alignment as a later style write would expose a partially
imported table and create two separate Undo targets. The operation log must
preserve atomicity, server authority, and exact inverse preimages for both
properties.

## Decision

- Add wire `SetCellBatchCommand` for a bounded set of scalar cell values and
  explicit per-cell style patches. It is one document command.
- Normalize it against the current authoritative projection into
  `CellBatchWrite`, containing guarded content writes, guarded style writes, and
  optional compensation identity.
- Commit values and styles in one candidate, event, CAS version, and durable
  journal transaction. Undo writes both preimages in one compensation event;
  journal admission guards content addresses and style leaves independently.
- GFM alignment markers map to the existing horizontal alignment style leaf.
  Unspecified columns retain their existing alignment. Tables without
  alignment markers keep using the existing `SetCellValues` path.
- New persisted schemas are 28 for an authored batch and 29 for its inverse.
  Storage protobuf field 26 and wire command field 14 are additive. Existing
  schema versions, semantic hashes, and the server-authoritative PostgreSQL
  model remain unchanged.
- The batch stays within the existing 1,000-cell, 512 KiB batch text,
  32 KiB-per-cell, style-profile, and event-size limits.

## Consequences

Markdown alignment survives reload and is included in the same durable Undo
target as table values. The storage format now has one content-plus-style batch
operation that future structured import and rich copy/move work can reuse.
Rich Copy/Move style semantics remain a separate roadmap slice.

## Verification

Domain, codec, PostgreSQL journal, and browser checks cover authored and inverse
events, schema/hash round trips, invalid style footprints, exact Undo, and GFM
alignment at Chromium DPR1/2. Final command results are recorded in RUNBOOK.md.
