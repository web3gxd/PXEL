# INDEX native bound evaluator: preliminary local measurement

Windows 10.0.26100, Intel Core Ultra 9 185H, pinned Rust 1.98.1,
release build, engine pxel-formula-0.9.0. Criterion: 1 second warmup,
2 seconds measurement, 20 samples. Local dev and other verification processes
were running: laptop load was uncontrolled. These observations are not latency
acceptance evidence or CI regression thresholds.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- INDEX_bound --warm-up-time 1 --measurement-time 2 --sample-size 20
```

The fixture contains 100,000 stable rows and populated numeric cells. Both
formulas are bound once and their expected results checked before measurement.
Each iteration uses the real bound evaluator and a fresh default budget.
Fixture construction, parsing/binding, projection graph scheduling, WASM crossing,
PostgreSQL, commit, network and rendering are excluded. No persistence is involved.

| Formula                  | Criterion estimate | Reported confidence interval |
| ------------------------ | ------------------ | ---------------------------- |
| INDEX(A1:A100000,100000) | 13.563 µs          | 12.064–15.827 µs             |
| SUM(INDEX(A1:A100000,0)) | 26.920 ms          | 26.119–27.710 ms             |

Selection visits one target despite the large source rectangle; the aggregate
visits all selected cells in fixed order. Reported estimates are not p95 samples.
Repeat on an idle controlled runner before using these numbers to choose an
optimization. Criterion artifacts remain under ignored target/criterion.
