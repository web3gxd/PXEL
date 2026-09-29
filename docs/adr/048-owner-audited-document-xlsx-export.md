# ADR-048: owner-only audited document XLSX export

Status: accepted for the sixth phase-6 slice (2026-09-29).

`GET /api/documents/{id}/export.xlsx` reads a server-confirmed document
projection and maps its current visible coordinates and formula text into the
validated `NormalizedWorkbook` scalar export IR. The XLSX writer has bounded
output and rejects unsupported names, characters, coordinates, formulas or
size before any response bytes are sent. Formula calculation caches and stable
IDs are not exported. The current in-memory export is limited to one concurrent
conversion per server process. It is not a job or object-storage artifact.

The endpoint is owner-only. An early owner check rejects viewers before the
CPU conversion slot is acquired. The server then authorizes the document read
and checks owner membership and document state again in the audit transaction. The
append-only `export_audit_events` row records actor, document/version, output
SHA-256 and size; it is committed before returning bytes. An audit failure
aborts the response. Each successful retry produces a separate audit record.
This records preparation of a download, not proof the client received every
byte. An ACL change concurrent with response delivery still needs the planned
HTTP send-gate test/implementation; the current owner-only role is not mutable
through the supported permission API.

The spreadsheet ZIP is generated in `spawn_blocking` from a bounded domain
snapshot. This is trusted server content, unlike the isolated untrusted XLSX
import worker. The writer and our reader have a synthetic scalar round-trip;
Excel/LibreOffice interoperability, styles, dates, merges, frozen panes,
document import/reimport and a downloadable compatibility report remain open.
The `export_audit_events` schema is independent of the durable operation log.
