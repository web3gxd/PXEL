# ADR-106: secure guest sharing

Date: 2026-10-03. Status: IMPLEMENTED; local Windows acceptance passed.

Owner routes create, list and revoke shares under
`/api/documents/{document_id}/shares`. Creation requires a UUIDv7 request ID,
one of `view`, `comment` or `edit`, a bounded lifetime and exactly one secret.
PINs are 6–64 ASCII digits (the UI generates six); passwords are 10–128 Unicode
scalars, preserved without trimming or normalization. Indefinite links require
an explicit API confirmation and the matching UI checkbox.

The server creates a random 256-bit link token and returns it once in
`/share/{link_id}#<token>`. It is kept out of the path and query, and only its
SHA-256 hash is stored. PIN/password verifiers use Argon2id v19 with
`m=65536 KiB,t=3,p=1` and a unique 16-byte salt; the PHC verifier is encrypted
at rest. HTTP share admission and Argon workers are both bounded to two active
tasks; a permit is acquired before `spawn_blocking`.

Attempt state is serialized in PostgreSQL per link. Five consecutive failures
block attempts for 15 minutes; the third completed lockout disables the link
and commits an owner notification. Success resets the counters. Public failures
for invalid, unknown, expired, disabled and revoked links are indistinguishable;
temporary lockout reports only a bounded countdown. Attempts, opens, lockouts,
disablement, revocation and expiry have append-only audit records without
credentials.

Guest endpoints stay under `/api/shared/{link_id}/`. A guest principal has one
document and explicit capabilities; it cannot use member endpoints, create
documents, administer shares, export, or read historical canonical events and
other authors' preimages. Guest editor undo is restricted to that actor. The
guest cookie is random, HttpOnly, SameSite=Strict and scoped to that link's API
path; PostgreSQL stores only its hash. `Secure` is omitted only when the explicit
development-auth mode is enabled on the loopback bind; other modes always set it.
Its token hash uses the same textual hex bytes that authentication hashes.

Revocation commits under the document delivery write gate before advancing the
gate epoch, clearing queued delivery and waking connected guests. PostgreSQL
publishes committed access-epoch changes with transactional `NOTIFY`; each
serving process listens, closes affected guest/document WebSockets and advances
its local gate. A reconnect gap closes active subscriptions before listening
resumes. Outbound document payloads hold a shared authorization row-lock lease
through the bounded socket send, so a remote access-epoch mutation cannot commit
mid-send. Expiry timers and final send checks enforce deadlines without relying
on heartbeats. HTTP guest responses are also delivered under the gate.

Windows verification: storage share tests passed 11/11; Rust Clippy (both
profiles), all workspace Rust tests/build, wasm32 and proto checks passed;
`just sqlx-check` and `just audit` passed; server unit tests passed 49/49.
Chromium E2E passed at DPR1 and DPR2: create a comment link, open it from
a context without member login, confirm editing controls are disabled, add a
comment, revoke the link and observe the idle guest close. Additional UI
acceptance covers server-returned expiry display, PIN paste across six cells,
keyboard-operated TTL and ARIA value text, dialog focus containment/return,
separate clipboard copies, required indefinite confirmation and its warning
badge, light/dark system palettes, and no external HTTP(S) asset requests.
All three browser cases passed on both DPR1 and DPR2 (6/6). The E2E uses the
loopback development-auth mode and does not establish production owner auth,
Linux/Compose, or remote CI. A separate two-process PostgreSQL acceptance test
proves idle guest WebSocket revocation across server instances; the Chromium
sharing suite again passed 6/6 at DPR1/2 with the terminal close behavior.

On 2026-10-05 the full local Chromium suite passed 198/198 at DPR1 and DPR2.
The owner sharing controls now remain reachable when the document menu reaches
its constrained width: menu items wrap instead of being covered by the sharing
actions. The live acceptance also exercised comment sharing, real comment entry,
guest revocation, TTL/PIN/password controls, keyboard focus, indefinite-link
confirmation and local-only assets. This remains loopback development-auth
coverage; production owner authentication and external deployment gates remain
open.

The WebSocket owns a strong reference to its delivery gate for the entire
connection. A watch receiver alone does not retain the sender, and the gate
directory deliberately stores weak references. This is essential for anonymous
guests without a normal document subscription and with no owner connection.
Committed epochs notify only on a strictly increasing value. Regression tests
cover a fresh guest login after closing the owner editor, safe PXEL export, and
policy revocation while the owner remains offline; results are in RUNBOOK.
