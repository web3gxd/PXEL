# ADR-132: pinned backend XLSX runtime adapter

Status: implementation in progress, 2026-10-07.

Connect ADR-131's verified standalone worker to the backend's existing bounded
process/IR boundary. An operator supplies PXEL_XLSX_SANDBOX_MANIFEST, schema1,
containing absolute runtime/module paths, SHA256 digests and runtime version
49.0.2. Requests cannot select executables, module paths or runtime arguments.
Unknown schemas, versions, hashes, symlinks/reparse points, missing/oversized
artifacts and incompatible module headers fail closed with redacted typed errors.
The actual runtime version is checked with bounded output and a deadline.

The runtime command uses the fixed verified capability/fuel/memory/time settings
from ADR-131. No directory/environment/network inheritance, HTTP, cache or AOT
is enabled. Artifact file handles are retained for execution; Windows denies
write/delete sharing while the handles are held. Deployment artifacts and parent
directories must be operator-owned and read-only to the service on Unix. Linux
runtime and filesystem behavior remain unverified deployment gates.

Configured execution never falls back to native parsing on validation, spawn,
protocol or resource failure. Existing operator-only CLI preview/import keeps
its native behavior when no sandbox manifest is configured; this is not a public
upload fallback. The future HTTP import must require an available isolated
executor. No HTTP upload endpoint or publication change is introduced here.

Parent process-tree cancellation, ten-second transfer deadline, bounded output
and independent IR validation remain mandatory. Guest memory is not an OS-wide
host-process memory cap; that host resource policy and malware/import policy
remain open before public XLSX upload. PostgreSQL candidate/CAS/commit contracts
and workbook/storage/wire versions remain unchanged. No new dependency is needed.

Tests must exercise actual configured CLI execution, valid native/WASI results,
tampered runtime/module digests, unknown manifest versions/fields, missing
artifacts and failure redaction. Results are recorded in RUNBOOK.
