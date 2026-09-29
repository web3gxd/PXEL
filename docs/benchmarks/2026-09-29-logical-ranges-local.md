# Logical range evaluation, 2026-09-29

Command: `cargo bench --locked -p spreadsheet-formula --bench scalar -- 'AND|OR' --sample-size 10 --warm-up-time 1 --measurement-time 2`.

Host: Windows 10.0.26100, Intel Core Ultra 9 185H, Rust 1.98.1 GNU toolchain,
Criterion 0.8.2. Release bench profile, fixed single-thread row-major traversal,
no fast-math. The local dev stack was idle; this is an ordinary laptop run, not
a fixed self-hosted acceptance runner.

The AST is parsed before timing. Each iteration evaluates `AND(A1:A100000)` or
`OR(A1:A100000)` through a resolver that visits exactly 100,000 numeric ones.
The boolean result is checked before timing. Each cell visit consumes work;
OR keeps scanning after its first true condition. No range vector is allocated.
This isolates evaluator traversal and coercion. It excludes sparse workbook
lookup, dependency scheduling, PostgreSQL, actor queue, WebSocket and Canvas.

| Case           | Criterion estimate interval | Central estimate |
| -------------- | --------------------------- | ---------------- |
| AND_range_100k | 1.5268–1.8862 ms            | 1.7585 ms        |
| OR_range_100k  | 1.5742–1.7065 ms            | 1.6437 ms        |

Ten samples, one second warm-up and two seconds requested measurement per case.
These are Criterion estimate intervals, not individual-call p95 latencies.
No earlier logical baseline exists, so no speedup is claimed. CPU contention
and frequency scaling can affect a repeat; benchmark source is retained for
comparison on the same fixture and toolchain.

Separate Chromium stateful parity run: 1001 books/2002 patches, including
logical formulas and the cross-sheet fixture, gave exact native/WASM results
and normalized hashes. Worker compute 199.2 ms, JS/Worker roundtrip 1542.1753 ms,
one WASM load; Chromium 153.0.8010.12. These aggregate corpus timings do not
measure an interactive edit p95 or Excel compatibility.
