# Local combined collaboration stress sample

Date: 2026-10-02. Host: Windows 11 build 26100, Intel Core Ultra 9 185H,
22 logical CPUs. Server: local debug `pxel-server` with dev-auth. Database:
fresh isolated PostgreSQL 18.6 with `synchronous_commit=on`, `fsync=on`, and
`full_page_writes=on`. Client/server/database used loopback; this sample has no
network delay injection or CPU isolation.

Command, with the isolated app and PostgreSQL started by a local test runner:

```powershell
corepack pnpm exec node --test --test-concurrency=1 tests/performance/combined-stress.test.mjs
```

The scenario committed 600 sequential 30,005-byte updates to one hot cell while
two HTTP readers made 120 total document reads and a third subscribed socket was
paused. A second active socket received every durable head notification. Every
ACK/version and notification matched through version 601; the stalled reader was
disconnected on queue overflow without blocking the writer.

| Measure                       |      p50 |      p95 |       max |
| ----------------------------- | -------: | -------: | --------: |
| HTTP document read            | 21.75 ms | 40.63 ms | 181.12 ms |
| Durable ACK                   | 21.54 ms | 35.49 ms | 161.46 ms |
| Second-client head visibility | 29.14 ms | 44.60 ms | 180.68 ms |

The test passed in 37.82 s (38.03 s test process duration). These are local
debug/loopback stress values, not production SLOs. The run provides combined
database read/write and websocket backpressure coverage; formula-heavy,
multi-document, multi-host/network-impairment, release load, Linux and
production-like combined stress remain open.
