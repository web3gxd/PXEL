# ADR-116: encrypted home library and working templates

Status: ACCEPTED; implemented and targeted locally verified (2026-10-06).

The root URL presents the current workspace library; document query links and
guest share URLs remain valid. PostgreSQL joins current active documents,
memberships and ACL for every page; an owned transaction keeps selected ACL rows
locked through response delivery. Metadata titles and per-book personal favorite/
recent preferences use the book DEK and independent schema-1 revisions. Technical
list/card layout preference has no content and uses a bounded enum with user CAS.
Search text is transient. Pagination does not decrypt inaccessible titles.

The UI offers search, recent/favorite/shared sections, list/card layout, empty,
loading/error states, creation and the authenticated PXEL import preview. Budget,
Tasks and Sales templates contain actual formulas and typed formatting, and use
the same fully validated atomic new-book publication as file import. Titles join
that transaction. Workbook history/head/context/Undo do not change on metadata
or personal library preference writes. Pending edits block leaving the editor
until saved or explicitly discarded; normal browser unload also warns.

Markdown import uses the existing parser/preview after opening a new book.
Browser XLSX upload remains gated. Production authentication/workspaces and
external deployment retain their existing gates. Acceptance follows in RUNBOOK.

Guarded importer writes are ordered by stable address before persistence.
Template validation covers all three books and caught this codec prerequisite.
Title metadata is part of the import transaction and export checks its revision
under the final delivery lease. Native/PG/restart and core plus focus/layout
Chromium cases passed. Final local check/sqlx/audit/formula-parity/smoke passed:
698 Rust tests, 266 frontend units, 77 API/integration, 9 hard-restart and
318 Chromium cases at DPR1/DPR2. RUNBOOK records actual results and intermediate
failures. Manual NVDA and external deployment/CI gates remain open.
