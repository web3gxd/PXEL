# ADR-123: atomic clearing when merging populated cells

Date: 2026-10-06. User-selected behavior: retain the top-left anchor, clear
covered content and formatting, restore them only through Undo. Unmerge alone
does not restore cleared cells. Existing conservative MergeWrite events and
import/snapshot validation retain their meaning.

The browser sends explicit clear_covered merge intent. Application freezes
the union of occupied/styled non-anchor cells, with exact content and sparse
style preimages. New MergeCellsWrite event schemas 43/44 encode ordinary and
compensating operations separately. No snapshot or authored projection codec
change is needed: committed merged state still has empty/default covered cells.
New canonical events use the existing full-projection resync path, keeping
preimages out of public wire events and confirmed/pending/results separate.

Candidate preparation clears through the existing guarded cell batch, then
validates merge membership on a private candidate. The inverse unmerges before
restoring frozen values/styles. One public event, one CAS transaction, one
version and one history target cover the complete change. Journal admission
independently verifies the head projection, exact command/inverse provenance,
membership/content/style history, candidate hash and codec before durable commit.
Stale destructive intent fails rather than clearing values the user did not see.
Formula dependency reconciliation includes every cleared/restored address.

Existing geometry limits apply; at most 1000 occupied/styled non-anchor cells
may be cleared in a single operation, within existing text/event budgets.
Oversized, overlapping, hidden-axis and malformed merges fail atomically.
Anchor content/style remain unchanged. No new dependency, service or port is
introduced. Local verification and remaining deployment gates are in RUNBOOK.
