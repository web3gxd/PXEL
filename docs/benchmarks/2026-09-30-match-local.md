# MATCH native bound evaluator: preliminary local measurement

Windows10.0.26100, Intel Core Ultra9 185H, pinned Rust1.98.1 release,
engine pxel-formula-0.10.0. Local server/browser verification and build processes
were running; CPU load was uncontrolled. Criterion:1s warmup,2s measurement,
20samples. These numbers are neither p95 nor acceptance/regression thresholds.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- MATCH_bound --warm-up-time 1 --measurement-time 2 --sample-size 20
```

Real domain fixture:100,000 stable rows and populated numeric cells, all equal
to1. Parsing/binding, fixture construction, graph scheduling, WASM crossing,
PostgreSQL, commit, network and rendering are excluded. Each iteration executes
the actual bound evaluator with fresh budget; expected results1 and100000 are
checked before timing. Both cases traverse and validate the complete source.
The approximate case also validates ascending order and duplicate position.

| Case                                              | Criterion estimate | Confidence interval |
| ------------------------------------------------- | ------------------ | ------------------- |
| MATCH(1,A1:A100000,0), first exact hit            | 42.116ms           | 36.551–47.998ms     |
| MATCH(1,A1:A100000,1), last approximate duplicate | 24.918ms           | 24.323–25.514ms     |

The timing difference is not evidence of intrinsic algorithm ordering: concurrent
load differs during the sequential cases. Do not optimize or assert relative
speed from this run; repeat with stage attribution on an idle controlled runner.
Wildcard comparisons consume the same work budget and have property/resource
tests; this numeric range benchmark does not measure worst-case wildcard cost.
