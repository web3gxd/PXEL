# Stateful style parity, local verification

Command: `node tests/parity/workbook.mjs` (2026-09-30).

The release native runner and Chromium Web Worker used the same Rust engine and
compared complete normalized outputs, including independently computed authored
hashes for schema4 books. The corpus contains 1027 books and2037 patches: the
existing formula/volatile corpus plus17 style cases, covering all16 style leaves
and the shared styles-v4 fixture with stable identity metadata and multiple sheets.
All comparisons passed. Style-only cases evaluated zero formulas and preserved
the calculated-result hash.

| Observation                             |  Actual value |
| --------------------------------------- | ------------: |
| Serialized corpus input                 | 1660241 bytes |
| WASM loads in Worker                    |             1 |
| Worker computation for the whole corpus |       251.4ms |
| Browser roundtrip for the whole corpus  |     1740.26ms |
| Generated WASM size                     | 1632978 bytes |
| Vite-reported WASM gzip size            |      433.32kB |

Machine: Intel Core Ultra9 185H, Windows10.0.26100, Chromium153.0.8010.12,
pinned Rust1.98.1. The local application and PostgreSQL were running. This is a
single functional parity run; it does not establish p95 latency, isolated CPU
performance, a16ms optimistic-edit target, or a production throughput threshold.
The previous generated WASM was1355725 bytes; the added typed wire formula
decoder and style/metadata adapters increase the artifact size. Measure actual
loading and interaction before considering size or execution optimizations.

The shared adapter accepts a bounded Worker profile; oversized valid server books
may receive a typed preview refusal. This run does not prove all possible books
fit in Worker limits, Excel bitwise parity, styled XLSX compatibility, or crash
recovery of a complete UI session. Those remain separate acceptance checks.
