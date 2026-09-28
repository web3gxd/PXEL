# ADR-004: Transactional log and verified snapshots

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 storage foundation; snapshots implemented in phase 5.

## Context

Process memory/WebSocket are not durable. Replaying a growing journal should remain correct while snapshots accelerate startup. Transport schema must evolve independently from years of persisted data.

## Decision

PostgreSQL stores append-only versioned protobuf event envelopes in bytea, with CAS head/event/idempotency in one transaction. Separate spreadsheet-storage-codec is introduced with the first event; public wire DTOs have a different schema lifecycle. Byte checksum and canonical domain state hash are distinct.

Snapshot lifecycle is UPLOADING→STORED→VERIFIED→ACTIVE, or REJECTED. Activation requires downloaded/stream-verified bytes and domain reconstruction/hash validation, then a short metadata transaction. Multiple previous ACTIVE generations remain available for fallback; a preferred pointer picks the newest appropriate one. GC is separate and retention-aware. Operation log remains authoritative and is not automatically removed.

BlobStore is an application port. Phase 0 uses only LocalFilesystemBlobStore and its contract tests. S3BlobStore and Garage/Russian-S3 contract runs begin in phase 5. Neither MinIO nor Garage is a phase 0 service.

## Alternatives

One JSON workbook loses scalable operation history. Kafka is unnecessary for the current single application. Snapshot-as-authority breaks tail durability. Shared wire/storage protobuf types couple protocol deployments to persisted history. Immediate S3 deployment introduces an unused dependency.

## Consequences

Every format is versioned; read migrations preserve old bytes. Unsupported events stop replay. Snapshot failures leave recoverable metadata/orphans rather than silently published state. Production still needs PITR/backups and tested restore; local COMMIT is not disk-loss protection.

## Validation

Phase 0 runs filesystem contracts; phase 1 atomicity/idempotency/CAS; phase 5 corruption/fallback/crash matrix and snapshot+tail equality. Source: [protobuf serialization is not canonical](https://protobuf.dev/programming-guides/serialization-not-canonical/).
