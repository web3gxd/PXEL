# ADR-104: encrypted physical backup and private recovery

Date: 2026-10-02. Status: IMPLEMENTED; encrypted Windows recovery and named-point PITR passed locally on 2026-10-03.

The supported maintenance bundle contains a dedicated local PostgreSQL 18.6
cluster without extra tablespaces and its LocalFilesystemBlobStore objects.
`backup create <archive> --maintenance-confirmed` requires stopped application
workers and an exclusive database fence. Sources, includes, WAL and secret
placement are checked before streaming. Secret URL/keyring files are private,
bounded and reject hard links; credentials use environment, never CLI arguments.

PostgreSQL `pg_basebackup -Ft -D - -X fetch` stdout is framed in memory alongside
sorted physical blobs, directly into RustCrypto AEAD-STREAM. No plaintext backup
file is written. Every archive has an independent random DEK, authenticated
header and encrypted manifest. The 64 KiB data frames have authenticated kind
and length and a mandatory final tag followed by exact EOF. Every physical-key
nonce is committed before use, including each STREAM nonce. An output failure
retains consumed reservations. The current bounded manifest allows 4096 blob
objects and 1 MiB metadata; exceeding it fails before publishing an archive.

Publication uses a new private ciphertext partial file and retained directory/
file identities, verification of the complete bundle, no-overwrite publication,
file durability and platform-supported directory sync. A typed maintenance guard
permits encryption under its existing exclusive fence; standalone archive
encryption owns the shared fence. ACL/session mutations use the same writer fence.

`backup restore <archive> <new-root> --to <fresh_key_id>
--maintenance-confirmed` authenticates the whole archive before extracting into
an unpublished private root. The recovery reader is protected against mutation
between verification and extraction. Tar extraction accepts bounded regular
files/directories only, checks paths/checksums/duplicates/padding/end blocks,
and rejects links, tablespace redirection and platform control-name aliases.
The extracted plain PostgreSQL layout passes `pg_verifybackup`, including WAL,
then actual recovery with controlled configuration and no inherited library,
replication or archive commands. The restored cluster identity and durability
are checked before fresh write generation activation. It is stopped before
completion is reported; stop failures are explicit. Failed stages remain private
and unpublished, and cannot be overwritten by retry. Uncatchable host/process
failure requires inspecting and stopping that private stage before manual cleanup.

`backup recovery-check <archive> <new-root> --maintenance-confirmed` performs the
same physical verification and recovery in read-only mode without enabling a
write lineage. This also verifies an encrypted legacy rollback bundle. It does
not publish a runnable service or claim fresh write keys were activated.

The opt-in encrypted PITR form adds `--target-name <name>` and
`--wal-archive-dir <private-dir>` to `backup restore`. PostgreSQL 18.6 WAL files
are archived one at a time through `backup wal-archive <archive-dir>
<source-path> <wal-name>`; the encrypted manifest binds the canonical PostgreSQL
name, exact length and SHA-256. Archiving is idempotent only when a retry decrypts
to the same source bytes. `backup wal-restore <archive-dir> <wal-name>
<destination>` authenticates the entire WAL object and manifest before writing
plaintext. On Windows, PostgreSQL supplies the reserved `%p` basenames
`RECOVERYXLOG` and `RECOVERYHISTORY`; they are accepted only for segment or
history/backup names respectively, while the encrypted manifest must still match
`%f`. Existing private `pg_wal` targets are replaced only after authentication,
then flushed and synced before the helper succeeds. Recovery waits up to 30
seconds for both database pools to connect after `pg_ctl` starts the restored
cluster. The named-point Windows PostgreSQL 18.6 drill passed locally, including
v4 head/hash/ciphertext, snapshot v3 plus one-event replay, historical v3, and
exclusion of the later v5 commit. This is a local acceptance result, not a
production WAL-retention or RPO claim.

Windows-specific recovery normalizes canonical `\\?\` paths before launching
PostgreSQL utilities and retains only `SystemRoot`, its `cmd.exe` as `ComSpec`,
and the pinned PostgreSQL `bin` directory in their environment. Directory identity
handles deny delete sharing while allowing child-file writes, which lets the
recovered postmaster create its pid/WAL files without allowing the protected
directory itself to be renamed. The original `postgresql.auto.conf` is bounded
to 1 MiB, copied to the private staging root, and replaced in place with a
comment-only file before startup. This prevents restored includes and commands
from running while avoiding a Windows rename against a guarded data directory.
The `pg_ctl start` launcher is not killed when its short-lived process exits;
the registered recovery lifecycle explicitly stops the descendant cluster, even
on cancellation. A private `restore-postgres.log` captures PostgreSQL's stderr;
the generated config disables statement and parameter logging.

Restore renewal retains every old ciphertext/key generation and initializes its
PENDING barrier in the same transaction as the exclusive maintenance lock.
New writes, rotation and backups fail during pending activation. Each document,
including orphan pre-genesis keys, receives a fresh independent DEK, wrapped by
a physically unused target KEK. Aliases, retained DEKs and old physical nonce
identities cannot be selected as that KEK. Durable per-document progress resumes
only under the exact pending target; final activation verifies all retained keys
and switches the active KEK atomically. A copied registry alone is never permission
to encrypt again with pre-restore keys. Later guest-sharing tables must be revoked
by this flow before service publication.

Rotation does not remove KEKs needed by retained archives. Production S3 backup
sources, Linux runtime, remote CI, coordinated distributed maintenance, off-site
WAL retention, and production PITR/RPO remain separate gates. The existing
plaintext test-only recovery fixtures are not proof of encrypted backup safety.

Sources: [RustCrypto STREAM 0.6.0](https://docs.rs/aead-stream/0.6.0/aead_stream/),
[PostgreSQL 18 pg_basebackup](https://www.postgresql.org/docs/18/app-pgbasebackup.html),
[PostgreSQL 18 pg_verifybackup](https://www.postgresql.org/docs/18/app-pgverifybackup.html).
