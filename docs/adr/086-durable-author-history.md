# ADR-086: durable author history discovery

Status: ACCEPTED — locally verified bounded author history discovery (2026-09-30).

## Previous decision

ADR085 makes explicit axis operations eligible for guarded compensation. Until
now the client kept 100-entry Undo/Redo stacks only in memory. Reload lost those
stacks even though the operations and their exact inverse identities were durable.
The log records no tab identity, direction intent or terminal rejected intent.
Consequently discovery reconstructs shared author history, not a previous tab's
private stack or a guarantee that a suggested inverse currently passes guards.

## Contract

GET /api/documents/{id}/undo-history authenticates the server-selected actor and
requires current owner/editor access. Query parameters cannot select an actor.
Schema1 returns document/actor IDs, a frozen through_version, earliest scanned
version, two arrays of operation IDs/committed versions, and explicit truncation.
Each stack contains at most100 targets, newest at the end. No contents, formulas,
preimages or calculated patches enter the response. Execution continues through
the existing UndoCell authorization, candidate, history guard and durable CAS.

An OperationJournal read scans a contiguous newest suffix through a frozen head,
including foreign and system events. Version floor is max(1,head-10000): the
target itself makes this at most10001 records, not10000. Internal pages128 are
streamed with a64MiB payload budget and4MiB individual envelope cap. The newest
selected suffix is reduced in ascending order; earlier omitted bytes are reported
as truncation rather than substituted with an empty history. Gaps, corrupt bytes,
unknown schemas and timeout are errors. The read holds no document lock throughout
the full scan; access epochs and edit permission are checked between pages and
before delivery. Concurrent commits may advance the head without changing the
frozen selected range. The server shares the existing two-reader history gate and
30-second timeout. No new history table or dependency is introduced.

Application owns content-free descriptors and the deterministic reducer. Own
supported originals push Undo and clear Redo. Compensation matching exact ID and
version anywhere in Undo removes that target and pushes the compensation to Redo;
matching Redo removes it and pushes to Undo. Independent entries survive nonlinear
chains from multiple tabs. A target outside the scanned/bounded/abandoned branch
becomes a new latest inverse action in Undo and clears Redo, preserving older Undo.
An ID/version mismatch in the visible suffix is corrupt. Own unsupported actions,
including current AddSheet and automatic ExtendRows, remain history barriers.
Foreign and system events do not modify this author's stacks. Terminal refusals
remain local; discovery may offer that potential target again after reload.

## Client consistency and scheduling

History loading does not block editing or durable command submission. Undo/Redo
wait for a current validated history. Responses are stamped with document, actor,
head and connection/generation; stale responses cannot erase new pending or
acknowledged work. ACKs included in through_version are not inserted twice.
Uncertain outcomes retain original operation bytes and IDs until receipt lookup.
Actor changes never transmit a prior actor's pending intents as the new actor.
One bounded previous-identity quarantine retains uncertainty; a further switch
that would overwrite it is refused locally rather than silently losing intents.
The total active plus suspended intent count retains the128 limit.

Additive DocumentHeadChanged scheduling metadata distinguishes exact user/system
events and identifies an exact user's opaque actor ID. Unknown/coalesced/older
server hints remain unknown. This is neither canonical content nor proof of Undo
eligibility. Own remote events and unknown resyncs refresh history; foreign events
do not change the author stack. A separate observed-history watermark advances
only through verified discovery or successive known ACKs/head hints, never merely
through an HTTP projection refresh. A gap invalidates history even when the last
event is foreign/system or a local ACK: coalescing may hide an earlier own edit.
Immediate successive foreign/system hints avoid full scans. Exact fresh system
events retain their provenance through explicit recalc, opening and bootstrap;
receipt-only or unmatched refreshes remain unknown. Existing protocol1 readers
ignore additive fields. Persisted
event/snapshot formats and engine numeric identity stay unchanged.

## Verification

Pure reducer, PostgreSQL/ACL/bounds/corruption, HTTP DTO and real browser tests
prove these contracts, including nonlinear history, reload, shared actor,
late receipts, branching and foreign conflicts. Eight reducer tests, seven real
PostgreSQL discovery tests, 139 frontend unit tests and the full 80-test HTTP
suite passed. The 142-test browser regression passed before the final fallback
envelope omission fix; 34 affected tests then passed, and the final 16 history
tests passed on the updated binary. Native/browser parity remains unchanged.
RUNBOOK records exact commands, failures, recovery results and verification scope.
This ADR does not claim completion of all phase4, XLSX or production gates.
