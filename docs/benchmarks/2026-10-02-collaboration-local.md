# Local durable ACK and collaboration visibility benchmark

Date: 2026-10-02. Windows 11 build 26100, Intel Core Ultra 9 185H, 22 logical
CPUs, 15.61 GiB available RAM. Node 24.18.0, pinned PostgreSQL 18.6, optimized
Rust release build with `dev-auth`, loopback client/server/database. The isolated
database used `synchronous_commit=on`, `fsync=on`, and `full_page_writes=on`.
There was no Docker/WSL, network hop, CPU isolation, or competing benchmark load.

Command after building the release server and launching an isolated development
server against fresh PostgreSQL:

```powershell
$env:PXEL_BENCH_SERVER = 'http://127.0.0.1:<isolated-port>'
$env:PXEL_BENCH_SERVER_PROFILE = 'release/dev-auth, isolated PostgreSQL 18.6, loopback'
$env:PXEL_BENCH_WARMUP = '100'
$env:PXEL_BENCH_SAMPLES = '1000'
$env:PXEL_BENCH_RUNS = '5'
corepack pnpm exec node --test --test-concurrency=1 tests/performance/collaboration.test.mjs
```

Each run creates a document, subscribes a second client, performs 100 warm-up
writes and 1,000 measured sequential writes to the same cell, and waits for both
the durable ACK and the second client's head notification. It asserts committed
versions and ordered head versions for all 5,500 writes. Results below show
milliseconds per run; each percentile is calculated over that run's 1,000
measured samples.

|                       Run | ACK p50 | ACK p95 | ACK p99 | Second client p50 | Second client p95 | Second client p99 |
| ------------------------: | ------: | ------: | ------: | ----------------: | ----------------: | ----------------: |
|                         1 |   7.141 |   9.886 |  11.787 |            10.225 |            13.736 |            15.900 |
|                         2 |   7.310 |  11.030 |  13.437 |            10.376 |            15.669 |            18.792 |
|                         3 |   7.072 |  10.077 |  11.710 |            10.107 |            14.033 |            16.736 |
|                         4 |   7.308 |  10.732 |  12.969 |            10.518 |            15.071 |            18.116 |
|                         5 |   6.603 |   9.262 |  11.300 |             9.419 |            12.718 |            15.630 |
| Median of run percentiles |   7.141 |  10.077 |  11.787 |            10.225 |            14.033 |            16.736 |

The entire prescribed run passed in 58.3 s. These are loopback debug-auth
measurements with one scalar hot cell; they do not include formula evaluation,
browser optimistic-preview timing, internet latency, heterogeneous documents,
multi-host deployment, or production contention. The median-of-run p95 is not a
pooled p95 across 5,000 samples. This result informs the local functional and
latency baseline, but is not a controlled production SLA or a full load gate.
