# ADR-137: self-hosted local accounts and open-source publication

Date: 2026-10-12. Status: implemented; verification is recorded in RUNBOOK.

The user chose AGPL-3.0 and author `web3gxd`, then specified first-run onboarding
for LAN self-hosting. A master account provisions users and has full access to
every instance book. This expands the earlier source-only deployment preparation
request. Actual production hosting remains an open gate.

## Account boundary

One local instance owns one workspace and exactly one master. A bootstrap token
is generated on the server, read from a private bounded file and compared by hash.
First setup locks the database boundary, refuses existing books and creates the
instance/master atomically. There is no public registration or automatic adoption
of old development data. Dev authentication refuses initialized local instances.

Local accounts and sessions live in PostgreSQL. Passwords reuse the existing
bounded Argon2id worker; sessions use independent random opaque tokens, hashes
at rest, credential epochs and an absolute eight-hour lifetime. Cookies remain
Secure/HttpOnly/SameSite=Strict. Temporary-password users cannot access workbook
or admin APIs until changing their password. Wrong-password counters are durable;
reset, password change and disable revoke sessions. Existing per-send membership
validation also applies to local sessions.

All local accounts share the instance workspace. A database trigger rejects new
documents outside it and adds the master as owner when another creator's owner
permission is inserted. This uses existing document ACLs, durable commit rules
and protection against owner removal. It does not give ordinary accounts access
to each other's books. The master cannot be disabled or demoted.

Authentication HTTP DTOs have `schema_version: 1`; unknown versions fail before
mutation. Admission precedes reading a bounded 4 KiB body with a five-second
deadline. User-facing errors do not expose credentials, hashes or account existence.
The UI adds welcome/setup/login/password-change screens and master account
management. Auth failures refresh the account gate. Guest link routes keep their
separate authorization flow.

## Publication and independent review

AGPL-3.0-only metadata, the unmodified full license, NOTICE and CITATION.cff
record authorship. Scientific citation is requested without imposing extra terms.
The browser exposes license, warranty and source links. Modified deployments
must configure a link offering their Corresponding Source. Third-party notices
retain their original licenses.

An independent security subagent reviews account boundaries and import admission.
The `.pxel` JSON body previously preceded authentication and shared admission;
both now occur before bounded parsing. Blocking decode work holds the permit
through preview/commit. The review includes HTTP rejection before large/stalled
bodies, durable account state and session revocation.

Publication hygiene rejects private/generated paths, requires licensing metadata
and scans Git history with Gitleaks. Its one historical formula-prose false positive
is suppressed by exact fingerprint only. A pinned source-map-js transitive patch
closes GHSA-68fv-2mgg-jv7q; no new runtime dependency is introduced.

## Limits

No server or domain has been selected. Linux, remote CI, trusted HTTPS, operational
backup/recovery, load and accessibility gates remain open until actually verified.
No production readiness claim follows from source publication. The modular
monolith, confirmed/calculated/optimistic separation, PostgreSQL commit contract
and independent wire/storage versioning remain unchanged.
