# ADR-113: recognition and separate redacted projections

Status: ACCEPTED — implemented and locally verified (2026-10-06).

Owner-local recognition now runs automatically when the enabled policy, confirmed
head or disclosure revision changes. Policy reads are cached by document and
disclosure revision. The local mask is a rectangular canvas-aligned veil with
subtle green motion; hover/focus reveals only a current confirmed scalar already
authorized for the owner. Escape, pointer leave and blur hide it. Originals are
absent from the resting veil DOM. Recognition outlines are removed. A clipped
outer rectangle covers every visible fragment; the inner label/effect keeps full
cell coordinates, so scrolling clips content rather than recentering it. Reduced-motion preferences disable animation.
Guest/redacted projections cannot reveal originals; clipboard and exports retain
the server's safe values. A local visual reveal grants no new access rights.

Passport recognition accepts formatted or compact ten-digit values with context
anywhere in the same row or column of the same sheet, including labels below or
after the value. It does not require adjacent labels. Header context does not
relax digit count, word boundaries or the nontrivial-value check. Context analysis
has bounded work and shares all redacted delivery paths.

Bulk false-positive exclusions reuse DTO 1 exact-content/address/type fingerprints
and existing durable policy CAS. The owner can check findings, choose all, exclude
the current grid selection or enter an A1 range. The complete deduplicated batch
is rejected before saving if it exceeds 1000 exceptions. Ranges stay on the active
sheet and validate both endpoints; changed content no longer matches an exception.
Each successful policy change retains guest invalidation and automatic analysis.

Recognition is off by default and owner-controlled per book. Encrypted policy
DTO 1 stores enabled types and up to 1000 exact-content/address/type fingerprint
exceptions. Changing it uses head/revision CAS, synchronous durable commit,
revises the existing book disclosure stamp without changing its rules, increments
access_epoch and revokes guest sessions. Workbook head/context/history remain
separate. New anonymous links atomically create the encrypted link disclosure
policy with the link and bind their creation mode into idempotency checks.
Only view/comment roles are offered for those links.

The pure domain scanner uses bounded UTF-8 spans, INN/SNILS/Luhn checksums and
passport format/context heuristics. Matches can be false positives or misses;
the UI states this. Arrays are conservatively masked in the scalar projection.
Calculation runs on a private projection at confirmed head/context. Static
downstream closure includes potential conditional branches and range users.
Recognition and graph budgets fail closed. Original authored cells survive.

Redacted projection DTO 1 contains layout, stable axis IDs, safe scalar values,
masked sheet/style names and capabilities. It contains no authored state hash,
formula source/AST, history/preimages or calculation seed/context. The frontend
adapts its layout into an explicitly marked, read-only display projection; the
internal all-zero hash is an inert sentinel, never sent to calculation/commands
or presented as an authored hash. It does not initialize the WASM preview.
Guest WebSocket uses existing head-only hints; raw calculation requests are
suppressed. Original context/history/recalculation endpoints reject anonymous
sessions. Storage commit guards reject their cell/structural writes regardless
of transport. Comment originals remain encrypted; guest delivery masks them.

Clipboard receipt adds anonymized flag and the same safe projection, after
durable authorization audit and under the delivery lease. Safe copying issues
only TSV/HTML scalar values and omits formula/range/move payloads. Cut is refused
under mandatory anonymization. Owner local masking uses a separate overlay and
disables editing; stale overlays show a placeholder until refreshed. XLSX uses
the same analyzed scalar map. Policy changes are fenced through delivery.

Recognition sources: [FNS INN structure](https://www.nalog.gov.ru/rn77/fl/interest/inn/),
[Bank of Russia SNILS algorithm, pp. 13–14](https://cbr.ru/StaticHtml/File/117620/method_0921.pdf),
[RFC 5322](https://www.rfc-editor.org/info/rfc5322/),
[Stripe synthetic card fixtures](https://docs.stripe.com/testing).
The scanner is a bounded heuristic, not full RFC email parsing or verification
that a document/account exists. Verification and remaining gates go in RUNBOOK.

Anonymous clipboard analysis runs before the final delivery lease: actor reads
need a document write lock. Durable audit precedes preparation; the final guard
checks the same document/policy stamp again and holds the lease through delivery.
Analysis failures add a durable content-free refusal. This avoids lock inversion
without issuing data after a policy change.

Verification: two scanner tests, one separate-projection server test, one real
PostgreSQL policy/link/audit case, one hard-restart case, 10 Chromium cases at
DPR1/DPR2, and 254 frontend tests passed. Common final gates and manual NVDA
remain open; these results do not establish production deployment readiness.
