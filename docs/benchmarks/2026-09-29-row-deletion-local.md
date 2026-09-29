# Local row deletion with 10,000 formulas, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook delete_target_10000_formulas -- --sample-size 10 --measurement-time 2`.

Fixture: 10,001 stable rows, one column, 10,000 sparse formulas bound to the first row. The timed operation deletes that target row. Each surviving formula must become a typed invalid reference; the candidate also removes the row from the axis, records its retired ID and validates workbook budgets. Fixture construction is outside the timed loop. This excludes PostgreSQL, event encoding, domain hash, server calculation, WebSocket and browser rendering.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM; Rust 1.98.1 `x86_64-pc-windows-gnu`, optimized bench profile. PostgreSQL and the dev server were running during the measurement. Criterion used a 3-second warm-up, a 2-second target measurement and 10 samples (165 timed iterations).

Result: **14.974–17.812 ms** Criterion 95% estimate; point estimate **15.916 ms**. One of ten samples was a high mild outlier. This is a preliminary local domain-only baseline, not p95 operation commit latency or a fixed-runner gate. The current transform scans every formula; its resource budget and correctness tests are documented in ADR-022.
