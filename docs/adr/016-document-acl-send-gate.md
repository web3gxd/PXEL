# ADR-016: Document ACL epoch and process-local payload send gate

Date: 2026-09-29. Status: implemented for the single-process MVP.

## Context

An authorization check before queueing or serializing a WebSocket frame does
not prevent that frame from being sent after an ACL revoke commits. A document
actor also cannot fence a socket send that runs outside its queue. The phase-3
contract requires a durable epoch and a linearization point for payload sends.

## Decision

The owner-only HTTP permission command accepts a UUIDv7 `change_id`, a target
user ID and a grant (`editor`/`viewer`) or revoke action. It may invite an
existing user into the owner's workspace on grant; the document permission is
still required to read content. The transaction locks membership rows before
the document row, checks ownership, deduplicates `change_id`, changes the ACL,
increments `documents.access_epoch` and appends a schema-1 `audit_events` row.
The audit table is immutable and separate from authored workbook operations.
No document version is consumed by an ACL change.

Each active server process keeps a weakly indexed per-document delivery gate.
Every document WebSocket payload send holds its read side from the final
authorization/epoch check through the bounded socket send. The permission
command holds the write side through its durable transaction and epoch
notification. The write lock waits for earlier sends; after the commit no
send may begin using the previous epoch. Subscribers watch epoch changes and
close, discarding any queued replay or calculation frames. Repeated identical
`change_id` returns the original audit receipt without a new epoch or signal.
Rollback releases the gate without changing its signal. Process crash closes
all sockets, and reconnect reads durable ACL/epoch state from PostgreSQL.

ADR-018 moved the actual socket send to a byte-bounded writer task. That writer
now owns the final access check and read gate; queued old-epoch frames fail
closed after revoke. The linearization rule above remains unchanged.

Local development accounts can request a workspace in their dev session only
when they are members. This is feature-flagged development authentication,
not the production identity provider. Grant/revoke uses server-authenticated
principal identity, never an actor ID asserted in the request.

## Limits and verification

This gate fences sends only inside one server process. Distributed fanout will
require a leader lease or acknowledged cross-process epoch invalidation before
multi-process serving. Direct administrative SQL that skips this command is
outside the supported live revocation path. Membership revocation and a user
facing sharing panel are separate phase-3 work. The gateway caps each socket
send at two seconds and acquiring the ACL write gate at three seconds; timeout
returns a retryable service error without claiming a mutation.

Storage tests cover grant, role change, idempotent retry/collision, revoke,
append-only audit and denied access. A gate test proves write waits for an
in-flight read. Live integration covers two users and subscription closure;
recovery covers grant/revoke across hard server restarts. This evidence does
not complete the phase-3 convergence/presence/slow-consumer gate.
