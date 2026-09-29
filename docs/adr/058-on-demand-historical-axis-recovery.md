# ADR-058: Recover missing historical axes on demand

Date: 2026-09-29. Status: ACCEPTED FOR LOCAL RECOVERY SLICE.

## Context

ADR-057 starts an actor cache at its recovered head. A formula authored before
restart can therefore lose its rebase window despite the durable log retaining
the needed events. Loading 10,000 events on every open would defeat snapshot
startup, including documents without delayed commands.

## Decision

Add an optional exact-version recovery callback to the in-process directory.
The server connects it to the existing authorized PostgreSQL snapshot/replay
implementation. No storage adapter types enter the collaboration crate.

After authorized command refresh and idempotency lookup, a delayed formula
whose base is missing from the axis cache triggers recovery. The base must be
positive, not ahead of head, and at most 10,000 versions behind. Recovery shares
the four startup recovery permits; exhaustion rejects with backpressure rather
than waiting unboundedly. A ten-second cooperative timeout covers recovery and
tail reconstruction.

Recover the exact base workbook, validate its document/version, and replay
authenticated, checked event pages of at most 128 up to the actor's captured
head. Domain application verifies contiguous versions and state hashes. Build
axes-only checkpoints using ADR-057 limits, then verify the final domain hash
against the actor head. Drop the transient historical content projection and
replace only the disposable axis cache. Re-normalize the command and perform
the unchanged candidate/CAS/durable commit path. Concurrent external commits
or ACL changes are still fenced by the journal transaction; CAS retries refresh
and validate again.

An existing cache that fails the formula work budget does not trigger expensive
recovery again. Recovery does not publish calculations, snapshots or document
events. Failed recovery does not change confirmed content or acknowledge a write.
There is no wire/persisted schema migration or rewritten historical event.

## Consequences and verification

Ordinary snapshot opening is unchanged. Cache-miss recovery temporarily holds
the reconstructed workbook's content in addition to current head; it is not an
axes-only persisted format. Older base reconstruction may replay from genesis
when no eligible snapshot exists, even though the base-to-head tail is bounded.
Timeout is cooperative: a single synchronous domain application cannot be
preempted. A dedicated CPU executor and large-book recovery benchmark remain
performance gates. Eviction/oversized axes can still require resync after recovery.

The hard-restart test loads head before submitting the old formula, proving the
cache starts without pre-restart history. Insertion preserves the original A1
target, current text becomes A2, and retry returns a receipt. A second restart
after deletion admits an old formula on a surviving origin and returns #REF!
for its deleted target. The dedicated test binary keeps the user's editor live.
This local scenario does not replace the full crash/snapshot/ACL stress matrix.
