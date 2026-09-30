# Multiple criteria: preliminary bound evaluator measurement

Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1 release,
engine pxel-formula-0.14.0. Live browser/server/build verification ran concurrently;
CPU load uncontrolled. Criterion20samples,1s warmup,2s requested measurement.
Estimates below are not p95 or performance gates.

```sh
cargo bench -p spreadsheet-formula --bench scalar -- IFS_bound --sample-size 20 --warm-up-time 1 --measurement-time 2
```

Real domain workbook with100k stable rows/populated numeric ones. Two criteria,
">0" AND "<2", inspect A1:A100000. SUMIFS also sums that rectangle. Expected100000
is checked before timing. Fixture construction and parsing/binding are outside
timing; each iteration uses actual bound evaluator and a fresh budget. Graph
invalidation/scheduling, database/commit, network, WASM crossing and UI are excluded.

| Formula                | Estimate | Confidence interval |
| ---------------------- | -------- | ------------------- |
| COUNTIFS, two criteria | 36.439ms | 29.567–44.469ms     |
| SUMIFS, two criteria   | 44.664ms | 42.300–47.152ms     |

COUNTIFS retained four high outliers among20measurements. Mask intersection reads
every admitted criteria cell, including unmatched ones, to preserve typed resource
boundaries and deterministic traversal. SUMIFS additionally reads the sum range.
This numeric workload does not measure127criteria, wildcard complexity, shape
admission or errors. Repeat on a controlled runner with stage attribution before
optimizing or making relative throughput claims.
