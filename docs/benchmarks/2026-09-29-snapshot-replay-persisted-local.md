# Persisted recovery: snapshot + 10 000 events, 2026-09-29

Command: `just bench-recovery-storage` (twice). `DATABASE_URL` must allow
temporary test-database creation. The ignored SQLx test creates an isolated
PostgreSQL database, migrates it, checks readiness, commits genesis and
20 000 `SetCellValue` events through the normal journal API, and publishes
one verified LocalFilesystemBlobStore snapshot after edit 10 000 (document
version 10 001). The remaining 10 000 events form the replay tail. Fixture
creation and snapshot publication are outside the timed section. The test
checks `snapshot_version = 10001`, `replayed_events = 10000`, and equality of
both paths with the committed domain hash on every sample.

Machine: Windows 11 Pro build 26100, Intel Core Ultra 9 185H (16 cores,
22 logical processors), 15.6 GiB RAM, Rust 1.98.1, PostgreSQL 18.6 on
`127.0.0.1`, local temporary BlobStore. Release profile uses thin LTO and
one codegen unit. `PgDatabase::readiness` confirmed `fsync=on`,
`full_page_writes=on`, `synchronous_commit=on`, and exact migrations. No
browser or application server was running.

Each run warmed both paths twice, then alternated their order for 20 samples
per path. Percentiles are nearest-rank values from those 20 samples, not
confidence intervals or service SLOs. Timed snapshot recovery includes ACL
lookup, PostgreSQL snapshot metadata, full object read and checksum/codec
verification, 10 journal pages, decoding and domain replay. Timed full replay
starts with an empty workbook and reads 21 journal pages through the same
authenticated journal API; it does not perform a snapshot metadata lookup.
The document has one sheet, one stable row and column, and one repeatedly
updated numeric cell. This is a warm-cache sparse fixture, not a large-book,
cold-start, process-restart or network-client benchmark.

| Run | 20k commits + snapshot | Snapshot + tail p50 | Snapshot + tail p95 | Full replay p50 | Full replay p95 |
| --- | ---------------------: | ------------------: | ------------------: | --------------: | --------------: |
| 1   |               20.477 s |           49.795 ms |           87.611 ms |      107.578 ms |      139.006 ms |
| 2   |               21.167 s |           50.739 ms |           71.356 ms |      102.740 ms |      131.171 ms |

Observed min/max: snapshot 44.160–96.892 ms in run 1 and
44.275–80.682 ms in run 2; full replay 85.024–158.912 ms in run 1 and
71.075–164.789 ms in run 2. Both runs passed. The difference is useful for
this fixture but does not justify changing the snapshot interval without a
larger-document and cold-restart corpus. Commit time during fixture setup is
not a per-operation latency measurement.
