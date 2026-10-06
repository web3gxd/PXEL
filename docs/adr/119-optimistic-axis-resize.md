# ADR-119: Responsive axis resize preview

**Date:** 2026-10-06
**Status:** Accepted; local verification recorded in RUNBOOK.md

## Context

The grid painted only confirmed axis overrides. Dragging updated a tooltip on
every pointer event; releasing the pointer left the old geometry visible until
the serialized server command completed. Server or network latency therefore
became visible UI latency. A held-ACK browser regression reproduced an unchanged
column extent for more than five seconds.

## Decision

- Keep confirmed sheet geometry in the authoritative projection. Pending size
  intents additionally retain sheet ID, stable axis ID, axis kind and requested
  size. They never change confirmed content, version, state hash or calculation
  results. Wire commands and persisted formats retain their existing versions.
- Share a separate geometry overlay between Canvas, hit tests, editor placement,
  selection, privacy veils, frozen panes and both scrollbars. The latest pending
  size for each stable target wins. ACK removes the pending intent; a terminal
  rejection rolls it back. Retryable failures retain the same ID and bytes.
- Preview drag geometry locally, coalescing pointer moves with
  requestAnimationFrame. Cancel, lost pointer capture and unmount clear the
  transient preview. Submit one durable command on release, resolving the
  captured stable target against the current sheet rather than reusing its old
  numeric position. An unchanged effective size produces no command.
- Reuse the confirmed prefix index for size previews. A delta changes offsets
  after the affected index without copying axis arrays; offset lookup remains
  logarithmic in axis extent. Hidden rows stay hidden. The size panel reads the
  actual requested size independently of filter visibility.
- Cache pending geometry by immutable confirmed metrics and pending operation
  IDs. Content/used-range extraction keeps using the confirmed sheet identity;
  dragging never clones the entire sheet or rescans its populated cells.
- Invalidate Canvas geometry for size changes, excluding tooltip coordinates
  and unrelated sheets' pending sizes.

## Verification and limits

Store tests cover rejection rollback, pending reset, repeated sizes, stable-ID
remapping/deletion and filtered-row sizes. Geometry tests compare all offsets,
hit tests and visible ranges against a rebuilt index, and bound lookup work at
100,000 allocated rows. Chromium tests with withheld socket sends prove live
resize, local paint before durable commit, cancellation, hit testing, reload
and a second subscribed tab at DPR 1 and 2.

`node tests/performance/resize.mjs` measures pointermove to matching Canvas draw
submission on a real locally persisted book. These measurements do not measure
GPU presentation, production latency or a 100,000-populated-cell workload.
Server persistence/candidate validation, calculation and deployment gates remain
unchanged; no dependencies are added.
