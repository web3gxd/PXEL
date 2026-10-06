# Approved PXEL additions — implementation state

The user authorized all nine consecutive vertical slices on 2026-10-05.
Existing uncommitted changes are the input baseline and must be preserved.
The full plan is in the conversation; its accepted contracts remain authoritative.

1. Disclosure policies/audit and encryption/share warnings: implementation in
   ADR-108; targeted PostgreSQL, browser and restart tests passed. Combined
   acceptance is still being recorded in RUNBOOK.
2. Frozen top rows and personal saved filters: implemented in ADR-109; event 36,
   snapshot 7, authored projection 7, WASM init 5, personal view DTO 1. Shared
   candidate/CAS and encrypted private revision CAS passed native/PG/restart
   checks. Combined baseline and 224/224 full browser cases passed (see RUNBOOK). Hidden-row geometry
   governs rendering, navigation and selection; partial vertical merges stay visible.
3. SortRange: implemented and locally verified (ADR-110); pure domain/planner/executor/codec, 2 PostgreSQL, 12 Chromium and one hard-restart test passed. One atomic candidate/CAS/event/Undo target;
   stable multi-key confirmed value sorting; header exclusion; explicit range
   expansion confirmation; formula copy rebasing; 100k cells and 4 MiB event cap.
4. Hardcode diagnostics: implemented and locally verified (ADR-111), 10 browser cases, native/WASM/PG/restart and 249 frontend tests passed. Shared Rust/WASM diagnostic spans,
   UTF-8/browser offsets and relative reference normalization; 100-formula/90%
   dominant-template thresholds; accessible dismissible heuristic warnings.
5. Profiler: implemented and locally verified in ADR-112; 12 Chromium cases, native/actor/executor, 251 frontend tests and Clippy passed. Owner fixed-version/context analysis on a separate
   projection; executor clock measurements; no new head or calculation epoch;
   500 nodes/2000 edges with explicit truncation and static potential dependencies.
6. PII and redacted guests: implemented and locally verified (ADR-113). Scanner/server/PG/restart, 10 Chromium and 254 frontend tests passed. Opt-in per-type recognition and
   checksums; originals retained; separate values/masks-only server projection,
   dependency taint, comments/sheet names, every channel and fail-closed budgets;
   only view/comment links, policy changes invalidate guest sessions.
7. Action journal/version diff: implemented and locally verified (ADR-114). Pure/PG/restart, 12 Chromium and 257 frontend tests passed. Owner paginated encrypted
   journal; separate historical projection; stable-ID structural/style/value
   comparison; no head/context/Undo/pending mutations.
8. `.pxel` v1: implemented and locally verified (ADR-115). Crypto/adapter/restart,
   12 Chromium plus 2 anonymous guest cases and 259 frontend tests passed.
   Independent password-required format; Argon2id
   v19 64 MiB/3/1 via KDF, XChaCha20-Poly1305, fresh salt/nonce; 12–128 Unicode
   characters without normalization; 32 MiB plaintext cap; preview and atomic
   new-book import with reassigned IDs. No history, users, links, comments or keys.
9. Home library: implemented and targeted locally verified (ADR-116). Native/PG
   pagination-CAS/restart, 4 library units, 12 core plus 8 keyboard browser cases
   passed. PostgreSQL catalog/current permissions,
   encrypted titles/preferences, search/recent/favorites/shared/list/cards;
   Budget/Tasks/Sales templates with actual formulas; pending-change navigation
   guard. Existing `/?document=id` and `/share/...` must remain valid.

Required-anonymization output uses ADR-113's separate safe scalar projection;
disabled recognition or exhausted budgets block it. Never substitute originals.
NVDA speech, Linux/Compose/remote CI, production auth/storage/deployment remain
explicit gates. Browser XLSX upload remains gated on sandbox/scanning/reporting.

Local verification uses only `.local/crypto-verification/acceptance-database-url`
and its private keyring, with `.local/target-merge` and two Rust test threads.
The ignored `.local/additions-env.ps1` loads this setup without printing secrets.
After the user's cleanup request, cargo clean removed approximately 119 GiB of
generated outputs. Verification now disables incremental caches and dev/test
debug symbols. `.local/additions-final-env.ps1` selects a fresh isolated final
verification database; final common gates are still being recorded in RUNBOOK.
Do not inherit the ordinary development URL by accident. Read RUNBOOK, roadmap
and applicable ADRs before the next slice, including merge ADR-096 and crypto
ADR-102. Keep confirmed content, calculated results and optimistic intents apart.
