# ADR-015: Bounded PostgreSQL replay across the subscribe/live barrier

Date: 2026-09-29. Status: implemented as a phase-3 slice.

## Context

ADR-014 sent one latest canonical event; a receiver behind by two or more
versions fetched the entire HTTP projection. A coalescing watch receiver can
skip intermediate notifications. It cannot serve as the event history.

## Decision

`SubscribeDocument(last_seen_version)` registers the actor's watch receiver
before reading its authorized head, which becomes the barrier V. The server
reads the append-only PostgreSQL journal for `(last_seen,V]` and sends each
representable user event in document-version order as a
`DocumentHeadChanged` carrying the exact `OperationCommitted`. It sends
`ReplayComplete(V)` after those frames. If another writer commits during
replay, the registered watch receiver wakes afterwards; the server reads all
versions `(V,new_head]` from PostgreSQL. Coalescing therefore affects wakeups,
not the event sequence. A second leader may likewise be observed on the next
head check, but cross-process prompt fanout remains a separate requirement.

Replay is capped at 128 events and 1 MiB of encoded frames per catch-up.
Journal reads use batches of at most eight events, so the encoded-byte cap is
not enforced only after materializing all 128 persisted payloads.
When the gap exceeds the cap or includes a system event without a public wire
representation, the server sends a version-only hint and marks
`ReplayComplete.resync_required=true`. The browser then reads an authorized
HTTP projection. The same fallback applies to live catch-up. The browser
applies an event only when its version is exactly the next confirmed version;
pending local operations remain separate. It uses the barrier to repair any
residual gap. Each outbound frame rechecks document read access and the
development session. A failed check or send closes the socket. No document
content is written to tracing.

`ReplayComplete` is additive wire v1. Persisted event schema and domain hash
do not change. The public marker says the captured barrier has been sent; it
does not assert that the browser successfully applied every event.

## Limits and next steps

The replay buffer is PostgreSQL itself, so it does not grow in process memory
with a slow subscriber. One socket send has a two-second timeout, and the
bounded replay has at most 128 frames before requiring resync. A dedicated
byte-bounded outbound queue and explicit slow-consumer metrics remain open.
System events currently require full projection refresh. Atomic `access_epoch`
invalidation for ACL changes, presence, cross-process notification, and seeded
convergence stress remain required before accepting phase 3. The original
single-event hint remains the same wire shape and is now a degenerate replay.

Integration tests cover multiple missed versions, a commit triggered during
replay, a live commit after the barrier, the system-event fallback and session
revocation. Browser tests cover live two-tab updates; no p95 delivery target is
claimed from these functional tests.
