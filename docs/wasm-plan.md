# Native/WASM integration plan

Status: stateful product Worker integrated, 2026-09-29; phase 2 remains open.
The original fixture adapter evidence below is historical and narrower than
the current product implementation. ADR-011 defines the current boundary.

## Stateful product evidence

`PreviewWorkbook`/`WasmWorkbook` use the same domain hydrate/preview-candidate
and CalculationProjection on native and WASM. Initialization is bounded at
32 MiB; patches at 1 MiB/1000 unique cells. Aggregate authored text/AST limits
are checked while binding, before retaining the next formula. Preview CAS uses
confirmed input version and a separate monotonic pending revision.

The React application uses a same-origin module Worker with one in-flight job
and a coalesced queue capped at 256 cells/1 MiB. Source edits/canonical changes
produce cell patches. Only initial open/resync sends the sparse checkpoint.
Result caches are separate from server-confirmed values. Invalid draft syntax
can be corrected without reinitialization; fatal failures disable preview
visibly. Document disposal terminates the Worker, ordinary WS reconnect does not.

Actual `tests/parity/workbook.mjs`: 1000 seeded single-sheet workbooks, 2000
preview patches, 3000 result sets/hashes agree between native and browser WASM.
The corpus covers primitive changes and nine arithmetic/aggregate/IF shapes;
it is not evidence for structural edits, multiple sheets or full Excel parity.
Seed 1347962188, Chromium 153.0.8010.12, Windows 10.0.26100, Ultra 9 185H.
One run: Worker compute 170.7 ms, harness roundtrip 1293.15 ms including 1.60 MB
input transfer; not a per-keystroke latency percentile. Report `.local/workbook-parity.json`.

The 2026-09-29 COUNT/COUNTA extension reran the same harness: 83 scalar
fixtures matched expected/native/browser WASM results, and 1000 seeded books
with 2000 patches matched again across nine formula shapes. This single run
reported 144.6 ms Worker compute and 1278.89 ms harness roundtrip for 1.60 MB
of input on Windows 10.0.26100, Intel Core Ultra 9 185H and Chromium
153.0.8010.12. These are harness totals, not input-latency percentiles.

The IFS slice expanded the shared corpus to 92 cases and the seeded stateful
generator to ten formula shapes. `just formula-parity` passed 92/92 expected,
native and browser WASM scalar results, plus 1000 books/2000 patches with
matching normalized hashes. That Windows/Chromium run measured 142.3 ms Worker
compute and 1392.33 ms total harness roundtrip over 1.60 MB input. These
measurements are aggregate harness timings, not product latency percentiles.

The IFERROR slice expanded the corpus to 99 cases and the stateful generator
to eleven shapes. A first run exposed a one-ULP _harness_ mismatch for division:
native/WASM semantic hashes agreed, but the native example parsed its own JSON
result into `serde_json::Value` before serializing it again. The runner now
validates and retains each result as `RawValue`, preserving its numeric JSON
without a second decimal-to-binary conversion. The unchanged exact comparison
passed 99 scalar cases and 1000 books/2000 patches. This run measured 132.3 ms
Worker compute and 1244.06 ms total roundtrip over 1.60 MB input on the same
Windows/Chromium machine. No numeric tolerance was introduced.

Actual browser suite: draft preview, invalid-to-valid recovery, cancel with
unchanged durable version/hash, reload, server recalculation and missing-WASM
failure passed at DPR 1/2. Twelve scenarios passed against built Vite assets
served by Axum with production CSP. Worker and WASM are separate assets.

Persisted calculation contexts and an explicit authenticated recalculation HTTP
trigger are now implemented and replay-tested. The browser validates epoch and
context identity on server calculation responses. Worker initialization/patches
do not yet carry the persisted volatile context, so `RAND`, `NOW`, and `TODAY`
remain disabled. Open: native/WASM volatile parity, expanded workbook corpus,
performance percentiles and structural-axis support.
The following historical proposal sections remain context; their statements
that product integration is pending are superseded by this section and ADR-011.

## Actual adapter and parity evidence

Implemented `crates/spreadsheet-wasm`, `scripts/wasm.mjs`, and
`tests/parity/formula.mjs`. The adapter calls the shared Rust parser/evaluator;
there is no JavaScript formula implementation. `evaluate_fixture` is used by
the native runner and the exported WASM `evaluate` wrapper.

Actual `cargo test --locked -p spreadsheet-wasm`: 3 tests passed. Adapter Clippy
with warnings denied and `cargo check` for `wasm32-unknown-unknown` passed. Actual
`node scripts/wasm.mjs`: release `wasm32-unknown-unknown` build and matching
wasm-bindgen 0.2.129 generation passed. Actual
`node tests/parity/formula.mjs`: all **77** shared corpus cases matched expected
typed values and exact native/WASM normalized hashes in Chromium. The test
loaded the real WASM binary in a module worker under the CSP described below;
aborting a second worker's binary load produced failure, not a JS fallback.

The test adapter accepts a strict versioned object:
`{schema_version:1,current_sheet,sheets,cells}`. Cells have sheet name, zero-based
row/column and tagged primitive/error value. A cells-only array is a fixture
convenience with Sheet1 as origin and a catalog inferred from cell declarations.
Explicit catalogs distinguish an empty known sheet from a missing sheet (`#REF!`).
Input limits: 32 KiB formula UTF-8, 1 MiB cells JSON, 100000 cells. Diagnostic
names remain typed. Result hashes use a versioned explicit tag/binary64/text
encoding, independent of JSON serialization, with normalized zero.

Single observed run, not a p95 benchmark: WASM 281517 bytes, initialization
23.4 ms, 77 evaluations 18 ms, roundtrip 105.08 ms, input 11022 bytes. Machine:
Intel Core Ultra 9 185H, Windows build 26100, Node 24.18.0, Chromium
153.0.8010.12. Full report: ignored `.local/formula-parity.json`. The fixture
test does not establish incremental workbook parity, Excel verification, or
the production Axum asset/CSP integration gate.

## Verified tooling

- Workspace pins Rust 1.98.1. Actual `rustup target list --installed` returned
  `wasm32-unknown-unknown` and `x86_64-pc-windows-gnu`.
- Official registry reports both `wasm-bindgen` and `wasm-bindgen-cli` latest
  stable 0.2.129, released 2026-09-25. The crate is not yanked, license
  `MIT OR Apache-2.0`, declared Rust minimum 1.81.
- Downloaded the official Windows MSVC CLI archive into ignored `.tools`;
  SHA256 was verified before extraction:
  `79e348c169d0c10c0b287647f1e753d0985353ca96800bc29be8f4261baf13e6`.
- Executed `.tools/wasm-bindgen/wasm-bindgen-0.2.129-x86_64-pc-windows-msvc/wasm-bindgen.exe --version`:
  actual output `wasm-bindgen 0.2.129`. `wasm-bindgen-test-runner.exe` is beside it.
  No global installation or PATH modification. `wasm-pack` is not needed.
- Current Vite 8.3.1 config has no WASM plugin or worker customization. Current
  Vitest uses the Node environment; it cannot establish browser parity by itself.
- Existing Playwright 1.63.0 Chromium runs phase-1 browser tests successfully.

Sources: [registry metadata](https://crates.io/api/v1/crates/wasm-bindgen/0.2.129),
[CLI release and checksums](https://github.com/wasm-bindgen/wasm-bindgen/releases/tag/0.2.129).

## Build workflow and product integration proposal

The Rust dependency and CLI are pinned to exactly the same 0.2.129 version. The
thin `spreadsheet-wasm` adapter has `cdylib` and native `rlib` output. Its dependencies
are the pure formula crate and necessary boundary/hash tooling; it must not depend
on HTTP, Tokio, SQLx, OS randomness, or server adapters.

The build script successfully executed the first two commands below. Product
frontend integration with these generated assets is still pending:

```sh
cargo build --locked --release -p spreadsheet-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --typescript --out-name spreadsheet_wasm --out-dir apps/web/src/generated/wasm target/wasm32-unknown-unknown/release/spreadsheet_wasm.wasm
pnpm --filter @pxel/web build
```

The portable Node build script resolves the local CLI or configured PATH,
checks its version against the pinned dependency, spawns commands with argument
arrays, and fails if generated output is missing. It clears its verified output
directory before building so a failed compile cannot silently serve old binaries.
Keep generated JS,
declarations, and WASM together. CI must generate them before TypeScript or Vite
build. Do not leave a working old binary behind when a fresh build fails.

Use `--target web` ES-module glue with explicit initialization. Avoid assuming
Vite implements the default `bundler` WASM imports. In the worker, import the
generated initializer and import the binary through `*.wasm?url`, passing that
URL to the initializer. Vite supports explicit WASM asset URLs and module
workers. Verify the actual generated initializer signature before coding.
[wasm-bindgen deployment](https://wasm-bindgen.github.io/wasm-bindgen/reference/deployment.html),
[Vite WASM/worker support](https://vite.dev/guide/features.html#webassembly).

Create the worker with a statically analyzable constructor:

```ts
new Worker(new URL('./calculation.worker.ts', import.meta.url), { type: 'module' });
```

Keep it as a separate same-origin asset, not a blob/data inline worker. Set
`worker.format: 'es'` only if the generated module bundle requires it; verify
both dev and production builds. Disable WASM asset inlining or use a no-inline
asset URL so production parity can verify a real `.wasm` response.

## Worker lifetime and messages

One long-lived worker owns a Rust workbook/engine instance per open document.
Load allocated axes plus sparse contents once on initial open or genuine resync.
Afterward send commands and patches, not the whole workbook per keystroke.
Do not duplicate the evaluator in JavaScript, including as an error fallback.

Proposed boundary messages, finalized only after the engine API is available:

- `Initialize`: boundary schema, document ID, engine/profile/context IDs,
  confirmed version, bounded snapshot chunks.
- `ApplyConfirmed`: version and canonical operations, applied strictly in order.
- `SetPending`: pending-operation revision and bounded deltas. Reconcile
  rejected/transformed operations against the confirmed base.
- `Preview`: input version, pending revision, calculation epoch, request ID,
  patch sequence, final flag, changed results, normalized result hash.
- `Failure`: typed initialization, version, decoding, or calculation-limit
  failure without document content in logs.
- `Dispose`: free the Rust instance and terminate the worker when closing a
  document. Worker failure leaves server calculation usable and marks preview
  unavailable; it never manufactures confirmed results.

Bound message bytes, pending count, engine cell count, and evaluation work.
Each preview is tagged with both confirmed version and local pending revision;
discard any result whose complete identity no longer matches. Local preview
cannot advance server `calculated_version` or replace its result hash.

An executing synchronous WASM function cannot handle a new worker message
halfway through evaluation. Use an engine instruction/work budget and preferably
bounded recalculation steps that yield between chunks. Logical supersession
must discard stale output even if a previous calculation finishes. A watchdog
may terminate a stuck worker and reconstruct it from confirmed state plus
bounded pending operations; do not claim a postMessage cancellation flag alone
interrupts an active WASM call.

Measure structured-clone/transfer bytes, worker dispatch delay, WASM-call time,
changed-cell count, and total preview latency separately. Transferred buffers
are detached: never transfer the only copy of an unacknowledged command needed
for retry. Copy a compact patch or use a distinct owned buffer.

## Fixture and parity gate

Use one versioned corpus under `fixtures/formulas/`. Each fixture specifies
stable IDs/axes, primitive inputs, formula text, engine/profile/context,
ordered updates, expected typed values/errors, and expected dirty-cell sets.
Excel verification provenance belongs to each fixture and is separate from
native/WASM convergence. Synthetic expectations must not be labeled as
manually verified Excel outputs.

1. A native Rust runner executes each initial fixture and every update using
   the production engine API. It writes normalized hashes and typed results
   into a generated test artifact.
2. A Playwright browser test sends the same fixture/update sequence through
   the production worker and actual WASM binary. Compare every result with the
   native artifact and expected fixture values, not only with another JS hash.
3. Include Unicode/text, mixed types, blanks, error propagation, negative zero,
   precedence `=-2^2`, lazy IF, ranges, cross-sheet references, dependency
   removal, independent formulas, cycles, and limit/cancellation cases.
4. Verify a real WASM network load and a real worker. Fault the binary load in
   an isolated test and require typed preview failure, proving there is no JS
   evaluator fallback. Server calculation remains separately authoritative.
5. Run against Vite **and** the built frontend served by Axum, so generated
   asset URLs, MIME types, CSP, and initialization are covered.
6. Capture engine/profile versions, Rust/CLI versions, browser version,
   hardware/OS, fixture sizes and measured boundary overhead. Do not enforce a
   universal p95 target on shared CI.

## Production response policy prerequisite

Current Axum CSP contains `script-src 'self'`, which does not permit WASM
compilation. Add the narrower `script-src 'self' 'wasm-unsafe-eval'` and explicit
`worker-src 'self'` when integrating WASM. Ordinary JavaScript `unsafe-eval` is
unnecessary. Serve the binary as `application/wasm`. Check console/CSP errors
on port 8080; success on the Vite development origin is insufficient.
[CSP script/WASM policy](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/script-src),
[worker-src policy](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/worker-src).

## Remaining implementation work

Context: the stateless fixture adapter establishes scalar parity but is not a
workbook engine lifecycle. Root owns product integration; the formula agent owns
the core API. Remaining work requires stateful worker patches, shared workbook
fixtures, native/WASM workbook hashes, stale-preview rejection, and production
asset/CSP checks. Product frontend runtime files were not edited in this step.
