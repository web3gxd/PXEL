# Historical formula binding: local measurement

Date: 2026-09-29. Windows 10.0.26100, Intel Core Ultra 9 185H,
Rust 1.98.1 Windows GNU, release build. Ordinary local laptop; no fixed CI runner.

Command:

```sh
cargo test --locked --release -p spreadsheet-collab --lib axis_history::tests::historical_formula_binding_benchmark -- --ignored --exact --nocapture
```

Fixture: 1,000 initial rows, three columns, 100 successive row insertions,
101 axis checkpoints. A delayed `SUM(A1:A3)` binds to its original targets and
renders as `SUM(A101:A103)`. Five warmups and 100 measured bindings.

| Metric                     | Measured value |
| -------------------------- | -------------: |
| Accounted checkpoint bytes |      3,562,238 |
| p50 binding                |       9.712 ms |
| p95 binding                |      26.795 ms |
| Maximum binding            |      31.987 ms |

The measurement excludes PostgreSQL, candidate commit, networking, UI and
formula evaluation. Accounted bytes are the conservative cache admission
estimate, not measured process RSS. Results do not establish a universal
50 ms commit threshold. The harness is ignored during ordinary unit tests and
must be invoked explicitly. Restart historical hydration, very large axes and
projection-resync cost still need separate measurements.
