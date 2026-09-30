# Table lookup: preliminary native bound evaluator measurement

Windows10.0.26100, Intel Core Ultra9 185H, pinned Rust1.98.1 release,
engine pxel-formula-0.11.0. Local browser verification and compilation were
running concurrently; CPU load was uncontrolled. Criterion20 samples,1s warmup,
2s requested measurement (automatically extended for the larger cases). These
are estimates with confidence intervals, not p95 or acceptance thresholds.

```sh
cargo bench -p spreadsheet-formula --bench scalar -- LOOKUP_bound_table --sample-size 20 --warm-up-time 1 --measurement-time 2
```

Both real domain fixtures contain100,000 populated cells and stable axes:
VLOOKUP50,000rows x2columns, HLOOKUP100rows x1000columns. Key axis consists of
numeric ones, return cells of twos. Exact picks the first duplicate; approximate
picks the last. Both traverse/validate the complete key axis and resolve one
return cell; whole table admission is bounded. Expected2 is checked before timing.
Fixture construction, parsing/binding, dependency graph, WASM crossing, database,
commit, network and rendering are excluded.

| Case                          | Estimate | Confidence interval |
| ----------------------------- | -------- | ------------------- |
| VLOOKUP50k keys, exact        | 16.642ms | 16.030–17.544ms     |
| VLOOKUP50k keys, approximate  | 13.759ms | 13.020–14.510ms     |
| HLOOKUP1000 keys, exact       | 145.99µs | 135.24–161.33µs     |
| HLOOKUP1000 keys, approximate | 114.76µs | 111.53–118.39µs     |

Initial harness failed because the horizontal fixture requested10k columns,
exceeding the domain's1000-column cap. Corrected fixture keeps100k cells within
existing production limits. The incomplete first run is not the reported result.
Criterion's comparison against that uncontrolled earlier run printed a regression;
different concurrent load prevents attributing it to code performance. No kernel
optimization or universal latency claim follows. Repeat on a controlled runner
with stage attribution before making regression decisions. Wildcard worst-case
cost and full graph recalculation are separate measurements.
