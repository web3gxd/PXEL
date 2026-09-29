# POWER: local scalar and parity observations

2026-09-30 Moscow time, Windows 10.0.26100, Intel Core Ultra 9 185H, Rust 1.98.1
GNU, release build. Ordinary laptop, active dev stack. Frontend checks and native/
WASM builds ran during the broader run; no dedicated quiet reference runner.

```sh
cargo bench --locked -p spreadsheet-formula --bench scalar -- POWER_scalar --sample-size 10 --warm-up-time 1 --measurement-time 2
```

POWER(4,0.5): Criterion estimate interval 60.739–83.081 ns, central 74.330 ns.
Expected result 2 is verified before timing, AST parsed outside the loop. Each
iteration uses a fresh evaluator budget. This particular exponent exercises the
library's square-root special case, not the general logarithm/exponential path.
No general power-function latency, p95 commit or rendering threshold is claimed.
Parsing, dependency scheduling, PostgreSQL, network and WASM crossing excluded.

Shared corpus: 236 cases, normalized native/browser hashes and expected results
matched. WASM initialization7.1ms/evaluation9.7ms/aggregate roundtrip80.4553ms,
894184 WASM bytes. Stateful 1001 books/2002 patches includes POWER(A1,B1) with
seeded signed fractional values and ABS(A1)^0.5: aggregate compute145ms and
roundtrip1636.1807ms, one WASM load. These are batch observations, not per-edit p95.
Chromium153.0.8010.12, Node24.18.0. Published Excel examples are checked at their
documented decimal precision; manual Excel execution and cross-platform parity
beyond this native/WASM pair are not established.
