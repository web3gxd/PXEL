# PostgreSQL timer actor admission: local observations

Windows10.0.26100, Intel Core Ultra9 185H, Rust1.98.1 GNU release.
Native local PostgreSQL18.6 on loopback, SQLx test-provided pool and isolated
test database. Local backend/frontend remained running; laptop load uncontrolled.
Production implementation610ec03. No persistence mock or injected projection.
Queried and asserted: synchronous_commit=on, fsync=on, full_page_writes=on.

```sh
cargo test --locked --release -p spreadsheet-storage timer_admission_benchmark -- --ignored --nocapture
```

Two independently persisted one-sheet books, each1000 rows/one column and1000
populated cells. one_now has999 scalar values and oneNOW;1000_formulas has1000
bound NOW+SUM calls. Setup uses validated candidates and matching command/event
CAS transactions, then actor replays actual journal. Setup, first context and
initial complete native calculation are outside timing. Later calculation may
compete for CPU; its completion is not part of the measured drain.

Current-thread async runtime releases1/8/32 tasks through Barrier. Each performs
four consecutive try_send attempts without awaiting between attempts. Thus this
is a controlled queue burst, not simultaneous threads, browser subscribers or
WebSocket throughput. Queue capacity64; rejected timer admission can be retried
by a later heartbeat. Unexpected errors fail the harness.

For each case:2 warmups,20 retained rounds. Each due round samples60s after
previous persisted timestamp; non-due samples59999ms after it. Actual journal
checks require exactly one Timer epoch for a due burst with accepted requests,
zero otherwise. A duplicate same-clock burst outside timing must leave hash/head
unchanged and append zero events. Timing stops after producer joins and a final
authorized actor head request. It includes Barrier readiness, scheduling,
enqueue, actor processing, SQL access, possible CAS commit and head barrier.
The barrier has a bounded retry budget; all observed barrier retries were0.

| Profile       | Producers | Due   | Attempts | Accepted | QueueFull | Drain p50 ms | Drain p95 ms | Maximum ms |
| ------------- | --------- | ----- | -------- | -------- | --------- | ------------ | ------------ | ---------- |
| one_now       | 1         | false | 80       | 80       | 0         | 2.305        | 3.063        | 4.113      |
| one_now       | 1         | true  | 80       | 80       | 0         | 4.939        | 5.469        | 5.488      |
| one_now       | 8         | false | 640      | 640      | 0         | 15.637       | 22.707       | 22.784     |
| one_now       | 8         | true  | 640      | 640      | 0         | 16.828       | 22.027       | 36.535     |
| one_now       | 32        | false | 2560     | 1280     | 1280      | 29.578       | 36.881       | 40.223     |
| one_now       | 32        | true  | 2560     | 1280     | 1280      | 31.817       | 34.503       | 40.776     |
| 1000_formulas | 1         | false | 80       | 80       | 0         | 2.776        | 3.031        | 5.021      |
| 1000_formulas | 1         | true  | 80       | 80       | 0         | 6.612        | 11.588       | 12.206     |
| 1000_formulas | 8         | false | 640      | 640      | 0         | 18.550       | 23.232       | 26.982     |
| 1000_formulas | 8         | true  | 640      | 640      | 0         | 22.063       | 29.194       | 34.188     |
| 1000_formulas | 32        | false | 2560     | 1280     | 1280      | 31.538       | 51.392       | 52.038     |
| 1000_formulas | 32        | true  | 2560     | 1280     | 1280      | 40.379       | 111.679      | 186.800    |

Attempts/counts exclude warmups and duplicate verification bursts. Increasing
producers increases offered work; these are not comparisons at equal load.
For20 drain observations, p50 is the10th sorted sample, p95 the19th (second
maximum). This is a small initial sample, not a stable tail estimate or CI gate.

try_send timings combine accepted and QueueFull attempts and include Instant
overhead. Every case enqueue p50 was0.100us; enqueue p95 in table order was
0.400,0.300,0.200,0.300,0.100,0.100,0.500,0.500,0.300,0.200,0.100,0.300us.
Sub-microsecond values should not be interpreted beyond this clock resolution.

Release harness passed1/1 in16.08s after2m01s compilation. All12 cases checked
durability, coalescing and queue accounting. The formula-heavy32-producer due
case shows a long tail requiring investigation; it does not identify SQL, AST
scan, candidate preparation, worker contention or scheduling as its cause.
No speedup, user-command commit p95, browser delivery threshold or Google Sheets
comparison is claimed. Native/WASM parity and browser suites were not rerun for
this test-only addition; previous verification retains its own scope.

Open performance task: record stage-level SQL/actor/calculation traces and repeat
the formula-heavy burst on a controlled runner before selecting an optimization.
Completion requires attributable timing and a before/after correctness-preserving
comparison. Separate real WebSocket/cross-host workloads and calculated-result
completion remain deployment gates.
