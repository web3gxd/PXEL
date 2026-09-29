# Calculation context transport: differential harness observations

Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1 GNU release WASM/native,
Chromium153.0.8010.12, Node24.18.0. Dev stack active; not an isolated runner.

```sh
node tests/parity/workbook.mjs
```

1003 books/2006 patches passed normalized result parity, including schema3
with complete persisted context for one and two sheets. The new adapter is
929513bytes WASM, compared with914441bytes before this context slice.
Aggregate Worker compute203.4ms and roundtrip1504.44ms for1613285input bytes,
one WASM load. These are harness observations, not per-command p95 or a claim
of speed improvement. Workload was expanded and laptop load is uncontrolled.

Context validation precedes axis/formula hydration. Ordinary scalar patches
carry no repeated context or whole workbook; durable epoch control currently
requests an authorized projection resync and Worker reset. This run does not
measure volatile evaluation, timer scheduling, context-only patch transfer,
PostgreSQL commit, or UI latency. Those remain separate checks.
