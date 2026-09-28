# ADR-006: Required corrections after architecture review

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 only.

## Context and superseded proposal

The initial proposal in RUNBOOK was PROPOSED and had semantic gaps: first domain apply after COMMIT, insufficiently explicit head conflict handling, optional calculation in ACK, vague historical formula binding/revoke/replay mechanisms, combined wire/storage codec, early full-row SortRange and phase 0 Garage. The user conditionally approved the direction and required these corrections before skeleton.

That proposal remains a clearly marked historical record. ADR-001…005 contain the corrected foundation; this ADR records the change and its consequences rather than pretending the old choices never existed.

## Decisions

1. Head CAS plus unique versions/operation IDs; conflict repeats preparation and the entire transaction.
2. Canonical operation applies to an isolated candidate and hashes before commit; post-commit publication is a prepared state swap. Quarantine prevents writes from a broken projection.
3. Separate OperationCommitted and CalculationCommitted; every calculation carries input_version/epoch/context/sequence/final status, and stale results cannot replace current state.
4. Persist CalculationContext through AdvanceCalculationEpoch system events; no seed/time in every user operation. RAND is independent of traversal and worker scheduling.
5. Bounded 10k-version/16MiB transformation window with axis checkpoint reconstruction; server binds formula text at base_version or returns RebaseWindowExceeded.
6. Separate copy/move/cut/insert/delete semantics. Early SortRange removed; future sort permutes rectangle contents only. Relative deltas are normalized after structural changes.
7. Access epochs and ordered authorization control lane close payload send gates and invalidate queues; already sent bytes cannot be recalled. Retry receipt excludes stale calculation/content payload.
8. Application owns used ports. Wire and storage encodings have independent schemas/crates. Phase 0 creates only BlobStore port, not unused future interfaces.
9. Register subscription at actor barrier before replay DB reads; ReplayComplete precedes buffered live drain.
10. Phase 0 uses LocalFilesystemBlobStore only. S3/Garage wait for snapshot use case and contract tests.

Additional decisions: snapshot explicit lifecycle/retained ACTIVE generations; reverse range index and work budgets; independent byte/cell/cost limits; Linux cgroups and Windows Job Objects for XLSX workers; candidate toolchain status; no project MSRV promise before production v1; durability-qualified performance thresholds on fixed runner only.

## Alternatives rejected

Actor-only exclusion, post-commit fallible apply, replay-then-subscribe, UI-only ACL invalidation and stale-base current-coordinate binding all admit correctness/security races. Full-row sorting under a rectangle operation risks changing unrelated content. Early unused object-storage service and unused ports increase foundation cost without validating snapshots.

## Migration and scope

No persisted documents or implemented command protocol existed when this decision was recorded, so no data migration is needed. Historical documentation is superseded; current documents and future code must use corrected contracts. Storage codec appears when its first event is persisted, and database migrations remain append-only after use.

This acceptance authorizes phase 0, not all formula/collaboration semantics. Gate 0 must pass before phase 1; phases 2 and 3 retain explicit confirmation checkpoints. Actual tool availability and executed checks must be recorded rather than inferred from release metadata.

## Validation

RUNBOOK records all ten corrections before skeleton. Future tests target candidate failure, duplicate writers, lost ACK, stale calculation, rebase window boundary, send/revoke race and replay barrier. Filesystem contract tests are phase 0 scope. No future test is claimed passed by this decision.
