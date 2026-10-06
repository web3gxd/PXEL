# ADR-109: shared frozen rows and independent personal filters

Date: 2026-10-05. Status: implemented and locally verified (2026-10-06).

The approved additions distinguish shared sheet layout from each user's filter.
The workbook owns a requested count of frozen top rows, at most 1,000. A view
clamps that count to the current row extent after deletion; a later extension
can restore the requested extent. Changing the count uses `SetFrozenRows` and a
single guarded `FrozenRowsWrite` through the existing candidate, head CAS and
durable commit. It does not dirty formula nodes or renew the calculation epoch.
The initial layout command is not an Undo target, matching existing axis sizes.

The domain hash gains an omitted-when-zero `PXEL-FROZEN-ROWS-1` suffix, preserving
legacy hashes. Durable event schema 36 and snapshot writer 7 evolve independently
of wire protocol 1. Snapshot readers retain schemas 1–6 and reject a nonzero
freeze field under an older schema. The public wire adds distinct command and
canonical operation fields; unknown required semantics still fail atomically.

Personal view schema 1 stores up to five stable-ID filter ranges, each with an
optional header row and up to 50 distinct column conditions. At most 100,000
cells may belong to each range. Column conditions combine through AND. Supported
predicates are selected scalar values, blank/nonblank, case-sensitive Unicode
text equality/contains/prefix, and finite numeric equality/inequality/comparison.
Text numbers are not coerced to numbers. Blank includes an empty text result;
formula errors are nonblank and do not match a numeric/text predicate.

Settings are stored by `(document_id,user_id)` in PostgreSQL, encrypted under the
book DEK with authenticated purpose `personal-view`, actor ID, independent view
revision and codec 1. They never enter the workbook hash, events, calculation
context or Undo/Redo. A readable role may save its own settings; no endpoint
accepts another user's ID. Shared endpoints authenticate the exact link session.
Revision CAS and a matching current head fence geometry validation before save.
Unknown DTO versions, malformed IDs, duplicate columns and limits reject the
entire update. Decrypted settings are capped at 128 KiB.

Filters evaluate confirmed scalar values and confirmed formula results. Pending
content and Worker previews do not affect visibility. Retired or invalid stable
boundaries suspend that filter rather than interpreting its old A1 coordinates.
A vertical merge is hidden only when all its rows match the hidden decision;
partial decisions keep the entire merge visible and report that adjustment.
This preserves the conservative merge contract of ADR-096.

The same row geometry must govern rendering, hit testing, keyboard navigation,
selection, scrollbars and editing. Hidden rows retain their stable identities and
authored content. Sorting will cover the full explicitly confirmed range,
including hidden rows; it remains the next separately implemented slice.

Member and scoped guest `GET/PUT .../personal-view` are independent versioned
REST DTOs. Read delivery retains an authorization lease through the response
body. The UI provides explicit range bounds, header choice, predicate controls,
save/reset and visible-count feedback. Actual checks and failures belong in
RUNBOOK; no deployment or NVDA speech verification is implied by this ADR.

Combined baseline: 664 Rust tests passed / 17 intentionally ignored, 242 web tests, check/sqlx/audit/parity passed. After all-hidden regression and clipboard waits, typecheck plus 243 web tests passed and full Chromium 224/224 passed. Live integration 77/77 and recovery 4/4 passed. Final exact smoke after all additions and manual NVDA remain pending; see RUNBOOK.
