# Local optimistic Canvas latency, 2026-09-29

`just bench-optimistic` ran five times with the production Vite bundle in
Chromium 153.0.8010.12 against a release server on the same Windows
10.0.26100 / Intel Core Ultra 9 185H host. Each run created a fresh empty
1,000×26-axis document and made 20 warm-up and 100 measured numeric edits to
A1 through the formula bar. The browser viewport was 1280×720 at DPR 1.

The harness installs a browser-only probe before navigation. After the initial
subscription it withholds WebSocket sends, then timestamps Enter's capture
event and the matching `CanvasRenderingContext2D.fillText` call. Each draw
uses a unique numeric value. The authoritative document GET remained at
version 1 after all samples, proving that these draws came from the pending
overlay rather than a server confirmation. This measures Canvas draw
submission, not GPU presentation. It does not include the Playwright command
round trip or network latency.

| Run |    p50 |     p95 |     p99 |     Max |
| --- | -----: | ------: | ------: | ------: |
| 1   | 6.9 ms |  8.8 ms |  9.1 ms |  9.4 ms |
| 2   | 6.8 ms |  9.1 ms |  9.3 ms |  9.7 ms |
| 3   | 6.7 ms |  8.8 ms |  9.7 ms | 15.1 ms |
| 4   | 5.5 ms | 14.9 ms | 15.7 ms | 15.7 ms |
| 5   | 6.3 ms |  8.6 ms | 12.6 ms | 13.0 ms |

The worst run's p95 was 14.9 ms, under the local 16 ms target for this narrow
dataset. The result does not establish behavior for populated sparse sheets,
formula-heavy documents, slower hardware, real compositor presentation, or
multiple tabs. A fixed runner and larger workload are required before treating
this as a product performance gate.
