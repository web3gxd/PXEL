# ADR-067: Durable automatic volatile epochs after edits

Date: 2026-09-30. Status: ACCEPTED FOR EDIT ADMISSION.

ADR-066 initialized the first context but kept compatible contexts unchanged
after later edits. Content edits, copy/undo, and explicit structural insert/delete
now request a new persisted `VolatileDependency` epoch when the confirmed book
contains NOW/TODAY/RAND calls. The conservative policy recalculates volatility
after any such edit, including an unrelated cell. Static books acquire no event.
Automatic ExtendRows while scrolling creates empty axes and does not renew RAND.
Only a newly committed user event requests admission; idempotent receipts do not.

The actor sends the durable user ACK first. A separate prepared candidate and CAS
transaction publish the system epoch. Pending work retains its sampled ID/time/seed
and the latest required user-version watermark. Receipt lookup precedes cancellation.
An older receipt cannot cover an edit ordered after it. Changing context ID cancels
the old uncommitted sample, but does not erase newer required edits.

Recovery derives intent from the operation log rather than trusting process memory:
an authorized journal port locates the last system epoch through the recovered head,
validates its codec/checksum/event hash/type, and the actor reads the subsequent
verified events in pages of 128. A content edit after that epoch requires admission.
This handles restart after user commit before system commit and external writers.
The scan also runs when refresh/receipt/CAS retry changes head inside admission,
before marking that head inspected. It stops at the first relevant edit, checks
contiguous versions, yields between pages, and rejects after 4096 events with
typed Unavailable. Manual recalculation can establish a new epoch for a book with
an exceptionally long tail containing only automatic extent changes.

Each request admits at most two samples: resolving older uncertain work, then
covering newer edits. A remaining requirement returns Unavailable and is retried;
the original user commit is retained. Transactional tenant/ACL epochs and durable
CAS from ADR-066 apply to every system write. No public wire operation, persisted
schema change, dependency or engine-version change is introduced.

Verification includes real PostgreSQL restart-gap, pending/external-epoch/later-edit
and an injection during receipt lookup forcing CAS retry. The latter must return
version 7 / epoch 3, covering the edit at version 6. Live HTTP verifies a scalar edit
renews RAND once and a command retry does not. Browser tests cover confirmed values,
preview separation and reload. These establish edit correctness, not performance.

Timer epochs for clock consumers, compatible open-renewal policy, measured burst
admission/coalescing, timezone settings and full MVP gates remain subsequent work.
Normal consecutive edits currently produce separate epochs; delayed uncertain work
may coalesce. Formula inputs and calculated-cache schemas remain unchanged. Existing
tests expecting fixed context across edits must assert the new ordered epoch events.
