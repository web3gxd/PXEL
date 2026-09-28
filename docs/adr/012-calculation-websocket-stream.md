# ADR-012: Bounded WebSocket delivery of server calculations

Date: 2026-09-29. Status: implemented for the immutable-axis formula profile;
phase 2 and collaboration gates remain open.

## Context

The first formula slice exposed calculated results through an HTTP endpoint.
Polling could display a server result but did not exercise the independent
`CalculationCommitted` transport promised by the operation protocol. A complete
result can exceed one WebSocket frame, and calculations may finish out of order
relative to authored commits. The result cache is derived data, never the
durable document state.

## Decision

The client sends `RequestCalculation(document_id, input_version)` over the
authenticated WebSocket. The server responds with one or more
`CalculationCommitted` frames. Each frame carries the document, input version,
calculation epoch, engine/profile/context identity and increasing sequence.
Frames are at most 256 KiB; only the final successful frame has the 32-byte
calculated state hash. A complete result may therefore have many frames, but
the browser publishes its assembled cache only after the final frame has been
validated. A final error frame has a typed calculation code and no result hash.

The current result is a full formula-result snapshot for one immutable input
version. It is not an operation ACK. The browser rejects stale versions,
metadata changes, sequence gaps, unknown cells, non-formula addresses, excess
bytes and incomplete final results. It keeps calculated results separate from
confirmed authored content and from the WASM preview. The previous HTTP
endpoint remains for diagnostics and parity checks; the product UI does not
poll it.

One connection has one cancellable calculation response and a two-frame
outbound channel. A new request drops the previous response. Encoding uses a
bounded blocking pool with at most two concurrent permits, and each frame
rechecks document authorization and the current calculation version. Socket
writes have a timeout; a slow client cannot hold the document actor. Waiting
and encoding have a 30-second deadline. These are delivery limits, not a claim
that formula computation always finishes within 30 seconds.

Epoch 0 and context ID equal to the document ID identify the current
nonvolatile profile only. This temporary identity must be replaced by a
persisted `CalculationContext` and `AdvanceCalculationEpoch` system event
before volatile functions are enabled. A future calculation may skip an older
version; a request for an already superseded version is not replayed from a
historical result cache. Reconnect requests the latest confirmed version.

## Authorization boundary

The session authenticates when the request starts and immediately before each
frame is sent. A development cookie can be revoked for future HTTP requests
and WebSocket upgrades. An existing WebSocket still has a cached principal;
per-frame checks alone do not close the check-to-send race during ACL changes.
Phase 3 requires the ordered `access_epoch` control gate and queued payload
invalidation described in `docs/collaboration.md` before claiming immediate
revoke of an active subscription.

## Verification and migration

Rust tests cover bounded multi-frame encoding, strict sequence, complete cell
set and hash only on the final frame. Integration tests use real PostgreSQL,
WebSocket authorization and a result large enough to force multiple frames.
Browser tests run on DPR 1/2 against Vite and the built assets served by Axum;
the formula scenario asserts that no calculation HTTP polling occurs. Actual
command results are recorded in RUNBOOK.

Wire protobuf v1 gains additive request/result fields. The persisted event
envelope and its migration policy are independent; this transport change does
not rewrite the operation log.
