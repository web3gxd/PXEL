# ADR-003: One pure Rust engine for native and WASM

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 architectural boundary; formula profile remains proposed until phase 2 approval.

## Context

Server canonical calculation and immediate browser preview need matching semantics without duplicating a formula implementation. Formula input is untrusted and must not panic or execute arbitrary code.

## Decision

Pure spreadsheet-formula crate depends on domain/general-purpose libraries only. Native server uses it directly; spreadsheet-wasm is a thin adapter in a Web Worker. Versioned normalized AST lives temporarily in domain::formula without parser spans/evaluator dependencies. No ready-made formula framework.

Use deterministic binary64 normalization and operation order; no fast-math. First exponent profile is bounded integer exponentiation with a fixed algorithm. Broader math functions require a portable implementation and differential corpus. Persisted CalculationContext/AdvanceCalculationEpoch make volatile calculation replayable; deterministic RAND key includes stable AST node identity.

## Alternatives

Independent JS evaluator risks semantic drift. Server-only evaluation misses low-latency preview. Excel/LibreOffice runtime violates deployment/runtime constraints. Standard platform powf alone does not establish native/WASM equality.

## Consequences

Preview is never authoritative. Shared fixtures compare exact normalized native/WASM hashes. Excel compatibility is a separate golden-fixture claim, not bitwise parity. Engine/profile/context versions appear in result messages and snapshots. New dependencies require license/support checks.

## Validation

Parser properties/fuzz/snapshots, incremental dependency tests, cycle/budget checks and real browser WASM parity gate are required in phase 2. No formula engine crate is created empty in phase 0.
