# ADR-001: Server-authoritative document model

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 foundation; detailed collaboration contract requires approval before phase 3.

## Context

Spreadsheet operations combine scalar edits, structural changes and formulas. Clients need immediate feedback, while durable ordering and canonical calculation must survive disconnect/restart. An in-process actor alone cannot exclude a stale second writer.

## Decision

Use a modular monolith with one logical actor per active document and bounded queues. PostgreSQL assigns the durable order through a per-document head CAS and unique operation/version constraints. Prepare/apply/hash an isolated candidate before the transaction; publish its prepared state only after COMMIT. Keep client confirmed projection separate from pending intents. Recalculate asynchronously with version/epoch-bound results.

SessionDirectory first uses in-process routing. Future scale-out needs lease/fencing checked by durable commit, not merely sticky sockets. Access revocation uses ordered control lane and access epochs. Precise protocols live in [collaboration](../collaboration.md).

## Alternatives

Sequence CRDT and third-party collaboration engines add unproven complexity and do not remove spreadsheet semantic conflicts. One global server mutex would block independent documents. Actor-only ordering without DB CAS fails under duplicate writers. Microservices add deployment boundaries without a current load need.

## Consequences

Server is required for canonical commit; clients can only preview offline edits. Every write is replayable. Backpressure and replay are explicit. No distributed consensus service is deployed now. Phase 0 builds transport boundaries only; document commit begins in phase 1.

## Validation

Phase 1 tests two competing CAS writers, duplicate IDs, invalid candidate, crash before/after commit and lost ACK. Phase 3 tests converge/reconnect/barrier/revoke races. None of those future tests are claimed passed by this ADR.
