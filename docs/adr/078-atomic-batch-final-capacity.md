# ADR-078: validate atomic batch capacity against final contents

Status: ACCEPTED FOR THE BATCH CANDIDATE FIX (2026-09-30).

## Context

Preparing guarded range history exposed an existing correctness problem in
`preview_candidate`, also used by durable value batches and frozen copies.
At the workbook text, AST or cell-count limit, adding a destination before
clearing another destination could reject a batch whose final state fits.
Reversing independent destination writes could therefore change acceptance.
Range inverse application must not inherit this order-dependent behavior.

## Decision

Validate destination uniqueness and guarded write metadata/preimages against
the original workbook. Clone once, release all replaced destinations inside
that private candidate, then validate and apply their final contents. Formula
origins, bound references, source budgets and per-cell limits remain enforced.
All intermediate candidate mutations are private; any error discards the whole
candidate. No publication or database write occurs before successful validation.

Guarded writes use the existing content/preimage validator, shared with ordinary
single-cell candidate validation. After its original preimage check, application
uses the equivalent value/formula/clear operation on the private candidate.
A compensation cannot target a version newer than the original workbook head.
Changing the candidate's projected version does not authorize a future target.

This changes no persisted event representation, protocol or canonical hash
format. Previously valid event replay retains the same final content and hash.
Previously rejected batches whose final allocation fits may now be accepted.
It does not implement persisted batch preimages or expose range undo/redo.
Those remain the next phase-4 slice, including new immutable event schemas,
transactional per-address history guards and atomic conflict rejection.

## Verification

Regression tests first reproduced LimitExceeded for copy, value batch and guarded
preview at the text limit. Tests now require equal forward/reversed final hashes,
original-state preservation on failure, cell-count capacity release at100,000
cells, formula AST capacity release, exact preimages and formula origin/version
validation. Native suites, browser/native parity, live persistence regression
and the existing1000-value candidate benchmark provide additional evidence.
Actual commands and outcomes are recorded in RUNBOOK; deployment and full MVP
gates remain open.
