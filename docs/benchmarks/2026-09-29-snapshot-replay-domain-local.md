# Domain replay from snapshot with a 10,000-event tail, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-domain --features test-support --bench recovery -- --sample-size 10 --warm-up-time 1 --measurement-time 3`.

Host: Windows 11 Pro build 26100, Intel Core Ultra 9 185H (16 cores, 22 logical processors), 15.6 GiB RAM. Rust/Cargo 1.98.1, optimized bench profile, no development server running. One local run, Criterion warm-up 1 s and 10 samples per case. PostgreSQL, BlobStore, protobuf decoding, network and browser are **not** in these timings.

Dataset: one sheet, one stable row and column, 20,000 consecutive numeric `SetCellValue` events to that cell after genesis v1. Snapshot is taken after the first 10,000 edits at v10001. The timed snapshot case restores `SnapshotImage` and applies the remaining 10,000 canonical events; the comparison case applies genesis and all 20,000 edits from an empty workbook. Fixture setup and event generation are outside measurement. Before timing, the harness verifies both paths reach the same final domain hash and every event's expected hash is checked by `apply_event`.

| Case                                | Criterion 95% estimate |
| ----------------------------------- | ---------------------: |
| Snapshot image + 10,000 tail events |       5.7527–8.6369 ms |
| Full replay of 20,001 events        |       17.024–20.579 ms |

This is a small sparse document and a domain-only replay measurement. It does not establish PostgreSQL snapshot+tail restore latency or a p95 user-facing opening time. A subsequent benchmark must include verified BlobStore read, binary decoding, PostgreSQL pages and a larger sparse projection before tuning the scheduler or claiming the recovery target for the full system.
