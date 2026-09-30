# Atomic batch candidate after final-capacity validation

Measured2026-09-30 on Windows10.0.26100, Intel Core Ultra9 185H,
16,373,108KiB visible RAM (15.61GiB). The local development stack and other
checks were active; this was an uncontrolled developer-machine measurement.

Command:

```sh
cargo bench -p spreadsheet-domain --features test-support --bench workbook -- candidate_1000_value_batch --sample-size 10 --measurement-time 2 --warm-up-time 1
```

Criterion's optimized1000-value batch candidate interval was
**519.93–652.33µs**, central estimate599.42µs,10samples,1s warm-up,
2s measurement. One mild high outlier was reported. The fixture uses an empty
1000-row sheet and numeric writes; this measures domain candidate preparation
without PostgreSQL, network, calculation or rendering. It is not p95 commit.
Criterion reported no statistically significant change against its retained
local sample (p=0.08); differing concurrent loads preclude a controlled regression
claim. Capacity regression fixtures separately cover saturated text, formula AST
and100,000 populated cells; their test durations are not performance benchmarks.

Controlled release measurements for range compensation, large preimages,
transaction history scans and end-to-end ACK remain open.
