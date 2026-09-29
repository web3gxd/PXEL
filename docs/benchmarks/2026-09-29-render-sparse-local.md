# Synthetic 100,000-cell Canvas render, 2026-09-29

`just bench-render-sparse` ran twice against the production Vite bundle and a
release server on loopback. Chromium 153.0.8010.12 ran on Windows 10.0.26100
with an Intel Core Ultra 9 185H. The harness created a real empty document,
then intercepted only its document GET with a synthetic projection containing
10,000 stable row IDs, 100 stable column IDs and 100,000 numeric cells (ten
per row, distributed across columns). The response body was 14,583,072 bytes.
The synthetic projection reuses the empty document's state hash; it is a
frontend render fixture, not a valid canonical document or durability test.

The production app parsed the response, populated its sparse client store and
drew the viewport on a 1280×496 Canvas in a 1280×720 browser at DPR 1. The
marker fires after `drawGrid` returns for that document. Each run measured one
first load and 20 reloads. Resource Timing's document GET `responseEnd` was
used as the data-readiness boundary. Values are milliseconds.

| Run | First navigation → grid | First data → grid | Reload p50 navigation | Reload p95 navigation | Reload p99 navigation | Reload p50 data | Reload p95 data | Reload p99 data | Max DOM nodes |
| --- | ----------------------: | ----------------: | --------------------: | --------------------: | --------------------: | --------------: | --------------: | --------------: | ------------: |
| 1   |                   528.4 |             260.6 |                 674.1 |                 749.7 |                 758.4 |           354.1 |           416.9 |           430.0 |           107 |
| 2   |                   543.1 |             266.6 |                 756.9 |                 820.7 |                1010.0 |           398.2 |           442.8 |           501.9 |           107 |

The worst local p95 after data readiness was 442.8 ms and the maximum DOM
count remained 107, so this fixture meets the initial <1 s first-grid target
and does not create a node per cell. One navigation in the second run exceeded
1 s (p99 1010.0 ms). This test does not measure persisted 100,000-cell replay,
JS heap/worker memory, actual screen presentation, cross-host transfer or
interaction latency under a large workbook. The fixed reference runner and
real large-document recovery benchmark remain open.
