# Single-cell inverse history: preliminary PostgreSQL sample

Windows10.0.26100, Intel Core Ultra9 185H,15.61GiB physical RAM,
Rust1.98.1 debug/test profile, PostgreSQL18.6 on local loopback/storage.
Development stack and verification builds were active; load was uncontrolled.
This is one wall-clock sample, not p95 or a controlled performance gate.

```sh
cargo test -p spreadsheet-storage cancelled_cell_history_scan_timing -- --ignored --nocapture
```

The real SQLx PostgreSQL fixture asserts synchronous_commit,fsync and
full_page_writes are all on. It commits one original numeric cell write, then500
own write/inverse pairs (1000tail events) to that same stable address. Each event
is committed durably, checksummed and applied to the real domain projection.
The final inverse of the original write scans and cancels the pairs under the
document transaction and commits a new version. Exact full replay hash equality
is asserted afterward using bounded1000-event pages.

Measured final `PgDatabase::commit` wall time: **41.066ms**. This includes journal
validation, pool/transaction work, verified history decoding/reduction, CAS,
event/receipt insertion and durable COMMIT. Original preparation,1000tail commits
and final replay are outside timing. Queue wait, network delivery, editor response,
WASM and separate lock-hold/fsync stage attribution are not measured.

The effective stack retains only operation IDs/versions, bounded by the existing
10k-version history window. This sample does not establish cost at that window's
limit, with large/formula-bearing payloads or concurrent writers. Those cases and
controlled release measurements remain a performance follow-up before replacing
the scan with an index or declaring a latency target passed.
