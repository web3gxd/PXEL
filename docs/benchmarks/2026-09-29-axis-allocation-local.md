# Local axis allocation, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook axis_allocation -- --sample-size 10`. Existing Criterion harness builds one `SheetSeed` with 1,000 or 100,000 stable row IDs and one column, then creates the `Workbook` candidate at version 1. It does not measure persisted snapshots, browser rendering, or memory footprint.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM. Optimized Rust bench profile. Criterion used 3 s warm-up and 10 samples for each size. No database is used by the fixture. One run had a development server process present; it was stopped before the second run.

| Run | Server process | 1,000 rows, 95% estimate | 100,000 rows, 95% estimate |
| --- | -------------- | ------------------------ | -------------------------- |
| 1   | Running, idle  | 30.625–33.905 µs         | 7.1798–7.9022 ms           |
| 2   | Stopped        | 32.594–34.945 µs         | 6.7701–7.5430 ms           |

Criterion marked run 1 as 75–106% slower than an older local baseline for 100,000 rows. Run 2 was statistically indistinguishable from run 1 (`p=0.10` for 100,000 rows). The old baseline was recorded under different, undocumented host load and earlier code; these two runs cannot attribute the difference to `ExtendRows`. No optimization is proposed from this evidence. A controlled A/B benchmark with the same machine state and a memory measurement is required before changing the axis representation.
