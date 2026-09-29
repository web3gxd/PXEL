# ADR-031: Preserve reconstruction metadata in the snapshot image

Date: 2026-09-29. Status: ACCEPTED FOR THE FIRST PHASE-5 SLICE.

## Context

The current workbook is rebuilt from an append-only PostgreSQL log. A snapshot that stores only visible cells and axes can reproduce the current cacheless hash yet still change later behavior: deleted row/column IDs could be reused, and the formula binder could accept a stale base if the last axis-change version is lost. A volatile CalculationContext is also part of the authored state hash and must survive restoration.

## Decision

The pure domain exposes a `SnapshotImage` for its currently supported single-sheet workbook. It includes the document/version, last axis-change version, sheet identity/name/ordered axes, sparse authored cells including bound formulas, retired row/column IDs and the optional persisted CalculationContext. It contains no calculated cache, presence or pending client operations. `Workbook::restore_snapshot` rebuilds a bounded projection using the normal hydrate checks, rejects duplicate or live retired IDs, checks metadata bounds and verifies the expected cacheless domain hash before returning a usable workbook. A tail event is applied through the existing `apply_event` version/hash check.

The image is an internal domain transfer shape, not a storage or wire format. The next phase-5 slice adds a distinct versioned binary snapshot envelope, size limits and byte checksum; then PostgreSQL metadata and BlobStore activation implement the `UPLOADING → STORED → VERIFIED → ACTIVE` contract of ADR-004. A domain hash does not include `last_axis_change_version`, so activation must also bind this reconstruction metadata to a verified snapshot payload and its PostgreSQL record. The first slice must not be selected by a recovery reader until those later checks exist.

## Limits and validation

The current workbook has one sheet. Multi-sheet support requires a new image/schema version, with a migration path that preserves old snapshots. Snapshot creation clones the bounded sparse projection; allocation and size must be benchmarked before enabling an automatic scheduler. The test compares direct tail replay to restored-image-plus-tail, checks context and tombstones, rejects live/duplicate retired IDs and detects a wrong expected domain hash. It does not prove blob durability, crash recovery, or S3 compatibility.
