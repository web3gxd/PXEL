# ADR-102: authenticated storage envelopes

Date: 2026-10-02. Status: IMPLEMENTED; local Windows combined acceptance passed, including workspace checks and dependency audit. Linux/Compose/remote CI remain deployment gates.

The storage journal, snapshots and export preparation audit previously contained
plaintext authored data or content fingerprints. The agreed security work adds
encryption to these existing paths before guest sharing is enabled. PostgreSQL
remains authoritative; candidate validation, CAS, receipts and durable commit
before ACK retain their existing ordering.

`application::keys::KeyProvider` is the used wrapping port. Storage owns
RustCrypto XChaCha20-Poly1305, durable key records and nonce allocation. Domain,
formula and WASM receive neither KEKs nor DEKs. The pinned candidates are
`chacha20poly1305=0.11.0`, `getrandom=0.4.3`, `zeroize=1.9.0`; they become a
tested baseline only after the recorded combined checks and dependency audit.
Cipher and SHA-256 internal states enable their `zeroize` features.

Each document receives an independent random 32-byte DEK. Only authenticated
48-byte wrappers, nonces, physical key identities and generation IDs persist.
KEKs are configured by exactly one of `PXEL_KEYRING_FILE` and
`PXEL_KEYRING_JSON`. Configuration has schema 1, an `active_kek_id` and at most
32 entries `{id,key_hex}`. IDs are unescaped ASCII letters/digits/`-_.`; each
key is 64 hex characters. Input is bounded to 64 KiB, rejects duplicate IDs,
unknown fields and a zero KEK. No fallback key is generated. Files reject
symlinks and public access (Unix private permission bits; Windows owner and
allow entries restricted to current user, SYSTEM and Administrators). Keyring
files belong outside PGDATA, blobs and backup/restore destinations. Owned
configuration bytes, KEKs, DEKs, cipher state and decrypted buffers are erased
on drop. The original environment itself remains the operator's responsibility.

Crypto-envelope 1 has an independent lifecycle from persisted and wire codecs:
8-byte `PXELENC` plus NUL magic, u16BE format 1, u16BE algorithm 1, u32BE
DEK generation, random 24-byte nonce, ciphertext and 16-byte tag. Total overhead
is 56 bytes. AAD uses a domain separator and the expected workspace, document,
data version, purpose, object/operation UUID, DEK generation and codec version.
KEK IDs and wrapper revisions are excluded so rewrapping does not rewrite data.
Key workspace is checked against the authoritative document when it exists;
pre-genesis durable key records cannot silently change workspaces.

Every nonce is reserved under a SHA-256 physical-key identity in a separate
`synchronous_commit=on` PostgreSQL transaction **before** encryption. Equal key
material under different aliases has the same identity. Conflicts consume up to
eight bounded random attempts; aborted document transactions retain reservations.
An independent two-connection crypto pool prevents a held single-connection
document transaction from blocking the reservation it needs. Registry mutation
and truncation are forbidden.

The full persisted event, including formulas, Undo preimages and authored state
hash, is encrypted. Command fingerprints and canonical event hashes are also
separate authenticated envelopes. Ciphertext SHA-256 is the external corruption
check; authentication occurs before persisted decoding or publication. Snapshot
payloads are sealed before BlobStore receives bytes, including its temporary
files; snapshot state hashes are encrypted metadata. Export remains in memory;
its checksum and size share an encrypted 40-byte audit record. Existing 4 MiB
event and 32 MiB snapshot/export plaintext limits remain; encrypted readers and
SQL add checked envelope overhead. History budgets count bounded plaintext size
while allowing only the corresponding bounded ciphertext overhead.

Runtime has no legacy plaintext decoding fallback. Startup rejects legacy
datasets and unavailable retained KEKs. `PgDatabase::migrate` refuses plaintext
journals before changing their schema. Offline migration, encrypted rollback
backup, restore with fresh write generations, guest authentication, comments
and sharing UI are subsequent acceptance slices; this ADR does not claim them.
Attachments and a disk export cache are absent. Their future writers must
encrypt before any temporary or permanent bytes reach storage and must define
their own AAD purposes, size limits and authenticated recovery contract.

The encrypted snapshot hard-restart acceptance also covers the S3 adapter:
against loopback Moto, PostgreSQL metadata matched the direct AEAD object bytes,
the synthetic cell style was absent from the object, and a server restart
recovered the selected snapshot plus event tail. The local filesystem adapter
passed the same restart scenario. This does not establish compatibility with a
production S3 provider or Garage; see RUNBOOK for the recorded environment.

Sources: [RustCrypto 0.11.0](https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/),
[OS randomness 0.4.3](https://docs.rs/getrandom/0.4.3/getrandom/),
[zeroize 1.9.0](https://docs.rs/zeroize/1.9.0/zeroize/).
Exact passed/failed commands and remaining gates are recorded in RUNBOOK.

Startup readiness streams all retained DEK wrappers through one ordered
LEFT JOIN with their durable document workspaces, including orphan pre-genesis
key rows. It still authenticates every wrapper, checks physical key identity,
rejects unsupported wrap schemas and verifies document/workspace consistency;
this removes the per-generation metadata-query fan-out without caching or
skipping key verification. On 2026-10-05 the 24-restart recovery acceptance on
the accumulated local database (1,812 generations at the earlier measurement)
passed in 45.2 s under its unchanged 180 s test timeout. See RUNBOOK for the
combined smoke and workspace gate results.
