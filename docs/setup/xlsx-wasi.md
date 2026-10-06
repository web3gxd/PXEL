# Local XLSX WASI verification

This is the guest-boundary verification for ADR-131, not a public upload or
deployment gate. Wasmtime 49.0.2 is a newly pinned external verification tool;
the parser and protocol use existing Rust dependency versions.

On Windows x86_64, use the official immutable release archive under ignored
.tools and check its published SHA256 before extraction:

```powershell
$pxelWasmtimeRoot = 'C:/projects/PXEL/.tools/wasmtime/49.0.2'
New-Item -ItemType Directory -Path $pxelWasmtimeRoot -Force | Out-Null
$pxelWasmtimeArchive = Join-Path $pxelWasmtimeRoot 'wasmtime-v49.0.2-x86_64-windows.zip'
Invoke-WebRequest 'https://github.com/bytecodealliance/wasmtime/releases/download/v49.0.2/wasmtime-v49.0.2-x86_64-windows.zip' -OutFile $pxelWasmtimeArchive
if ((Get-FileHash $pxelWasmtimeArchive -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'c95a3925acde6398808baabe5ad13d0b1df02b004bc256c00cf8e2a36daccea8') { throw 'Wasmtime checksum mismatch' }
Expand-Archive -LiteralPath $pxelWasmtimeArchive -DestinationPath $pxelWasmtimeRoot -Force
. ./scripts/env.ps1
just xlsx-wasi-check
```

The recipe builds trusted source to wasm32-wasip1, builds the native comparison
worker and verifies the real Wasmtime version, capability/protocol/parity
cases and configured backend execution. It never executes uploaded WebAssembly or accepts a user-supplied runtime
argument. Tests can use an operator-selected PXEL_WASMTIME_BIN path; CARGO_TARGET_DIR
is respected. Other hosts must install their verified official 49.0.2 runtime;
Linux execution and artifact verification remain an open deployment gate.

Runtime output must not be logged by a future production adapter. This harness
uses synthetic fixtures and compares IR checksums, without dumping workbook
payloads. Clocks and stdio are allowed, while directory/environment inheritance,
network capabilities, HTTP and compiled-module cache are disabled. Guest memory
is capped at 256 MiB; fuel and timeout tests independently terminate loops.
Those caps do not establish an OS-wide host-process memory limit.

## Operator-configured backend (ADR-132)

Set `PXEL_XLSX_SANDBOX_MANIFEST` to an operator-owned JSON file with exactly:
`schema_version` (1), `runtime_version` ("49.0.2"), absolute `runtime_path` and
`module_path`, and lowercase 64-character `runtime_sha256` and `module_sha256`.
Calculate artifact hashes only after verifying the official runtime archive and
building the trusted locked worker source. Keep the manifest under ignored
`.local`; the service must not be able to modify artifacts or parent directories.

Each CLI preview/import verifies the artifact hashes and runtime version before
execution. Configured failures never fall back to native parsing. Without the
manifest, the existing operator-only CLI still uses its native child. This does
not enable browser uploads or establish a startup readiness/public-upload gate.
