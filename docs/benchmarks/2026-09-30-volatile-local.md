# Explicit-epoch volatile evaluation: local observations

Windows 10.0.26100, Intel Core Ultra 9 185H, Rust 1.98.1 GNU release,
Chromium 153.0.8010.12, Node 24.18.0. Dev stack and browser regression suite
active during measurement; laptop load is uncontrolled. Engine 0.8.0.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- volatile_ --warm-up-time 1 --measurement-time 2 --sample-size 20
```

Parsed/bound AST is prepared once. Each iteration evaluates through the real
bound adapter with persisted context and an empty result cache; includes bound
validation/resolver overhead. Expected values checked before timing. No host
clock, mutable RNG, HTTP or PostgreSQL is involved in timed work.

| Function | Criterion interval | Central estimate |
| -------- | ------------------ | ---------------- |
| TODAY    | 2.6113–3.0059 µs   | 2.8438 µs        |
| NOW      | 2.8109–3.2417 µs   | 3.0024 µs        |
| RAND     | 3.4566–4.8395 µs   | 4.0662 µs        |

20 samples, 1 s warmup, 2 s measurement per case. RAND has two outliers
(one mild, one severe). This is initial evidence, not a stable performance
threshold, regression comparison or frontend/commit p95.

`node tests/parity/workbook.mjs` passed 1010 books/2020 patches, including
seven volatile expected cases: time/day, independent SHA-256 RAND golden,
incremental dependent preview with unchanged epoch, multiple call nodes,
lazy branch and arity. Native/browser values and normalized hashes matched.
Input 1625854 bytes, one WASM load, Worker compute 206.3 ms, total roundtrip
2147.7291 ms. Aggregate observations include initialization and mixed formulas;
do not divide these into a claimed per-edit latency.

This slice measures explicit epochs only. Automatic open/timer/dependency
trigger cost, whole-document volatile workloads and manual Excel verification
remain open gates. No speed comparison with Google Sheets was performed.
