# Local column deletion with 10,000 formulas, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook delete_target_column_10000_formulas -- --sample-size 10 --measurement-time 2`.

Fixture: 10,000 stable rows, two columns, 10,000 sparse formulas in the second column with bound references to the first column. The timed operation deletes the referenced first column. Each surviving formula becomes a typed invalid reference. Fixture construction is outside the timed loop; the loop clones the book, changes the axis, transforms formula ASTs and validates the candidate. PostgreSQL, event encoding, domain hash, server calculation, WebSocket and browser rendering are excluded.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM; Rust 1.98.1 `x86_64-pc-windows-gnu`, optimized bench profile. PostgreSQL was running; the application server was stopped. Criterion used a 3-second warm-up, a 2-second target measurement and 10 samples (220 timed iterations).

Result: **12.241–13.377 ms** Criterion 95% estimate; point estimate **12.712 ms**. One of ten samples was a high mild outlier. This is a preliminary local domain-only baseline, not p95 operation commit latency or a fixed-runner gate. The formula scan is deliberate until a controlled benchmark and convergence tests justify another representation.
