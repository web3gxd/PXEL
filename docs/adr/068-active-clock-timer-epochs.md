# ADR-068: Timer epochs for active clock consumers

Date: 2026-09-30. Status: ACCEPTED FOR ACTIVE TIMER ADMISSION.

ADR-067 advances volatility after edits. NOW/TODAY also need time-based updates
while a document is open without edits. The existing authenticated WebSocket
heartbeat (15 seconds, missed ticks skipped) now enqueues an internal timer request
only for its active document subscription. Bounded actor try_send drops admission
when full; another heartbeat retries. There is no new background task, retained
session principal, public wire command or unconditional document scheduler.
Closing/unsubscribing the socket stops its requests and preserves actor idle expiry.

The actor refreshes and reauthorizes the identity, resolves pending edit epochs,
then inspects normalized clock function calls. Static and RAND-only books acquire
no timer epoch. NOW is due once sampled UTC time is at least 60 seconds after the
persisted context; TODAY-only is due when civil day changes. With both, either
predicate suffices. Lazy branches are classified conservatively. Backward time
and a not-yet-due interval cause no epoch; arithmetic is checked.

Current server sampling uses explicit UTC offset 0. A different persisted offset
must not silently adopt that policy: a fresh timer sample with mismatched offset
returns a typed error. General workbook timezone settings remain subsequent work.
Calculation engine still reads only injected, persisted context, never host time.

A pending timer retains its sampled ID/context. Receipt lookup precedes checking
expected context ID, due policy, or disappearance of clock consumers. A concurrent
epoch supersedes uncommitted timer work. Candidate application, ACL epochs, CAS and
durable commit precede publication. Multiple subscribers coalesce through persisted
timestamp; queueing a tick is not a commit or permission lease. After advancement,
existing journal recovery resolves any later content edit discovered by refresh/CAS.
A post-commit recovery failure retains durable history and edit requirement rather
than rolling back the timer. No event/codec/wire/engine version change is needed.

Actual PostgreSQL tests cover threshold/subscriber coalescing, TODAY midnight,
RAND-only no sampling, lost commit response with a later clock but same pending ID,
removal of the clock formula before retry, and an external epoch followed by an edit
during timer CAS. Native calculation is checked against injected time. A real-browser
NOW test waits for the production heartbeat/cadence without changing user input;
the resulting server epoch/value arrives by subscription. This is functional evidence,
not a timer-load benchmark or a hard realtime guarantee.

Initial classification-only [measurement](../benchmarks/2026-09-30-timer-classification-local.md)
covers warm scalar-heavy books through100k cells. Initial real-PG actor queue
[burst measurement](../benchmarks/2026-09-30-timer-admission-local.md) covers two
1000-cell profiles and bounded admission; real subscriber traffic is not measured.
ADR-069 implements compatible open-renewal including RAND. Further load profiling,
timezone settings, manual Excel corpus and full production gates remain open.
Normal cadence can be delayed by bounded queue/backpressure and actual DB commit;
missed periods are not replayed as a burst of artificial epochs.
