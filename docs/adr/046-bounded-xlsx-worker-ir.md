# ADR-046: bounded XLSX IR across the child boundary

Status: accepted for the fourth phase-6 slice (2026-09-29).

ADR-045 returned only counts. The version-2 `PXOW` response now carries a
length-prefixed JSON encoding of `NormalizedWorkbook`: four magic bytes, u16
version, u8 status and big-endian u32 payload length, followed by at most
64 MiB of payload. Rejection has status 1 and zero payload. Unknown versions,
statuses, unexpected trailing bytes, oversized frames and malformed JSON fail
closed. The request is unchanged except that it also uses protocol version 2.
JSON is a local, versioned transport encoding here, not a canonical state hash
or durable operation format.

The parent deserializes into the XLSX-specific IR, then independently validates
sheet count/names, strict sorted and unique coordinates, Excel coordinate
bounds, cell and expanded-text limits, formula lengths, finite normalized
numbers and unknown fields. The child runs the same validation before encoding.
No stable document IDs are assigned and no imported content is logged or printed
by the preview CLI. A hostile or stale worker cannot bypass parent IR checks.

This deliberately pays one JSON serialization/deserialization at import time.
It is not used for interactive edits. The 64 MiB frame cap can reject an XLSX
whose extracted text fits the 32 MiB IR limit but expands heavily through JSON
escaping; this is an explicit bounded-profile limitation. Parent still holds
the payload and decoded model at once, so OS memory enforcement and a streaming
codec remain future work if profiling shows a need. The worker still has the
parent's OS user privileges. Do not expose uploads until OS isolation, malware
scan, compatibility reporting and authorization gates are in place.
