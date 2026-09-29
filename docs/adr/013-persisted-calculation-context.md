# ADR-013: Persisted calculation context and system event

Date: 2026-09-29. Status: partial implementation in phase 2. Persistence,
system principal, replay, explicit trigger and context-aware delivery are
implemented; volatile native/WASM evaluation and snapshot recovery are open.

Update2026-09-30: ADR-064 closes authenticated HTTP/Worker transport of the full
context, including schema3 hydration and retention across patches. Existing
snapshot-image tests cover context retention, but production snapshot/recovery
matrix remains a broader gate. Volatile evaluation/open/timer triggers remain
open; context transport alone does not enable NOW/TODAY/RAND.

## Context

The present evaluator implements only nonvolatile formulas. Calculation
responses identify epoch 0 with the document ID, which is sufficient to reject
stale nonvolatile results. `NOW`, `TODAY` and `RAND` require a reproducible
clock and random source. Sampling time or seed during replay, or storing them
inside every user operation, would make a committed document depend on process
timing. The current durable event model requires `actor_id` for every event;
using a fabricated user for an internally triggered recalculation would make
authorization and audit history false.

## Decision

Add a validated `CalculationContext` to the workbook projection. It contains
context ID, positive monotonically increasing epoch, UTC instant, explicit
civil date/offset or versioned timezone rules, 32-byte seed, engine version,
compatibility profile and reason. The first context is created by a durable
`AdvanceCalculationEpoch` system event. Documents with no such event retain
the legacy epoch-0 calculation identity and their existing domain hash.
Every later context is a new document version and part of the canonical
authored-state hash through a versioned context-hash extension. Calculated
values remain derived and absent from that hash.

System events have an explicit `EventPrincipal::System` distinct from
`EventPrincipal::User(actor_id)`. A storage schema 3 envelope and an additive
PostgreSQL migration will represent this distinction; legacy schema 1/2 rows
continue to decode as user events and retain their hash goldens. The database
must enforce the principal/actor combination with a check constraint. No
client wire command may contain `AdvanceCalculationEpoch`. An authenticated
explicit recalc request authorizes the trigger, but its durable event has a
system principal with optional audit requester ID. Timer/open triggers use
internal capabilities only.

The document actor serializes context advancement with user commands. A
trigger obtains a stable event ID, timestamp, civil date and seed **once**.
Candidate application validates `epoch = previous + 1` and the engine/profile
before CAS commit. CAS or serialization retries reuse the same sampled inputs
and reprepare against the new head; they do not silently resample. If the
commit outcome is uncertain, operation-ID receipt lookup resolves it before
another context is created. Multiple opening clients may coalesce one pending
trigger. Replay and snapshot recovery never sample new context data.

Calculation results are identified by `(document version, epoch, context ID,
engine version, profile)`; changing any component supersedes older results.
The native evaluator and WASM worker receive the same context through bounded
initialization/patch messages. The browser accepts server results for its
current context only. Volatile functions remain disabled until this complete
path is implemented and differential tests prove native/WASM convergence.

For `RAND`, a versioned SHA-256 derivation uses document ID, epoch, formula
cell ID, stable AST node ID and seed; the specified first 53 bits map to a
binary64 value in `[0, 1)`. Two `RAND()` calls in one formula therefore differ,
and worker traversal order cannot change either result. `NOW` and `TODAY`
read the stored instant/civil date; the pure evaluator never reads the system
clock. Timer scheduling is needed only while a workbook has volatile consumers.

## Required verification before acceptance

- Schema 1/2 replay and historical domain/event hash goldens are unchanged.
- An epoch event survives restart; snapshot plus tail and full replay produce
  the same context and domain hash.
- Two concurrent trigger attempts produce one legal monotonic event each or
  coalesce, without duplicate IDs or version gaps.
- A forced CAS retry and an uncertain-commit retry preserve sampled inputs.
- No client operation or untrusted AST can forge a system principal.
- Stale calculation patches are ignored across epoch/context changes.
- Native and browser WASM evaluate volatile fixtures to the same normalized
  result hash, including multiple `RAND()` nodes and UTC/day boundaries.

The first implementation slice should persist and replay the context with
these invariants before enabling volatile functions or timer triggers.

Update2026-09-30 ADR-065: explicit-epoch NOW/TODAY/RAND evaluator реализован; смена контекста dirty-all с budget, native snapshot restoration сохраняет calculated hash. Automatic open/timer/dependency admission и общая production snapshot/recovery matrix остаются открыты.
