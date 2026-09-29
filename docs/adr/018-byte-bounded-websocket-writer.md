# ADR-018: Byte-bounded per-socket WebSocket writer

Date: 2026-09-29. Status: implemented as a phase-3 slice.

## Context

The document actor never waited for a socket, but the connection task still
called `socket.send` directly. A two-second send deadline bounded one call;
it did not define a queue budget across replay, calculation and presence
frames. Bursts could also delay reading incoming control messages.

## Decision

After the Hello/Welcome handshake, each socket is split into a reader task
and one writer task. A bounded Tokio channel holds at most 160 frames; an
owned semaphore permit charges their total payload bytes at 2 MiB. Queue
admission is nonblocking. Exhausting either budget closes that client without
blocking a document actor or another subscriber. The writer caps each actual
send at two seconds and signals its reader to stop on failure.

Document payload frames carry their original `DocumentAccess` ticket and a
reference to the document delivery gate. The writer acquires the gate's read
side, rechecks PostgreSQL ACL/access epochs and the authenticated development
session, then holds the gate through the socket send. An ACL revoke's write
side waits for any in-flight send; queued old-epoch frames are rejected when
they reach the writer. This preserves ADR-016's durable revocation boundary
after moving the actual send out of the reader task. Handshake, generic
protocol errors and rejected commands contain no document payload and use
their own path. If a preliminary ACL lookup lacks a ticket but the command
returns a commit or receipt, the server closes the socket instead of sending
that response without a delivery gate; the client resolves it by replay.

The queue capacity exceeds the current 128-event replay cap plus its barrier;
the 2 MiB budget exceeds the replay's 1 MiB encoded-byte cap while still
bounding other queued frames. Content remains in memory only for the lifetime
of its socket queue. `outbound_queue_overflow` tracing records the exhausted
limit, never frame content.

## Limits and verification

Count/byte unit tests verify independent caps and permit release on failed
admission. Live replay, ACL revoke, calculation, presence and two-browser
scenarios pass through the new writer. A real kernel-level slow-reader test,
queue depth histogram and reference-topology latency measurement remain open.
The writer is local to one server process; multi-process fanout still needs a
leader/fencing design.
