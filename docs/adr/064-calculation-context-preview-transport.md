# ADR-064: Authenticated calculation context in browser preview

Date: 2026-09-30. Status: ACCEPTED FOR CONTEXT TRANSPORT SLICE.

ADR-013 persisted the complete context but HTTP exposed only ID/epoch and
WASM hydration discarded it. Add an independently versioned nested context
schema1 to authorized document projections. Include ID, epoch, UTC timestamp
as decimal string, fixed UTC offset minutes, civil day since Unix epoch,
64 lowercase hex seed, engine/profile labels and reason. Seed is document
calculation input, not an authentication credential; never log it or responses.

Worker initialization schema3 requires a complete context. Schemas1/2 remain
accepted only without a context, preserving the existing nonvolatile profile.
Schema3 works for one or multiple sheets. Missing/unknown schema, malformed
fields, inconsistent civil day, epoch greater than document version and
engine/profile mismatch fail before hydration/evaluation. Scalar patches do
not change context. Existing system-epoch notifications force an authorized
projection resync and Worker reset; no whole-book copy for ordinary edits.

Domain hydration gains a context-aware entry point which validates context
and current version without applying a synthetic event or increasing version.
It is browser preview state, never canonical persistence. HTTP context cannot
forge server system events; canonical commands continue through server binding.

This slice closes context transport. NOW/TODAY/RAND remain disabled until their
next evaluation slice has native/browser fixtures, per-cell/node derivation
and epoch-change/recovery tests. Open/timer/volatile-dependency triggers and
durable timezone/date-system configuration remain required separate work.

Acceptance: malformed/missing/stale context rejection, complete native preview
hydration, context retention across patches, real browser differential schema3
cases, HTTP-to-Worker initialization and existing epoch resync fencing tests.
Wire protobuf and persisted event/snapshot formats are unchanged.

The real browser test exposed an existing HTTP/Worker DTO leak: formula
current_text/authored_source caused strict Worker JSON rejection on reload.
PreviewClient now converts formula metadata through an explicit whitelist,
using current-axis source and omitting a stale canonical hint. Rust keeps
deny_unknown_fields and independently binds the source; no client AST trust.

Actual evidence: domain context tests3/3, stateful adapter9/9, frontend82/82,
1003 native/browser books with2006 patches (two schema3 cases, one/two sheets),
targeted real PostgreSQL1/1 and browser context/reload/epoch-lostACK4/4 at DPR1/2.
Review found late context validation; moved decoding before axes/binding and
added an ordering regression. Recheck and separate DTO-conversion review found
no further concrete defect. Full regression results are recorded in RUNBOOK.
