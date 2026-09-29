# ADR-057: Bounded historical axis binding

Date: 2026-09-29. Status: ACCEPTED FOR THE LOCAL BINDING SLICE.

## Context

The structural slices rejected every formula authored before the latest axis
change. Binding its A1 text at the current head would silently change its
meaning. Client ASTs cannot be trusted as canonical input.

## Decision

Each active actor keeps axes-only checkpoints at structural changes. A delayed
formula is parsed and bound on the checkpoint covering its base version, then
transformed through later structural transitions using the domain reference
transformations. Insertion preserves targets; deletion invalidates scalar
targets and contracts ranges to surviving endpoints. Deleted origins are typed
errors. Original source, canonical authored text and base version remain stored;
current A1 text is rendered from the transformed bound AST.

The cache admits at most 10,000 versions of lag and 16 MiB of conservatively
accounted entries, including axis IDs, names and transition buffers. It contains
no cell contents. Binding has a 1,000,000-unit work budget charging axis hydration,
AST transformation and range deletion scans. These are implementation limits,
not a guarantee that all commands within 10,000 versions can be rebased.
Evicted history or exhausted work returns `RebaseWindowExceeded` and requires
resync/retry. Oversized checkpoints disable historical binding until a usable
checkpoint is available. Current-axis binding remains available.

Candidate checkpoints and queue capacity are prepared before the CAS transaction.
After durable commit, publication moves prepared buffers. CAS refresh/replay
also updates the cache. PostgreSQL events remain the source of truth; the cache
is disposable. A recovered actor starts at its recovered head, so history
crossing earlier structural changes may require resync. Durable historical axis
hydration and advertising the actual rebase floor are separate recovery work.

Wire formula text does not carry the complete bound AST. A delayed formula
therefore uses the existing projection-resync path: `transformed=true` and no
inline canonical operation. Replay uses its existing barrier fallback. This
avoids the browser Worker rebinding old A1 text at a new head. Fresh formulas
keep incremental delivery. No wire or persisted schema migration is required.

## Consequences and verification

This supersedes the conservative stale-formula guard described in ADR-019/021;
historical stored events are not rewritten. Copy/move/undo semantics are unchanged.
The cache is bounded per actor, not globally across active documents. Full
projection refresh for delayed formulas is a known bandwidth/performance cost.
The work budget is conservative accounting, not an OS CPU or memory sandbox.

Unit coverage includes mixed references, range deletion, deleted origins,
restart rejection and eviction under 1,200 structural changes. Real PostgreSQL
integration covers delayed binding across insertion/deletion and idempotent
retry. The isolated release harness measures historical binding, not commit
latency: [conditions and results](../benchmarks/2026-09-29-historical-formula-binding-local.md).
The dedicated browser case holds the original binary formula frame while a
second tab commits an insertion, then releases it to the real server. At DPR
1 and 2, the original tab converges without reload to B2=12 and current source
`=(A2+1)`; the second tab retains the result after reload. The preview marker
clears after server confirmation. The harness uses the documented
[WebSocket interception API](https://playwright.dev/docs/api/class-websocketroute),
forwards traffic to the actual server and does not synthesize acknowledgements.
