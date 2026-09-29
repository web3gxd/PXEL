# Active timer classification: local observations

Windows 10.0.26100, Intel Core Ultra 9 185H, Rust 1.98.1 GNU release.
Local PostgreSQL/backend/frontend remained running; laptop load uncontrolled.
Production implementation is commit c63c075. Benchmark changes only test code.

```sh
cargo test --locked --release -p spreadsheet-collab timer_classification_benchmark -- --ignored --nocapture
```

One sheet, one column, 1k/10k/100k stable rows and exactly that many populated
cells. Static fixture contains scalar numbers; one_now replaces one number with
a bound NOW formula and persisted calculation context. Fixtures and correctness
checks are outside timing. Each sample measures one real clock_consumers scan,
including Instant overhead, with black_box. 100 warmups are discarded; the next
1000 samples are sorted. p50/p95 use sample indices499/949; no confidence interval.

| Cells  | Profile | p50 µs   | p95 µs   | Maximum µs |
| ------ | ------- | -------- | -------- | ---------- |
| 1000   | static  | 1.500    | 1.500    | 5.900      |
| 1000   | one_now | 1.600    | 1.700    | 8.100      |
| 10000  | static  | 15.200   | 16.300   | 93.500     |
| 10000  | one_now | 15.200   | 31.900   | 148.500    |
| 100000 | static  | 2182.200 | 3475.800 | 5528.900   |
| 100000 | one_now | 2249.400 | 3365.500 | 5933.200   |

The release harness passed1/1 in5.26s after12.82s compilation. Repeated scans
are cache-warm; the 100k case has a substantially larger working set. This does
not prove which hardware bottleneck dominates. It does demonstrate measurable
work proportional to populated cells even for a static book.

This is not heartbeat throughput, queue wait, timer commit, recalculation or
end-to-end p95. Timer admission can invoke classification more than once;
the table does not measure a whole request. Formula-heavy ASTs, multiple sheets,
cold recovery and many subscribers require separate workloads. No hard CI
threshold, Google Sheets comparison or claimed cache optimization follows.

Next profiling gate: measure authorized actor admission with real PostgreSQL,
vary subscriber count and formula AST density, then consider version-keyed
classification reuse with invalidation tests if the repeated scans dominate.
