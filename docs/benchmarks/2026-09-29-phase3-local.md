# Local collaboration latency, 2026-09-29

This is a reproducible local observation, not a production SLA or a shared-CI
threshold. The harness is `tests/performance/collaboration.test.mjs`, run by
`just bench-collab`. The server was started with `cargo run --release --locked
-p pxel-server --features dev-auth`; the Node client and server used loopback.
PostgreSQL 18.6 ran natively on the same Windows host at `127.0.0.1:55432`.
Its live settings were verified with `SHOW`: `synchronous_commit=on`,
`fsync=on`, `full_page_writes=on`. The pool was warm after 100 warm-up edits
in each run. No persistence mock was used.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H, 22 logical CPUs, 15.61 GiB
RAM, Samsung MZAL81T0HDLB-00BL2 host disk. Node 24.18.0. Five independent
documents were created; each run made 100 warm-up and 1,000 measured sequential
rewrites of one numeric cell in an otherwise empty 1,000×26-axis sheet. The
second WebSocket client was subscribed before measurement. Each ACK and
canonical head event was checked against the expected document version.

All values are milliseconds measured with Node `performance.now()` from just
before command encoding/send. ACK is the arrival of `OperationCommitted` at
the writer client; second-client visibility is the arrival of
`DocumentHeadChanged` at the subscribed client. Both include local transport,
server work and PostgreSQL commit. They are not isolated transaction or fsync
times.

| Run | ACK p50 | ACK p95 | ACK p99 | Second p50 | Second p95 | Second p99 |
| --- | ------: | ------: | ------: | ---------: | ---------: | ---------: |
| 1   |    3.26 |    4.61 |    5.67 |       4.75 |       6.48 |       7.69 |
| 2   |    3.94 |    5.73 |    8.60 |       5.78 |       8.29 |      11.26 |
| 3   |    3.85 |    5.55 |    7.05 |       5.64 |       7.88 |       9.99 |
| 4   |    3.91 |    5.58 |    7.97 |       5.75 |       7.88 |      11.26 |
| 5   |    4.21 |    7.86 |    9.88 |       6.09 |      11.11 |      14.26 |

The worst run's p95 is 7.86 ms to ACK and 11.11 ms to the second client. The
local targets of 50 ms and 100 ms are met for this narrow dataset and topology.
No result here establishes behavior for large books, cross-host networking,
multiple concurrent writers, slow readers or an S3-backed recovery path.
Actor queue wait, validation, DB checkout, transaction/commit duration and
broadcast enqueue still require separate instrumentation. Optimistic UI p95
and first-render p95 were not measured by this harness.
