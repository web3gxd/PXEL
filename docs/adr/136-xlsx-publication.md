# ADR-136: retry-stable XLSX candidate and publication

Status: implemented and locally verified on Windows, 2026-10-07; final post-rebase/deployment gates remain.

Convert independently validated XLSX IR to the existing positional Book adapter.
Preserve selected sheet names/order, scalar types, bound formulas, shared-formula
relative copies, sparse cell styles and merges. Reject invalid selection, missing
selected reference targets and domain profile overflow. Do not widen the existing
CLI's legacy1000-cell profile or reuse its random identities for public imports.
Reuse pxel_workbook preparation for candidate validation and deterministic UUID7
identities from request/document/source digest, and the PostgreSQL atomic imported
book transaction. No new storage or wire codecs, dependency or plaintext staging.

Public publication requires member/origin authorization, the verified WASI
executor and existing import/export permit. Verify exact preview source/selection
digest and require explicit acceptance of reported parser losses. Validate title,
request/document UUID7 and all candidate operations before any durable creation.
Publication acknowledgement follows durable commit and retries use the same
identity/content. Browser controls, candidate/function reporting and publication/retry/restart E2E
are locally verified in RUNBOOK. Complete Excel compatibility, Linux and public
deployment are not established by this bounded profile.

Preview adds optional title: omitted title retains the ADR-135 digest, while
publication preview binds the exact converted positional Book (including title,
engine/profile, values, styles, merges and settings) and both compatibility
reports to the existing source and selection hash. Canonical serialization is
streamed directly into SHA-256 without a second plaintext bulk buffer.
Publication requires this candidate-bound digest. Conversion also
expands selected axes for valid references to empty cells, within domain limits.
Unknown functions are grouped by selected original sheet/name with affected-cell
counts (maximum128 report groups). Probe the actual pure evaluator dispatcher
with an empty argument call, never execute uploaded arguments; #NAME identifies
an unsupported name. Known volatile functions may report missing calculation
context and remain supported names. No duplicated function catalog is introduced.
Report names only after candidate binding validates their grammar/128-byte bound.
The existing accept_reported flag also gates these warnings; original formulas
are retained, and computed results remain independently derived.

Full-profile testing found the ADR-131 500 million fuel cap rejected a valid
100000-cell scalar package before candidate preparation. Measured synthetic
probes exhausted500M/2B fuel but passed5B in572ms with the same256MiB guest
memory/5s timeout. Set the bounded production guest fuel budget to5B; retain
512MiB host memory, one-core CPU, task caps, process/transfer deadlines and all
filesystem/environment/network denial. Add full-profile real configured-backend
and HTTP/PostgreSQL acceptance, plus existing malicious-loop fuel/timeout
positive controls. This adjustment is not a general performance SLA or Linux
verification and must pass combined regression before becoming tested baseline.

Full-profile HTTP preview then exceeded the fixture30s timeout because all
100000 scalar targets were split into128-target events, repeatedly hashing each
candidate. The XLSX-specific preparation entry point uses existing domain
1000-target batches for unstyled scalars only. Any formula/style batch retains
128 targets and256KiB source-text budget; durable event size validation and
candidate/hash guards remain unchanged. Legacy PXEL/operator preparation keeps
its128-target event shape, preserving its retry contract. Both XLSX preview and
publication use the same XLSX preparation profile. HTTP fixture preview timeout
now matches the existing browser60s budget; performance/recovery must pass before
claiming the full profile usable.
