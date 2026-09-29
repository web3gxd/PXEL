# Calendar functions: local evaluator observations

2026-09-30 Moscow time, Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1
GNU release build, Criterion10samples/1s warmup/2s measurement. Dev stack and
parity/browser checks were active during the broader run; not isolated hardware.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- DATE --sample-size 10 --warm-up-time 1 --measurement-time 2
```

| Case                                               | Estimate interval | Central estimate |
| -------------------------------------------------- | ----------------- | ---------------- |
| DATE(2000,1,1)                                     | 102.59–105.93 ns  | 104.49 ns        |
| YEAR/MONTH/DAY numeric composition for serial36585 | 388.01–411.78 ns  | 402.34 ns        |

Expected scalar results are checked before measurement; AST parsing is excluded,
regular evaluator work budgets included. DATE_scalar had one low severe outlier
among ten samples. These are Criterion estimate intervals, not p95 latency.
No PostgreSQL, scheduler, network, parsing or WASM-boundary cost included.

Shared corpus337 expected/native/browser cases passed, including both calendar
systems and missing-WASM failure. Aggregate init24.5ms/eval14.6ms/roundtrip
113.0634ms; WASM914441bytes. Stateful1001books/2002patches includes all four
calendar functions: aggregate compute391.6ms/roundtrip2472.4372ms, one WASM load.
Chromium153.0.8010.12, Node24.18.0. These aggregate observations are not per-edit
thresholds. Manual Excel execution and other native platforms are unverified.
