# Text functions: local evaluator observations

2026-09-30 Moscow time, Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1 GNU,
release build. Ordinary laptop with dev services and frontend checks during the
broader run; not an isolated reference runner. Criterion10samples/1s warmup/2s.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- 'TEXT_scalar|CONCAT_range_10k' --sample-size 10 --warm-up-time 1 --measurement-time 2
```

| Case                                           | Estimate interval | Central estimate |
| ---------------------------------------------- | ----------------- | ---------------- |
| Composition of all eight scalar text functions | 920.04–959.30 ns  | 937.73 ns        |
| CONCAT(A1:A10000), resolver yields numeric1    | 1.3717–1.6762 ms  | 1.4888 ms        |

ASTs are parsed outside timing, and expected scalar Unicode/result10000ones
checked before measurement. Regular evaluator budgets are included. The range
resolver is a pure benchmark input, not mocked application persistence. These
are Criterion estimate intervals; no p95 commit/edit/rendering claim. Parsing,
dependency scheduler, PostgreSQL, network and WASM boundary excluded.

Shared native/browser corpus284cases passed expected/output hashes and missing-
WASM failure. Aggregate init10.1ms/eval14.1ms/roundtrip99.2046ms; WASM906973bytes.
Stateful1001books/2002patches includes all eight text functions: aggregate Worker
compute169.1ms/roundtrip1420.0893ms, one WASM load. Chromium153.0.8010.12,
Node24.18.0. These aggregate harness timings are not a per-edit latency baseline.
Manual Excel execution, locale/legacy Unicode equivalence and cross-platform
coverage beyond this native Windows/WASM pair are not claimed.
