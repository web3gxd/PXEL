# Atomic range inverse candidate

Measured2026-09-30 on Windows10.0.26100, Intel Core Ultra9 185H,
15.61GiB visible RAM, pinned Rust1.98.1. The developer machine also ran browser,
PostgreSQL and compilation checks; concurrent load was uncontrolled.

```sh
cargo bench -p spreadsheet-domain --features test-support --bench workbook -- candidate_1000_range_inverse --sample-size 10 --measurement-time 2 --warm-up-time 1
```

The optimized fixture starts with1000 stable rows and1000 authored numeric writes.
One compensation restores their exact blank preimages after validating the whole
private candidate. Fixture setup is outside the measurement; no PostgreSQL,
history scan, formula calculation, browser or network is included.

Criterion reported **654.36–693.61µs**, central estimate673.93µs,10samples,
1s warm-up and2s measurement. One mild high outlier was reported. This is an
interval for domain candidate timing, not p95 end-to-end undo/commit latency.
The result does not close controlled release latency or large formula-history
performance gates. PostgreSQL resource-limit tests establish rejection boundaries,
not latency benchmarks.
