# ADR-011: Stateful browser preview boundary

Date: 2026-09-29. Status: implementation in progress, phase 2 gate open.

## Context

The scalar differential harness sends fixture inputs for each calculation. It
cannot be the product editing API: copying the complete workbook per keystroke
would defeat the sparse incremental design. Server results remain authoritative.

## Decision

A same-origin module Worker owns a Rust workbook and CalculationProjection.
Initialization supplies one bounded checkpoint. Subsequent messages supply only
changed cell contents, confirmed input version and a monotonically increasing
local pending revision. Compare-and-set expected versions reject stale patches.
Replacing pending edits, accepting a canonical edit and removing its pending
counterpart are reconciled atomically before requesting the next preview.

The initial supported profile has one sheet with immutable axes. Formula sources
are rebound locally against those axes; this does not authenticate the server's
canonical AST or domain hash. A future structural profile must transmit canonical
bound state and implement structural transforms before enabling these operations.

Preview results are always tagged as preview and keyed by document, confirmed
input version and pending revision. They never enter the confirmed result cache.
The UI identifies displayed preview results. A stale response is ignored. Worker
failure is visible and has no JavaScript calculation fallback. Pending work is
coalesced and bounded; replacing a Worker cancels its remaining computation.

Hydration and preview candidates use explicit validated domain APIs. Preview
updates may preserve the confirmed document version; durable candidate/event
APIs retain their strict consecutive-version requirement. No runtime dependency
on test-only fixture construction is permitted.

## Build and security

Dev startup and the common quality command build pinned wasm-bindgen output
before Vite/TypeScript consume it. Failed WASM generation stops startup. Vite
emits separate module-worker and WASM assets, loaded from the same origin.

The production CSP permits `wasm-unsafe-eval` and same-origin workers; it does
not permit JavaScript `unsafe-eval`. This follows the separate WebAssembly
permission described by [MDN](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/script-src).
Asset imports and module workers follow [Vite's documented support](https://vite.dev/guide/features.html#webassembly).

## Verification and open gates

Required evidence: atomic invalid-patch rejection; bounded hydration; native/WASM
stateful parity; stale response rejection; preview visible before server ACK;
cancel/reject restores the confirmed view; missing WASM is a controlled failure;
no full-checkpoint transfer on ordinary editing; production CSP and built assets
work in Chromium. Individual test outcomes are recorded in RUNBOOK, not inferred
from this design. Calculation push transport was added later in ADR-012; phase 2
remains open for persisted context and the remaining formula profile.
