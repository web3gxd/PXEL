# Storage adapters in phase 0

`PgDatabase` connects without SQL statement logging, applies checked-in SQLx
migrations and checks schema availability using a compile-time checked query.
Readiness additionally validates successful versions/checksums against the embedded
migrator using SQLx's migration API. Unknown, missing, dirty or changed migration
records fail closed. Two SQLx tests create isolated databases using `DATABASE_URL`
with CREATE DATABASE privileges; static SQL there intentionally injects schema
corruption and invalid tenant/role fixtures. There is no fallback to mock storage.
There is no document command/operation write path in this phase.

`LocalFilesystemBlobStore` implements the application `BlobStore` contract against
a private directory. Blob keys are canonical lowercase UUIDs or SHA-256 hex, never
paths. A versioned single-file envelope stores size and checksum with content;
metadata and body become visible together through an immutable hard link. Body
size is enforced while consuming a stream. Temporary files are private and removed
when a failed/cancelled future drops under normal filesystem operation. Destructor
cleanup cannot report an I/O failure; such failures and process termination can
leave dot-prefixed orphans that never become readable blob keys. Offline cleanup can remove these
temporary files only after ensuring there are no active writers.

Successful writes flush/sync the file. On Unix publication and deletion also sync
the containing directory. On Windows the safe standard library cannot promise a
durable directory-entry flush; Windows development checks atomic visibility and
file sync, not power-loss persistence of directory entries. The adapter requires
a local filesystem supporting hard links. It never silently falls back to a
non-atomic copy or overwrites a different object under an existing key.

Retrying identical content at one key succeeds, including after an unknown outcome
during publication. Cancellation/failure after publication can leave the complete
object present; this is an unknown result, not evidence that no write occurred.
Deleting an absent key succeeds. Coordinating retention deletion with concurrent
application writers belongs to the future snapshot/GC use case.

`head` verifies the entire body. `get` verifies the body before opening its bounded
64 KiB output stream; it therefore reads twice, trading throughput for a simple
integrity contract. A benchmark is required before optimizing this verification.

Symlinks, Windows reparse points and unsafe key components are rejected. Unix roots
must have private permissions (new directories use 0700 and temporary files 0600).
Windows directory ACLs must be configured by the operator. The root and ancestors
must be owned/administered by trusted local principals. This adapter is not a
sandbox against another process that can mutate those paths between check and
open, or mutate an open blob after verification. No unsafe or platform-specific
raw syscall exception is introduced to claim stronger protection.

Contract tests exercise the production adapter with real temporary directories.
They cover streaming, checksums, limits, missing keys, idempotence, conflicts,
concurrent publication, cancellation and corrupt objects. Unix-only tests cover
symlink rejection and run in Linux CI; Windows validation does not count them as
executed.
