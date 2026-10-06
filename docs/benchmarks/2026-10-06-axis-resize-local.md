# Local axis resize response, 2026-10-06

The previous grid waited for the server's committed size before drawing. The
fixed grid draws a separate local preview while dragging, retains pending
geometry after release, and rolls it back on terminal rejection. A browser test
with held socket sends failed on the old code after five seconds and passes
with the preview layer.

## Reproducible measurement

```sh
PXEL_BENCH_WEB=http://127.0.0.1:5173 node tests/performance/resize.mjs
```

Windows 11, Intel Core Ultra 9 185H, Chromium 153.0.8010.12. Vite development
frontend, existing dev-auth Rust executable, real isolated PostgreSQL 18.6 on
loopback. One empty durable workbook with 1000 allocated rows and 26 columns;
column A alternates between 112 and 172 CSS pixels. Two warmups followed by 20
samples for each version. No latency throttling or mocked persistence.

The probe starts at the resize pointermove and stops when Canvas submits the
matching active-cell selection border. This includes client/server waiting in
the original implementation and local drawing in the fixed implementation.
It measures draw submission, not GPU presentation or database durability time.

| Metric  | Original | Final fix |
| ------- | -------: | --------: |
| Median  |  50.3 ms |   19.4 ms |
| p95     |  82.8 ms |   36.9 ms |
| Maximum |  83.1 ms |   39.3 ms |

Raw final measurements are retained locally in ignored
`.local/resize-before.json` and `.local/resize-after.json`. The same script ran
against the original HEAD sources and the final sources, with source restoration
in a `finally` block. The final version synchronizes React overlay geometry with
Canvas painting. An earlier exploratory comparison, before that synchronization,
reported 467.1/19.9 ms p95 and 784.4/21.0 ms maximum; it is not the final-code
measurement. The initial budget-template profile measured 207–293 ms between
mouse release and the save-status update, which is a different metric.

## Limits

Developer-machine load is uncontrolled; these are observations, not a production
SLA. The test uses an empty book, not 100,000 populated cells. A separate unit
regression checks resize lookup at 100,000 allocated rows: the preview shares
prefix storage and needs at most 18 prefix lookups for hit testing. Functional
browser checks cover both axes, withheld confirmation, pointer cancellation,
pending hit testing, reload and another subscribed tab at DPR 1 and 2.

The server still validates candidate state and commits to PostgreSQL before ACK
or publication. Local preview does not acknowledge or alter confirmed state.
Production, Linux/Compose and remote CI gates remain open.
