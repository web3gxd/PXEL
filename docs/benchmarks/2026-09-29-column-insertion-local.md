# Local column insertion with 10,000 formulas, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook insert_front_column_10000_formulas -- --sample-size 10 --measurement-time 2`.

Fixture: 10,001 stable rows, one column and 10,000 sparse formulas referencing the first row in that column. One new column is inserted before the existing column, so both the formulas and their targets shift in displayed A1 space while stable IDs remain unchanged. The timed path clones the workbook, inserts the column ID, traverses/rebases formula ASTs and validates the candidate. Fixture construction is outside the timed loop. PostgreSQL, event encoding, domain hash, calculation, WebSocket and browser rendering are excluded.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM; Rust 1.98.1 `x86_64-pc-windows-gnu`, optimized bench profile. PostgreSQL was running; the application server was stopped. Criterion used a 3-second warm-up, a 2-second target measurement and 10 samples (220 timed iterations).

Result: **11.853–14.140 ms** Criterion 95% estimate; point estimate **12.883 ms**. One of ten samples was a high mild outlier. This is a preliminary local domain-only baseline, not p95 commit latency. The O(number of formulas) transform stays until a controlled benchmark and convergence tests justify another representation.
