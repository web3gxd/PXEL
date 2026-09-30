# Conditional aggregates: preliminary bound evaluator measurement

Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1 release,
engine pxel-formula-0.13.0. Browser and build verification ran concurrently;
CPU load uncontrolled. Criterion20samples,1s warmup,2s requested measurement,
automatically extended. These estimates are not p95 or performance gates.

Actual command:

```sh
cargo bench -p spreadsheet-formula --bench scalar -- IF_bound --sample-size 20 --warm-up-time 1 --measurement-time 2
```

Real domain workbook with100k stable rows and populated numeric ones. Formula
parsing/binding and fixture construction occur outside timing. Each iteration
runs the bound evaluator with a fresh budget; the expected100000 result is checked
before timing. Graph invalidation/scheduling, PostgreSQL, network, commit latency,
WASM crossing and rendering are excluded.

| Formula                   | Estimate | Confidence interval |
| ------------------------- | -------- | ------------------- |
| COUNTIF(A1:A100000,">0")  | 21.002ms | 19.216–22.414ms     |
| SUMIF(A1:A100000,">0")    | 15.171ms | 14.021–15.970ms     |
| SUMIF(A1:A100000,">0",A1) | 58.659ms | 55.030–62.881ms     |

COUNTIF/default SUMIF stream the source once. The explicit separate sum-range
form builds a bounded selection mask and traverses the actual anchored sum
rectangle, even when its cells overlap the criteria range. This benchmark uses
numeric predicates; it does not measure Unicode wildcard complexity or error
handling. Different concurrent load prevents relative speed claims. Repeat on
a controlled runner with stage attribution before deciding on optimizations.
