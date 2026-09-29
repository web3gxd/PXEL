# Local document Canvas render, 2026-09-29

`just bench-render` ran against the release `pxel-server` with the production
Vite bundle served on `127.0.0.1:8080`. Chromium 153.0.8010.12 and the server
ran on one Windows 10.0.26100 machine (Intel Core Ultra 9 185H, 22 logical
CPUs); PostgreSQL 18.6 was local. The browser opened one newly created empty
workbook with 1,000 row and 26 column IDs and no materialized cells. Viewport
was Playwright's default 1280×720 at DPR 1; the Canvas backing store was
1280×496. This is a local observation, not a production SLA.

The marker `pxel:document-grid-ready` fires after `drawGrid` returns for the
first frame that has the fetched document projection. The harness checks that
the Canvas bears that document ID and that the document GET has a Resource
Timing entry. It measures from navigation start to the marker and from the
GET's `responseEnd` to the marker. One first load and 20 same-context reloads
were measured. No mocked renderer or persistence was used.

| Metric                        | First load | Reload p50 | Reload p95 | Reload p99 |
| ----------------------------- | ---------: | ---------: | ---------: | ---------: |
| Navigation to document grid   |   122.1 ms |    60.4 ms |    64.4 ms |    64.9 ms |
| Document response end to grid |    13.1 ms |    15.1 ms |    18.8 ms |    19.5 ms |

The maximum observed DOM count was 107. The local target of p95 <1 s after
data readiness was met for this empty workbook. The sample size is only 20
reloads, so this is a preliminary baseline. It does not measure input-frame
latency, 100,000 populated cells, networked clients, or a cold browser profile
for each run. Repeat on the fixed reference runner with a populated sparse
workbook before closing the broader performance gate.
