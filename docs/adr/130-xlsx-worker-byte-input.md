# ADR-130: bounded XLSX worker byte input

Status: locally verified byte/process boundary, 2026-10-07; sandbox remains open.

Step 1's public XLSX upload remains gated on sandbox resource/filesystem/network
controls, malware policy and complete compatibility reporting. ADR-099's native
process-tree boundary is retained; this increment does not claim a sandbox.

The existing OOXML parser already accepts Read + Seek. The parent reads an
explicit operator-selected regular file, capped at 32 MiB plus a one-byte
overflow probe and a ten-second read deadline. It sends package bytes rather
than a filesystem path. A future isolated executor can therefore receive stdin
without requiring any filesystem preopen or plaintext upload temporary file.
No new crate or HTTP upload endpoint is introduced.

Worker protocol 5 consists of PXIW, big-endian u16 version, big-endian u32
nonzero bounded source length, exact package bytes and EOF. Previous protocol 4,
unknown versions, excessive lengths, truncated bytes and trailing input reject
before publication. The worker parses Cursor-backed memory using the existing
bounded normalize_reader and emits the existing validated IR with protocol 5.
Response limits, process-tree cancellation, stderr suppression, environment
clearing and independent parent IR validation remain in effect. Workbook,
storage, WebSocket and normalized-workbook schema versions do not change.

Verification covers unit request/response framing and malformed IR, actual child
byte parsing of a two-sheet fixture, malformed/oversized/old-version child
requests, existing preview redaction and process-tree cancellation. Results are
recorded in RUNBOOK. Hard memory/CPU limits, filesystem/network denial, malware
policy and browser XLSX preview/import are subsequent required slices, not
completed by this protocol change.
