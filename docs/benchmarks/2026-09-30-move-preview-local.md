# Shared move preview: local convergence and boundary measurements

Date: 2026-09-30. ADR084. This is one sequential developer corpus run, not
a controlled p95 result or a production performance acceptance gate.

Machine: Intel Core Ultra 9 185H; Windows 10.0.26100; Chromium153.0.8010.12.
Release WASM, engine0.14.0. Concurrent development checks were possible.

Command: `node tests/parity/move.mjs`, after `node scripts/wasm.mjs` completed.
Generated code and runtime are real Rust/browser Worker implementations.
The harness loads WASM once and compares full outputs, including normalized
result hashes and rendered frozen move contents. It does not measure database
commit, network ACK or GPU presentation.

| Measurement                                           | Actual result          |
| ----------------------------------------------------- | ---------------------- |
| Deterministic workbooks                               | 209                    |
| Moves                                                 | 418                    |
| Shared manually specified expected cases              | 8                      |
| Native/browser output comparison                      | Exact equality, passed |
| Corpus JSON input                                     | 711990 bytes           |
| API UTF-8 input including initialization              | 707391 bytes           |
| API UTF-8 output                                      | 1814682 bytes          |
| Worker compute including JSON and byte counting       | 328.20 ms              |
| Worker roundtrip including initialization and cloning | 1372.39 ms             |
| Native cargo-run wall time including release build    | 26872.35 ms            |

Corpus covers overlap in both directions, successive moves, blank source,
overwritten destination formulas, relocated formula targets, direct and range
inbound references, cross-sheet inbound references and persisted volatile
contexts. Unsupported geometry/ranges and failure rollback are separate native
tests, not silently omitted successful fixtures.

Compute and roundtrip measurements have different scopes. Dividing them by move
count does not establish a latency percentile. Large-book clone cost and a
controlled machine run remain required; initial snapshot initialization costs
are not recurring geometry request costs. Raw report: ignored
`.local/move-parity.json`; the harness recreates it.

## Large-book measurements

`node tests/performance/move-preview.mjs` starts with 100000 occupied cells:
99000 scalars and 1000 dependent formulas. A 1000-cell move produces 3000 frozen writes.
The first move overwrites 1000 destination cells; subsequent measured moves have
99000 occupied cells. Each of 5 warmups and 20 measured alternating moves checks source
clears, destination values, 1000 rendered references, input version/pending revision and
the calculatedhash. An independent scalar is verified outside the timing window.
WASM loads and workbook initialization occur once, not per move.

| Measurement                    | Concurrent browser suite | After browser suite, no concurrent PXEL checks |
| ------------------------------ | ------------------------ | ---------------------------------------------- |
| Worker initialization hydrate  | 1202.1ms                 | 469.3ms                                        |
| Initial Worker roundtrip       | 1297.5ms                 | 517.9ms                                        |
| WASM move call p50/p95         | 366.2/472.1ms            | 152.7/190.3ms                                  |
| API encode+call+decode p50/p95 | 368.2/474.1ms            | 154.7/191.0ms                                  |
| Page/Worker roundtrip p50/p95  | 390.2/488.9ms            | 160.9/197.4ms                                  |

Initialization input is 14146036 bytes. Each move request is 497–499 bytes and each
response is 763340–763341 bytes, below the real 4 MiB response cap. Twenty measured calls
transfer 9971 input bytes and 15266816 output bytes. This distinguishes geometry requests
from full-workbook resubmission; output still includes all 3000 frozen writes.

Percentiles use nearest rank over 20 samples. Page/Worker timing excludes Playwright
protocol and includes cloning; it does not measure Canvas/GPU presentation. Both
runs have uncontrolled developer background load; the second is not a certified
performance runner. Memory was not measured. The large change between runs is
evidence that concurrent load materially affects the measurement, not that code
was optimized between samples. Original parallel report is retained locally as
`.local/move-preview-large-concurrent.json`; quiet rerun overwrites
`.local/move-preview-large.json`. The Worker p95 below 200 ms in the second run is
specific to this bulk fixture and does not prove the 16 ms local editing target. Candidate/cache clone and
planner attribution require profiling before any optimization.
