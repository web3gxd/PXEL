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
worker and verifies the real Wasmtime version and 10 capability/protocol/parity
cases. It never executes uploaded WebAssembly or accepts a user-supplied runtime
argument. Tests can use an operator-selected PXEL_WASMTIME_BIN path; CARGO_TARGET_DIR
is respected. Other hosts must install their verified official 49.0.2 runtime;
Linux execution and artifact verification remain an open deployment gate.

Runtime output must not be logged by a future production adapter. This harness
uses synthetic fixtures and compares IR checksums, without dumping workbook
payloads. Clocks and stdio are allowed, while directory/environment inheritance,
network capabilities, HTTP and compiled-module cache are disabled. Guest memory
is capped at 256 MiB; fuel and timeout tests independently terminate loops.
Those caps do not establish an OS-wide host-process memory limit.
