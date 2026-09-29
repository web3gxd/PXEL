# Local atomic value batch, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook candidate_1000_value_batch -- --sample-size 10 --measurement-time 1`.

Fixture: one sheet with 1,000 stable rows and one stable column, no cells. The timed operation validates and applies 1,000 numeric values in one candidate version. Fixture construction is outside the timed loop. PostgreSQL commit, protobuf encoding, server calculation, WebSocket and browser painting are excluded.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM; Rust 1.98.1 `x86_64-pc-windows-gnu`, optimized bench profile. Criterion used a 3-second warm-up, 1-second target measurement and 10 samples (2,200 timed iterations).

Result: **567.59–631.31 µs** Criterion 95% estimate; point estimate **604.84 µs**. This is a preliminary local domain-only baseline, not p95 commit latency or the 16 ms optimistic UI target.
