# ADR-066: Automatic first calculation context

Date: 2026-09-30. Status: ACCEPTED FOR BOOTSTRAP ONLY.

ADR-065 made volatile evaluation deterministic but required manual first recalc.
The next actor increment creates a durable context automatically after a newly
committed volatile formula, or when an authorized current-projection read finds
a legacy volatile workbook without compatible context. Existing compatible
context is reused; reads/replay do not sample new time. Static legacy workbooks
do not acquire an unnecessary system event.

The server injects a trusted context sampler into the session directory; the
formula/domain crates still perform no clock or random IO. Explicit and automatic
triggers share the server sampler. One pending bootstrap stores ID/time/seed and
is retained across uncertain commit failures. Preparation/CAS retries reuse it.
Before every attempt, authorized receipt lookup resolves the saved ID and refreshes
the projection if that lookup observed a newer head. Only then is bootstrap need
checked against the current workbook. Another compatible context or removal of the
last volatile formula cancels uncommitted work without creating another epoch.
An authorized read can initialize a viewer's volatile document as a system action;
explicit recalculation remains editor-only.

SystemCommitRequest now carries authenticated trigger identity and observed ACL
epochs. PostgreSQL rechecks access with the same locked authorization path as
user commits, before idempotency or head CAS. Explicit reason also requires editor.
This closes the previous preflight-only ACL check for system triggers. Neither
trigger identity nor reason can be forged by a public wire operation.

User operation ACK confirms only its own durable event. Bootstrap is a subsequent
separate version and may produce a head-resync hint. A bootstrap failure does not
rewrite/reject an already committed user event. Its pending sampled request remains
available for the next authorized read; failed calculation never becomes a fake value.

Detection inspects normalized function calls, including conservative lazy branches;
it never uses formula-text substring matching. Known compatible context short-circuits
the scan. This increment does not yet advance epochs on every volatile dependency
change, reload of a compatible active book or timer. Those remain required next
work, along with measured coalescing/admission under load.

Verification uses real PostgreSQL with an injected BEFORE INSERT failure:
pending work is superseded by another committed context, cancelled when the
last volatile formula is removed, or resolved by its already committed ID.
A test-only forwarding journal places a same-ID real commit between refresh and
receipt lookup and checks the returned projection includes it. Production writes
are never replaced by a mock. Native role/epoch/revoke/cross-tenant tests cover
transactional system authorization. Live first edit and eight concurrent reads
create only one bootstrap event; browser reload retains its context and value.

The previous decision in ADR-065 required manual first recalc. The change removes
that prerequisite for new/legacy volatile books; no history/schema migration and
no new formula engine identity is needed. Compatible contexts remain unchanged.
