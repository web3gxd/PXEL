# Local row insertion baseline, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench workbook insert_front_99999_rows -- --sample-size 10`.

The Criterion harness creates a formula-free workbook containing 99,999 allocated stable row IDs and one column outside the timed loop. The timed operation creates a private candidate with one new row inserted before the first stable row ID, rebuilding the ID→position index. It does not calculate formulas, hash the result, commit to PostgreSQL, deliver a WebSocket patch, render the browser, or measure memory. The operation is checked by unit, property-based, integration and browser tests; this benchmark only measures one domain path.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), approximately 15.6 GiB RAM. Optimized Rust bench profile. Criterion used 3 s warm-up and 10 samples (935 timed iterations total). A development server process was present but idle; this is a local preliminary observation, not a fixed-runner performance gate.

Result: **6.6972–7.2624 ms** (Criterion 95% estimate; point estimate 6.9241 ms). No optimization follows from this single run. A controlled baseline with snapshots and real commits is needed before changing the simple vector/index representation.
