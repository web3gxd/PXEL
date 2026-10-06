# ADR-114: owner activity journal and isolated version comparison

Status: ACCEPTED — implemented and locally verified (2026-10-06).

Activity DTO 1 reads verified encrypted journal events in descending pages of
100 through a frozen head, with a 32 MiB ciphertext page budget. Current owner
access is checked in storage and before HTTP delivery. Actor IDs are opaque;
system events retain their provenance. Footprints contain stable IDs and their
current-head labels, including an explicit deleted-axis indication.

Historical DTO 1 wraps the existing authored projection and separate calculated
values recovered by the existing snapshot/replay reader, with explicit read_only.
It has no active actor,
pending intents or editable controls. Calculations use that version's context
on a private projection; a missing volatile context fails without initialization.
Diff DTO 1 compares stable sheet/axis/cell/merge identities, individual style
leaves, sizes, frozen rows, current formula rendering and derived scalar results.
Structural reference changes explicitly carry context. No head/context/Undo
mutation or new persisted codec is involved. Reads share bounded CPU admission,
30-second recovery timeouts and final authorization delivery leases.

Diff limits are one million visited identities, 100000 changes and 16 MiB DTO.
Budget errors reject the report as a whole. The UI pages the activity feed,
shows an independent read-only historical table, and filters colored before/after
changes by category. Automated browser checks and manual NVDA are separate gates.

Formula comparison includes authored text, current rendering and an AST binding
fingerprint; binding identity changes cannot disappear behind equal A1 text.
Absent style leaves compare with canonical defaults, so adding one bold property
does not report 15 unrelated default changes. History text clipboard is audited;
mandatory anonymization refuses historical text issuance because current-book
dependency analysis cannot prove an older derived value safe.

Verified: all-category pure diff test, real PostgreSQL pagination/owner/corruption
test, one restart scenario, 12 Chromium DPR1/DPR2 cases and 257 frontend tests.
Common final gates and manual NVDA remain open. Details and initial fixture
failures are recorded in RUNBOOK.
