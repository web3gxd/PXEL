# ADR-103: resumable KEK rewrapping

Date: 2026-10-02. Status: IMPLEMENTED; local Windows combined acceptance passed, including interrupted-rotation and restore-generation tests. Linux/Compose/remote CI remain deployment gates.

`pxel-server keys rotate --to <key_id>` authenticates every retained DEK wrapper,
reserves fresh nonces for the target physical KEK, rewraps and checks the new
wrapper before committing each row with a wrapper-revision CAS. A final read
checks the committed bytes. The data ciphertext, DEK generations and workbook
versions stay unchanged. The target must exist in the explicit configured
keyring; missing targets produce no wrapper/state/nonce mutations.

A PostgreSQL exclusive transaction advisory lock fences supported document
writers and key creation. Transaction lifetime releases the lock on rollback,
connection failure or cancellation. A bounded keepalive prevents idle timeout
while independent wrapper commits run. Writers use the shared side on their
main pool; a separate crypto pool remains available for nonce and wrapper
transactions. Nested genesis calls use their already held shared fence.

Each verified wrapper commit is durable progress. Interruption keeps these
rows while leaving the old active KEK in `crypto_key_state`; rerun verifies and
skips completed rows. The durable active KEK and its first rotation timestamp
switch only after every retained wrapper passes. New documents read this
durable state, rather than relying on the keyring's bootstrap active label.

Load both old and target keys in serving processes before rotation. A process
with an obsolete old-only keyring fails closed after rewrapping. Retained
backups may still require old KEKs; rotation does not authorize removing them.
Restore-specific fresh write DEK/KEK generations remain a separate gate.

Four initial PostgreSQL tests passed: unchanged journal bytes and retained
generations, interrupted resume, missing target without changes, and refusal
under an active writer with lock release. Final combined results are in RUNBOOK.
