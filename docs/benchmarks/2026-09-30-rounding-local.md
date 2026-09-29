# ROUND family: local scalar evaluator measurement

Run across 2026-09-29/30 Moscow time, Windows 10.0.26100, Intel Core Ultra 9
185H, Rust 1.98.1 GNU, release build. Ordinary laptop with the dev stack active;
compilation overlapped parity before timing. This was not a dedicated quiet runner.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- 'ROUND.*_scalar' --sample-size 10 --warm-up-time 1 --measurement-time 2
```

| Fixture                | Criterion estimate interval | Central estimate |
| ---------------------- | --------------------------- | ---------------- |
| ROUND(1.005,2)         | 258.12–390.84 ns            | 318.26 ns        |
| ROUNDUP(-12.34567,3)   | 433.21–499.67 ns            | 461.54 ns        |
| ROUNDDOWN(-12.34567,3) | 475.91–524.63 ns            | 494.32 ns        |

ASTs are parsed and expected results checked before timing. Each iteration
creates the regular evaluator budget. ROUND had two high outliers, ROUNDDOWN
four mixed outliers among ten samples. These are Criterion estimate intervals,
not p95 editing, commit or rendering latency. Parsing, PostgreSQL, scheduling,
network and the WASM crossing are excluded. No universal threshold is established.

Final shared browser corpus: 202 cases, 11.7 ms initialization, 17.8 ms evaluation,
108.0924 ms aggregate roundtrip, 890672 WASM bytes. Stateful fixture generator
included all three new functions: 1001 workbooks, 2002 patches, 193.7 ms aggregate
Worker compute and 1807.9725 ms aggregate roundtrip. Both harnesses confirmed
native/WASM normalized hash parity; these aggregate timings are not per-edit p95.
Chromium 153.0.8010.12, Node 24.18.0. Frontend checks ran concurrently with the
final parity run, so these observations are not isolated performance baselines.
