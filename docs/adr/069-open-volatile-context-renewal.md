# ADR-069: Volatile context renewal on subscription transitions

Date: 2026-09-30. Status: ACCEPTED FOR VOLATILE SUBSCRIPTION OPEN.

ADR-066 creates the first context and ADR-068 updates active clock consumers.
A compatible RAND context otherwise survives every reopen indefinitely. Renew
the shared persisted context when an authenticated WebSocket establishes a new
document subscription: initial subscription, switching documents, or reconnect.
Repeated Subscribe for the same active document does not renew. HTTP reads,
head checks, replay refreshes and calculation polling never act as opens.

The server validates the replay cursor against an authorized head before any
Open mutation; malformed future cursors cannot create an epoch. It calls an
internal actor Open request before registering its replay
barrier. The bounded queue, 10-second wait and current authorization apply.
The actor first refreshes, records context identity, then resolves first/edit
epochs. If that step created a context, reuse it. Otherwise a volatile book
gets a trusted Open sample. Static books get no epoch. Offset changes require
explicit policy migration and are rejected rather than silently adopted.

Pending Open keeps its operation ID/sample and resolves its receipt before
checking need, including removal of consumers or a newer external epoch.
An expected-context guard supersedes uncommitted obsolete work. After advancing,
recover edits discovered during CAS refresh. Keep pending identity until that
recovery succeeds, so a post-commit failure does not masquerade as a fresh Open.
Candidate validation, transactional authorization and durable CAS remain intact.
Failure prevents establishing the new subscription; ordinary reads cannot
promote WASM previews to canonical results.

Opening another tab changes shared RAND for existing subscribers too. A new
socket after reconnect is a distinct Open; simultaneous distinct opens may
advance separately. Reconnect storms therefore create durable work, subject to
existing bounded admission/rate limits. This is not a measured load guarantee
or an exactly-once session-open identity across process crashes. Replay restores
the recorded seed/time; replay itself never samples. Durable context/data survive
reload, but RAND is intentionally renewed on the new subscription.

Verification: actual PostgreSQL11 Open cases,47 HTTP/WebSocket cases and80
browser cases at DPR1/2 passed. Tests cover receipt loss, post-commit recovery
failure, bootstrap/edit reuse, obsolete candidate supersession, CAS edit recovery,
viewer/revoked/cross-tenant access, future cursor rejection, duplicate subscription,
document switching, reload renewal and shared RAND equality across two tabs.
These are functional tests, not a reconnect-load benchmark.

No public protocol, persisted schema or engine version changes. Broader timezone,
load/cross-host/fault gates and manual Excel verification remain open.
