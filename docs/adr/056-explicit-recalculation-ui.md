# ADR-056: Explicit recalculation from the editor

Date: 2026-09-29. Status: ACCEPTED FOR THE UI SLICE.

## Context

ADR-013 exposes an authorized HTTP trigger for a durable calculation epoch.
ADR-055 bumps the engine identity and requires advancing older persisted
contexts rather than silently mutating them. The endpoint had no editor action.

## Decision

Add a “Пересчитать” menu action for the confirmed workbook. The client uses
the existing cookie/Origin-protected POST endpoint and a UUIDv7 operation ID.
It permits one active request, and refuses offline or pending-command states.
The actor performs authorization, candidate validation and durable CAS through
the existing system-event use case. No new wire, storage format or backend
behavior is introduced.

Keep the request ID after HTTP timeout/network failure or unsuccessful refresh:
the system commit may already have succeeded. A retry sends the same ID and
receives its original receipt. After matching operation ID and validating the
positive committed version, refresh at least that version before clearing the
request ID. A later explicit user action gets a fresh ID and a new epoch.
No context, formula result or document version is created optimistically.
Calculation remains asynchronous after the projection refresh.

## Consequences and verification

Viewers receive a clear permission error from the server; pending edits must
commit before the action. Unsaved drafts are outside the confirmed workbook
being recalculated. Request identity is retained within the current store;
reloading the page ends that retry session. A subsequent explicit click can
legally create a new epoch. Persisting HTTP intents across browser restarts
requires a separate pending-intent storage design.

Unit tests cover uncertain response retry, fresh subsequent intent and pending/
permission refusal. Playwright aborts the HTTP response after real PostgreSQL
commit and verifies retry does not create a second epoch, then reloads and
reads the confirmed formula result at DPR 1 and 2. Timer/open-triggered volatile
calculation and workbook timezone controls remain separate work.
