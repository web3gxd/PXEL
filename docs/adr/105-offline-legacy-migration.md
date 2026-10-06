# ADR-105: offline migration into a separate encrypted cluster

Date: 2026-10-02. Status: IMPLEMENTED; content-bearing Storage API and CLI migrations passed against separate databases on another physical cluster. API Undo history, encrypted rollback recovery, CLI post-restart verification and stdout/stderr redaction for the cell marker and source/target/recovery URLs passed; representative frozen-service migration remains open.

Legacy decoding is compiled only with `legacy-migration` for the maintenance CLI.
Serving paths never fall back to plaintext. The supported source has the exact
pre-encryption migration manifest through 202610020005 and the known table,
column and type set. The source is read-only in a repeatable-read transaction;
legacy services/workers must already be stopped because they do not implement
the new maintenance fence. The destination is a fresh private blob directory
and an empty latest-schema PostgreSQL cluster with a different physical identity.

Before copying, the CLI creates an encrypted physical rollback archive and checks
its authentication, PostgreSQL manifest/WAL and actual private recovery. Durable
archive nonce reservations belong to the fresh destination registry and remain
burned during migration. A nonce-only registry does not make a destination a
pre-existing dataset.

Migration initializes its PENDING barrier with the exclusive destination lock
in a single synchronous commit, then reacquires maintenance before copying.
Metadata preserves identities, timestamps, ACL, epochs and audit. Each plaintext
journal payload/fingerprint/hash is checked and sealed before insertion; no
plaintext document bytes reach destination PostgreSQL/WAL. Every historical
version is replayed against candidate/state hashes. Snapshots are verified
against that exact historical book and encrypted before target BlobStore sees
bytes. Export checksum/size audit records are sealed. UTC serialization prevents
different database time zones from causing false metadata mismatches.

Missing REJECTED or UPLOADING snapshot objects may be legitimate interrupted or
cleaned uploads. Migration preserves their identities, states and timestamps,
seals their original state hash, adds checked ciphertext overhead to the length,
and records a zero external checksum without writing a placeholder blob. The
report counts these metadata-only tombstones. Missing ACTIVE, STORED or VERIFIED
objects fail closed. A preferred ACTIVE snapshot must remain verified and present.

Every target event, historical state, snapshot, ACL/audit row and count is compared
with the frozen source before COMPLETE activation commits with the exclusive
fence. Failure leaves a durable PENDING target that serving code cannot open.
Retry requires another empty target; it cannot rewrite the original tables.
Configuration publication and post-restart verification precede reopening writes.

Rollback before new writes can restore the verified original copy. After new
acknowledged writes, rollback to an older snapshot is prohibited. Historical
plaintext media/WAL must be retired through a separate operator procedure;
secure erasure of SSDs is not promised.
