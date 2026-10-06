# ADR-131: candidate WASI isolation for XLSX parsing

Status: Windows guest-boundary prototype verified, 2026-10-07; server integration pending.

ADR-130 makes the parser consume bounded stdin package bytes. To close the
filesystem/network and guest resource boundaries without platform-specific
unsafe code in PXEL, test the same parser as a wasm32-wasip1 executable under
pinned Wasmtime 49.0.2. Native and WASI workers share protocol5 implementation;
no duplicate parser, application state or persisted/wire schema is introduced.

Candidate runtime settings grant only stdio, clocks needed by parser deadlines
and no directories, environment inheritance, sockets or HTTP. Guest memory and
execution fuel are bounded; runtime timeout and the parent process-tree deadline
bound execution independently. Tests must actually attempt prohibited resources
and exhausted CPU/memory, and compare native/WASI normalized fixture output.
The native runtime host is not claimed to have an OS-wide memory sandbox.

Verified invocation: run -C cache=n, -W fuel=500000000,
-W max-memory-size=268435456, -W timeout=5s, -S inherit-env=n,
-S inherit-network=n, -S tcp=n, -S udp=n, -S http=n. No directory,
environment, preload, inherited socket or allow-precompiled option is passed.
Tests use independently lowered fuel and timeout to prove both traps.
The input is a trusted build artifact, never an uploaded executable module.

Wasmtime is a new required external tool for the isolated path, stored locally
under ignored .tools. Rust's additional wasm32-wasip1 standard library remains
under the isolated pinned toolchain. No new third-party Rust package is needed;
serde_json 1.0.151 is already in Cargo.lock and is required to encode the shared
normalized-workbook protocol from the standalone worker.

Official sources: [Wasmtime CLI](https://docs.wasmtime.dev/cli-options.html),
[49.0.2 release and security fixes](https://github.com/bytecodealliance/wasmtime/releases/tag/v49.0.2).
Windows runtime archive SHA256 from the immutable release asset:
c95a3925acde6398808baabe5ad13d0b1df02b004bc256c00cf8e2a36daccea8.

This ADR is not evidence that isolation has passed. Runtime version, compiler
target, settings and actual tests must be recorded before adopting a baseline.
Server integration, malware policy, browser preview/selection/import and the
remaining nine functional steps are still required. No upload endpoint is added
until its complete boundary has been verified.
