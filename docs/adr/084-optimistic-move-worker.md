# ADR-084: shared Rust move preview in the calculation Worker

Status: ACCEPTED FOR SAME-SHEET OPTIMISTIC MOVE PREVIEW (2026-09-30).
Native/WASM and browser convergence are locally verified. Neither durable
operations nor the engine identity change. Broader phase gates remain open.

## Context and previous decision

ADR083 made same-sheet moves durable and eligible for guarded undo/redo. Until
now the browser waited for a canonical projection refresh before showing moved
contents. A JavaScript relocation implementation would duplicate the Rust rules
for overlap, external references and partially intersected ranges.

## Decision

The existing stateful WASM Workbook receives geometry only: document ID, exact
confirmed input version, expected and next pending revisions, stable source and
destination addresses and dimensions. Its strict versioned decoder calls the
same Rust relocation planner as the server. No client AST becomes authoritative.
The private candidate keeps the original confirmed version; preview revision
identifies local intent rather than a synthetic committed event.

The Worker prepares the incremental dependency delta and clones its bounded
calculation cache. It evaluates and serializes a temporary candidate before an
infallible private swap. Validation, calculation or output-limit failures leave
the previous Worker workbook, cache and pending revision intact. This clone is
limited to a move, rather than every keypress. Its cost must be measured.

Results include rendered authored contents for the complete frozen footprint,
including blanks and affected external formulas, plus calculated changes and
the existing normalized result hash. The hash describes preview results and
does not establish a canonical server content hash. The serialized response is
capped at 4 MiB; existing geometry (1000 source cells), footprint (10000 writes),
text and calculation limits still apply. Partial range moves remain typed
unsupported. Responses carry document/input/pending stamps and request identity.

The client keeps a separate content overlay associated with one pending move ID.
It promotes only a matching completed Worker response. Confirmed cells, version
and hash remain unchanged until a durable server event or authorized refresh.
Server submission proceeds independently of Worker availability or success.
Retries retain the original command bytes and operation ID. A terminal rejection,
newer head, refresh, disposal or ordinary edit discards the overlay and resets
the Worker from confirmed content plus ordinary pending cell edits. A delayed
response cannot restore discarded preview contents.

## Consequences and verification

No wire protocol, persisted envelope, migration, dependency or engine semantic
version change is needed. Worker request schema 1 gains a separate move API; old
calculation apply messages are unchanged. Server-only undo/redo preview and
cross-sheet moves remain separate work, as do the broader phase acceptance gates.
Native failure-atomicity tests, frontend sequencing tests, delayed-delivery
browser tests and differential move fixtures must verify this decision. Actual
commands, timings and limitations are recorded in RUNBOOK rather than treated as
controlled performance claims.
