# ABS and MOD: local scalar evaluator measurement

2026-09-29, Windows 10.0.26100, Intel Core Ultra 9 185H, Rust 1.98.1 GNU,
release build. Ordinary laptop with the dev stack/build activity; no dedicated
quiet runner. Criterion 10 samples, one-second warm-up, two-second measurement.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- 'ABS_scalar|MOD_scalar' --sample-size 10 --warm-up-time 1 --measurement-time 2
```

| Fixture       | Criterion estimate interval | Central estimate |
| ------------- | --------------------------- | ---------------- |
| ABS(-123.5)   | 50.368–85.295 ns            | 73.321 ns        |
| MOD(-123.5,7) | 109.65–147.27 ns            | 131.64 ns        |

ASTs are parsed before timing and results are validated before the loop. Each
iteration creates the normal budget and evaluates the same scalar AST. MOD had
one high mild outlier among ten measurements. These intervals are Criterion
estimates, not p95 cell-edit or commit latency. Parsing, dependency scheduling,
PostgreSQL, WASM crossing, network and rendering are excluded. No universal
performance threshold or Excel precision parity is established by this run.
