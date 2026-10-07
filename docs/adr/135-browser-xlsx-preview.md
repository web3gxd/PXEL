# ADR-135: authenticated browser XLSX preview

Status: locally verified Windows browser preview, 2026-10-07.

Add POST /api/imports/xlsx/preview, strict JSON schema1 with file_base64 and
optional ascending unique selected_sheets indices. Omitted selection means all;
an explicit empty selection is invalid. Return bounded sheet metadata/samples,
the complete existing parser compatibility report and a digest of exact source
bytes plus canonical selection. This is a preview, never a durable publication.
Formula binding/import compatibility must also be checked before publication;
parser compatibility alone cannot establish that gate.

Authorize member/origin before buffering, acquire the existing single import/
export permit, require a verified operator sandbox manifest, limit request bytes
to48MiB and package bytes to32MiB, and bound body reading by15seconds. Public
execution requires an explicit Sandbox reference and cannot use native fallback.
Decode in memory, send framed bytes to the isolated worker, independently validate
IR, recheck active authentication, and release resources on failure/cancellation.
No plaintext temporary package, original-file persistence or response/body logs.

Reuse ADR-132/134 artifact pins, host/guest caps and process cancellation. Existing
operator CLI path reading delegates to the same byte-transfer implementation.
Keep wire/storage/workbook versions independent; no codec or dependency changes.
The library exposes a preview-only modal using the existing Modal/Select and
checkbox design system. File changes/unmount invalidate pending reads/responses;
selection changes require a fresh digest. A bounded typed decoder rejects unknown
versions, issue kinds, cell variants and invalid coordinates before rendering.
Samples show sparse original addresses and distinguish formulas/shared copies.
The UI explicitly states that publication is unavailable; parser compatibility
does not establish formula-binding compatibility. Publication/retry receipts
remain open. Linux enforcement remains unverified.
