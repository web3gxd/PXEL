# Local formula row-insertion baseline, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook -- insert_middle_10000_formulas --warm-up-time 3 --measurement-time 10 --sample-size 10`.

The fixture has 10,001 stable rows, one column and 10,000 sparse formulas, each with a bound relative reference to the first row. A single new row is inserted in the middle, so about half the formulas need a different relative row delta. The timed path clones the workbook, inserts the ID, rebases formula AST offsets and validates the resulting candidate. Fixture construction is outside the timed loop. This excludes PostgreSQL, event encoding, domain hash, server calculation, WebSocket and browser rendering.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), about 15.6 GiB RAM; Rust 1.98.1 `x86_64-pc-windows-gnu`, optimized bench profile. Criterion used a 3 s warm-up, 10 s target measurement and 10 samples (825 timed iterations). The database was running; no application server was running during this benchmark.

Result: **12.078–13.443 ms** Criterion 95% estimate; point estimate **12.681 ms**. Three of ten samples were marked mild outliers. This is a preliminary local baseline, not p95 end-to-end commit latency or a fixed-runner gate. The current O(number of formulas) rebase is a deliberate correctness-first implementation. A change to the representation requires a controlled A/B benchmark and convergence tests.
