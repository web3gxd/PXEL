# ADR-115: independent password-protected PXEL file v1

Status: ACCEPTED AND LOCALLY VERIFIED (2026-10-06).

The file lifecycle is independent of wire, snapshot, journal and XLSX codecs.
V1 carries current sheets, rendered formulas, sparse formatting, merges, axis
sizes, frozen rows and the exporting user's portable display/diagnostic settings.
It excludes old versions, comments, users, permissions, links, credentials,
server keys, AST identities and calculation seeds. Import creates a new book
with fresh identities/bindings and safe disclosure defaults. Preview completely
validates before showing composition; publication includes personal settings
in the same durable transaction. Invalid inputs never leave a partial document.

Header: magic `PXEL\r\n\x1a\n` (8 bytes), BE u16 version 1, algorithm byte 1,
reserved byte 0, BE u32 plaintext length, 16-byte salt, 24-byte nonce. The
56-byte header is AEAD associated data; ciphertext has a 16-byte tag. Length
must match exactly and plaintext is capped at 32 MiB before KDF/decryption.
Argon2id v19 uses fixed 65536 KiB / 3 passes / 1 lane / 32-byte output through
hash_password_into (the Argon2 0.6 KDF API). Every export generates salt/nonce.
XChaCha20-Poly1305 uses the already pinned 0.11 library. Passwords contain
12–128 Unicode scalar values; spaces and combining sequences are preserved.
Wrong password and failed authentication share one error. Unknown versions,
algorithms or reserved flags fail before cryptographic work. Passwords occur
only in POST bodies, are held in zeroizing memory and never logged or persisted.

Source: installed pinned Argon2 0.6.0 documentation (`src/lib.rs`, Key Derivation)
and [upstream KDF documentation](https://docs.rs/argon2/0.6.0/argon2/).
See [XChaCha API](https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/).
Passwords/file data share bounded worker admission with existing password work.
Exports apply book/link format policy, durable attempts and a final stamp lease.
Anonymous files contain only the existing safe scalar projection and safe settings.
Browser XLSX upload remains gated; this authenticated bounded JSON file parser
does not enable arbitrary ZIP/OOXML parsing or its unfinished sandbox gate.

The position-based adapter validates all sheet shapes, formulas, style leaves,
sizes, merges and portable settings before preview. Fresh identities include
the request ID and encrypted file digest, so retries are deterministic and a
different file cannot reuse the same intent. Content/style batches contain at
most 128 targets and 256 KiB authored text; every durable event remains within
4 MiB. Import admits 100000 content/style addresses, 1M allocated axes and
96 MiB encoded journal bytes. The independent decrypted file remains 32 MiB.
Book, owner permission and encrypted personal/privacy settings commit together.
Passwords zeroize even on early authentication/origin refusal; KDF admission
permits survive request cancellation. Import HTTP routes alone admit 48 MiB
base64 JSON; ordinary endpoints retain their existing 1 MiB body budget.

Anonymous files drop personal filter predicates and old diagnostic exceptions,
which can reveal original sensitive text; composition explicitly counts omitted
filters. Safe sheet/font/title strings use the same scanner. Formula-derived
values use the existing potential-dependency mask. Recognition disabled or
analysis budget failure prevents output. Shared exports require book AND link
format/participant grants; the new additive capability is can_export_pxel.
Already saved XLSX-only rules do not silently enable the new format.

Native crypto and three adapter tests, 259 frontend units, type/lint/Clippy,
12 Chromium cases plus two anonymous guest cases and one hard-restart test passed.
RUNBOOK records exact logs and failed fixture corrections. Full combined checks,
manual NVDA and external deployment gates remain separate.

Portable sparse cells, styles, axis sizes and merges are ordered by visible
coordinates. Fresh IDs on import cannot change serialized ordering; persisted
event writes continue to use the independently required stable-address ordering.
