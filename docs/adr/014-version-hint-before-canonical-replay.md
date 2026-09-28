# ADR-014: Coalescing document version hints before canonical replay

Date: 2026-09-29. Status: implemented as an interim phase-3 slice.

## Context

The first browser can commit and restore a document, while a second open browser
does not learn about the new head until it reconnects or reloads. Implementing
the final bounded operation replay/live barrier, access-epoch send gate and
canonical patch queue is a separate semantic gate. Sending unversioned cell
content directly from another client would violate the server-authoritative
model.

## Decision

An authenticated socket may send `SubscribeDocument(document_id,
last_seen_version)`. The document actor registers a coalescing Tokio watch
receiver **before** an authorization/head read. It sends `DocumentHeadChanged`
immediately when the server head exceeds the supplied version, or later after
a committed actor event. The send path rechecks the current ACL and head; the
hint contains only opaque document ID and version. Slow receivers coalesce
versions, never block the actor and never receive document content through this
message. The client performs an authorized HTTP projection read and keeps the
confirmed/pending separation. On reconnect it resubscribes with its last
confirmed version; an initial head check closes the subscribe/commit race.

This is a version invalidation channel, **not** an operation replay protocol or
the final `ReplayComplete` barrier. It does not establish access-epoch revoke
ordering or multi-process fanout. A lost hint is repaired by reconnect/head
check; a failed HTTP refresh is retried. The final phase-3 replay stream may
replace this hint, retaining the same confirmed-projection rule.

## Consequences and verification

Wire v1 gains additive request/response messages; durable event encoding is
unchanged. Every subscription uses the existing bounded actor queue and
per-send authorization. The development session token hash is rechecked in
PostgreSQL before socket commands and document payload sends; logout therefore
stops that socket at its next activity. This does not provide the atomic
document ACL revocation gate promised for the final phase. A real two-socket integration test covers both a live
commit and a late subscriber; a two-tab Playwright test checks both directions
at DPR 1 and 2. Cross-process immediacy, ordered ACL revocation, presence,
canonical replay, slow-client byte accounting and convergence stress remain
required before phase 3 acceptance.
