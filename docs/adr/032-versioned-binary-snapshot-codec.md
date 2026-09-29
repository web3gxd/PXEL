# ADR-032: Independent versioned binary snapshot envelope

Date: 2026-09-29. Status: ACCEPTED FOR THE SECOND PHASE-5 SLICE.

## Context

ADR-031 provides an internal image and checked domain restoration, but it cannot be stored or used for recovery without a persisted format. Reusing event-envelope bytes as a snapshot would couple two lifecycles and make startup replay semantics ambiguous. A protobuf byte sequence is not a canonical domain hash.

## Decision

`proto/pxel/storage/v1/snapshot.proto` defines a separate `SnapshotEnvelope` with its own schema version, document/version, last axis-change version, single-sheet axes, sparse authored cells, retired IDs, optional CalculationContext, cacheless state hash and writer engine/profile labels. It reuses immutable storage leaf messages from `log.proto` (addresses, cell content, bound formula, context), but never the event envelope or event schema number. Any incompatible leaf semantic change requires a new snapshot schema/message rather than reinterpreting old bytes.

`spreadsheet-storage-codec` encodes and decodes this envelope. Decode rejects unknown schema, oversized input, malformed identities/cells, duplicate or live retired IDs, invalid context, mismatched document/version/last-axis-version/hash/engine/profile, and a context whose engine/profile differs from the writer labels. The restored workbook must reproduce the stored cacheless domain hash. The first codec caps both encoded and decoded payloads at 32 MiB. Larger valid workbooks fall back to full journal replay until a bounded streaming decoder and allocation benchmark justify a higher cap; no partial snapshot is published.

The decoder expects an authorized ACTIVE PostgreSQL metadata record to provide document/version/hash/engine/profile. BlobStore must verify the exact byte checksum before decode. Neither metadata activation nor BlobStore integration exists in this slice, so the runtime recovery path still uses the full operation log. `last_axis_change_version` is not part of the existing domain hash; activation must bind it to the verified payload and metadata, and full-replay validation must cover the first published snapshot generation.

## Verification and next work

Codec tests cover a formula/context/tombstone round-trip, tail replay equality, unknown schema, duplicate retired ID, writer/context mismatch, expected metadata mismatch and size rejection. The next slice adds immutable blob upload, PostgreSQL lifecycle metadata, verification before ACTIVE, fallback to an older ACTIVE generation and crash/recovery tests. S3 contract runs and automatic scheduling follow those local correctness checks.
