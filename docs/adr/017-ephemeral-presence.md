# ADR-017: Ephemeral presence on the document WebSocket

Date: 2026-09-29. Status: implemented as a phase-3 slice.

## Context

Remote selections need prompt fanout without consuming a document version or
becoming part of its durable operation history. A subscriber can be revoked
while a presence frame is waiting to leave the server.

## Decision

Wire v1 adds `SetPresence` and `PresenceSnapshot`. The client sends stable
sheet/row/column IDs for anchor and focus; the server validates the envelope,
complete nonzero IDs and one sheet. It accepts updates only from a currently
subscribed, authenticated session with its original access epoch. The existing
document delivery read gate protects every outgoing snapshot; ACL revoke's
write gate prevents a stale subscriber from receiving another snapshot after
the durable revoke response.

A process-local directory keeps at most 64 peers per document, keyed by
WebSocket session ID. A Tokio watch channel coalesces snapshots, so a slow
receiver does not retain every pointer movement or block another client.
Updates are capped at one per 50 ms per socket; document fanout is capped at
one snapshot per 100 ms with trailing delivery of the latest state.
Disconnect, access-epoch
change and resubscription remove the old entry. Presence messages are not
persisted, do not alter `head_version`, and never enter the formula state hash.
The browser checks IDs against its current axes and ignores stale coordinates;
the Canvas renderer draws remote selection outlines only in the viewport.

## Limits

This is an in-process channel. Multi-server presence requires a leader-scoped
fanout transport before horizontal scaling. The server checks address shape
and authorization, but does not read the workbook on every pointer update to
verify that a stable row/column ID is currently live; the browser filters
unknown IDs. Only one document subscription is active per socket. A dedicated
byte-bounded outbound queue and slow-consumer metrics remain phase-3 work.

Wire decoder and directory tests cover malformed/partial addresses, capacity,
coalescing and removal. Live integration covers two sessions, fanout and
unchanged document version. Browser E2E covers two distinct users, visible
presence and disappearance after disconnect at DPR 1 and 2.
