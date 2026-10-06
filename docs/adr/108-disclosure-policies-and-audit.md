# ADR-108: disclosure policies, durable attempts and clipboard delivery

Date: 2026-10-05. Status: IMPLEMENTED; combined local acceptance in progress.

The prior document projection advertised export to every member while the XLSX
endpoint admitted owners only. Clipboard paths had neither an owner policy nor
a durable audit. This is the first vertical slice of the approved additions;
sorting, personal views, diagnostics, profiling, redaction, history/diff, `.pxel`
and the home library remain subsequent slices.

PostgreSQL holds independently versioned schema-1 disclosure rules for a book
and each share link. Absence of a row means the explicit defaults: copy allowed;
book XLSX format enabled; export for other participants disabled; link formats
empty and recipient export disabled. An owner may disable a book format too.
Effective rights require a current readable role, book rules and, for guests,
link rules. Cut additionally requires an editable role. Policy never grants
editing. A guest cannot substitute a member route or a different link identity.

Rules are encrypted with the existing document DEK and authenticated record
envelope (purpose `disclosure-policy`, scope UUID, policy revision and codec 1).
They do not enter the workbook hash, version, calculation context or Undo/Redo.
The independently versioned DTO is not a new wire command or durable workbook
event. Policy update locks the durable document, checks owner rights and performs
revision CAS. It bumps the existing durable access epoch and invalidates affected
guest sessions in the same transaction. Reopening a link requires fresh admission.
Book policy changes invalidate all guests; link changes invalidate that link.

Schema-2 capabilities add `can_copy`, `anonymized` and a disclosure stamp. The
stamp identifies confirmed document version, book revision and link revision.
The browser uses its current confirmed version after canonical operations, rather
than the version at the last capability fetch. Policy DTO counters currently use
JSON integers; the browser rejects unsafe integers and fails closed. Existing
workbook versions retain their decimal-string contract. `anonymized=false` is
truthful in this slice: no redacted guest projection exists yet.

Endpoints:

- `GET/PUT /api/documents/{id}/disclosure-policy`, optional `link_id`; owner only.
  PUT requires schema 1, `expected_revision`, allowed Origin and strict rules.
- `POST /api/documents/{id}/clipboard/authorize` and the corresponding
  `/api/shared/{link}/clipboard/authorize`; schema 1, Copy/Cut and expected stamp.
- Owner-only paginated `GET /api/documents/{id}/audit`, up to 100 records;
  opaque UUID cursor, stable descending event ordering.
- Existing member XLSX export and guest `/api/shared/{link}/export.xlsx` use
  the same effective policy and existing XLSX serializer.

Before regulated release, an attempt records action, format, principal/link,
decision, bounded reason code, version and both policy revisions. It contains no
values, formulas, clipboard content, fingerprints or secrets. An append-only
trigger refuses updates/deletes. Audit failure prevents release. The existing
encrypted prepared-export audit is retained for members. Export has an admission
decision before conversion and a final decision after conversion; both are
attempt records, not a guarantee that download succeeded.

Conversion operates on a fixed confirmed projection. A changed policy or head
rejects the prepared bytes. Final delivery holds the existing process-local gate
and durable PostgreSQL delivery lease through the response body. Policy/ACL
updates acquire the conflicting durable document lock, so fencing also works
between server processes. Authenticated attempts with a removed ACL are audited
where the document remains in the principal's workspace; unknown or unauthenticated
identities do not manufacture audit principals. Request authorization follows
the [OWASP authorization guidance](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html).

Clipboard events cannot asynchronously fill native `clipboardData`. Instead,
the handler prevents default and supplies promise-backed `ClipboardItem` values
within user activation. Those promises only produce bytes after durable audit.
The existing TSV/HTML/formula/range serializers are reused. Internal metadata is
encoded as an escaped attribute on the HTML table; legacy custom MIME payloads
remain readable. Parsing uses an inert template, without resource loads. Failed
authorization never creates a Cut intent; failed clipboard write also leaves it
unset. Selection/version changes during authorization reject the release.
Formula-bar/editor text and visible comments use the same authorization path.
Policy manages PXEL actions and cannot prevent screenshots or revoke copies
already obtained; the UI explicitly explains this boundary.

`require_anonymization=true` is enforced conservatively: all regulated release
is refused with `anonymization_required` until a later verified safe projection
can satisfy it. This flag alone does not mask a guest's ordinary document view.
The UI says that values/formulas/comments are issued in their original form
when creating a current link. No successful anonymization is claimed.

Encryption wording now explicitly says “Зашифровано при хранении” and lists
events, values/formulas, Undo data, snapshots and comments. It says the server
decrypts them for work and calculation. Share warnings precede creation and show
chosen role, lifetime and original-data mode. The four lifetimes, mandatory
indefinite confirmation and idempotent create/revoke are retained.

The common native modal now contains focus at both Tab boundaries and restores
focus after Escape according to the [APG modal pattern](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/).
Real Chromium tests cover DPR1/2, 390/1440 and light/dark. Automated accessibility
assertions and actual NVDA speech are distinct gates; NVDA speech was not exercised.
Actual checks and intermediate failures belong in RUNBOOK. Linux/Compose/remote
CI, production authentication and deployment remain open.

Unmapped DOM/editor text and text-input Cut use the same audited clipboard guard. Credential
dialogs are explicitly classified separately; historical content and title inputs
retain their own guards. For unmapped dialog text, mandatory anonymization fails
closed because a scalar can depend on sensitive cells without containing their
text. Cut changes the controlled input only after a successful clipboard write.
This includes formula-bar source and selected calculation-status text outside
dialogs. Safe range copying retains its server-derived dependency masks.
