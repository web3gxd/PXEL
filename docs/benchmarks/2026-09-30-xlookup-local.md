# XLOOKUP: preliminary bound evaluator measurement

Windows10.0.26100, Intel Core Ultra9 185H, pinned Rust1.98.1 release,
engine pxel-formula-0.12.0. Local browser/build verification was running;
CPU load uncontrolled. Criterion20samples,1s warmup,2s requested measurement
(extended automatically). Estimates and intervals below are not p95 or gates.

```sh
cargo bench -p spreadsheet-formula --bench scalar -- XLOOKUP_bound --sample-size 20 --warm-up-time 1 --measurement-time 2
```

Real domain fixture with100k stable rows/populated numeric ones. Lookup and return
are A1:A100000. Exact matches have identical payloads; this fixture does not measure
duplicate payload differences, wildcard complexity or nearest miss cases. Expected1
is checked before timing. Each iteration runs actual bound evaluator with a fresh
budget; parsing/binding, graph scheduling, fixture construction, WASM crossing,
PostgreSQL/network/commit/UI are excluded.

| Search mode         | Estimate | Confidence interval |
| ------------------- | -------- | ------------------- |
| Forward1            | 32.011ms | 29.916–34.413ms     |
| Reverse-1           | 25.132ms | 24.285–25.954ms     |
| Binary ascending2   | 23.658ms | 22.733–24.631ms     |
| Binary descending-2 | 17.490ms | 17.178–17.814ms     |

All keys are collected and boundary-validated; binary modes additionally validate
monotonic order before lower-bound lookup. Total work is O(n), not logarithmic,
and no speed-up claim follows from differently loaded sequential runs. Lower-bound
search itself is logarithmic but cannot bypass source admission/cancellation.
Repeat on a controlled runner with stage attribution before regression/optimization
decisions. Sorted source caching would require its own invalidation proof, benchmark
and native/WASM tests; no such optimization is introduced here.
