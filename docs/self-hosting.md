# Self-hosting PXEL

PXEL currently targets one instance, one workspace, PostgreSQL and a local
filesystem blob store. It is intended for a team on a trusted local network.
Local network users still need HTTPS and accounts. The master account is an
instance administrator with full access to every book, including books created
by ordinary users. Ordinary users retain document ACLs.

## Build from source

Use the versions in `rust-toolchain.toml` and `package.json`. Install the pinned
`wasm-bindgen-cli` **0.2.129** or set `PXEL_WASM_BINDGEN` to that binary. From the
repository root:

```sh
pnpm install --frozen-lockfile
node scripts/wasm.mjs
pnpm build
SQLX_OFFLINE=true cargo build --locked --release -p pxel-server
```

Do not enable `dev-auth` or `PXEL_DEV_AUTH` for the deployed binary. Build and
runtime commands must use the repository root as their working directory:
the server serves `apps/web/dist`. Preserve the complete source and build scripts
corresponding to the deployed version.

Before a build of a modified network version, set `VITE_PXEL_SOURCE_URL` to an
HTTPS URL or same-origin absolute path offering its complete Corresponding Source.
The UI links to this URL in the sign-in page, library and editor's **О PXEL**
dialog. The default points to the upstream repository. A link to upstream alone
does not provide the source of your local modifications. Browser runtime license
notices and the complete AGPL text ship in `apps/web/public`.

## Provision private storage and HTTPS

Create a fresh PostgreSQL database with `fsync=on`, `synchronous_commit=on` and
`full_page_writes=on`. Use a dedicated role; keep PostgreSQL and blobs inaccessible
to clients. Configure backups before storing important data.

Provision an independent random 256-bit key-encryption key in a private keyring
file outside PGDATA, blobs and backup destinations. Its format is:

```json
{
  "schema_version": 1,
  "active_kek_id": "instance-v1",
  "keys": [{ "id": "instance-v1", "key_hex": "<64 random hexadecimal characters>" }]
}
```

The placeholder is not a usable key. Generate the key on the installation host;
keep a protected independent recovery copy. Never reuse another instance's keys.
There is no built-in default key. On Linux use mode `0600` and a private parent
directory owned by the service account; Windows needs a private ACL. Do not put
keys, DB URLs, passwords or session cookies in Git, command arguments or logs.

Terminate trusted HTTPS at a reverse proxy and forward HTTP and WebSocket
upgrades to a loopback-bound server. Set exact public Origins, including scheme
and port. Do not configure a wildcard. Cookies are always `Secure`, `HttpOnly`,
`SameSite=Strict`, path `/api`. Plain HTTP to a LAN IP is unsupported.
Browser localhost exceptions are for tests, not a LAN deployment recipe.

| Setting                     | Purpose                                                                                                               |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `PXEL_DATABASE_URL_FILE`    | Private file containing the dedicated DB URL; alternatively use `DATABASE_URL` in the service's protected environment |
| `PXEL_KEYRING_FILE`         | Private keyring JSON file; do not also set `PXEL_KEYRING_JSON`                                                        |
| `PXEL_BLOB_ROOT`            | Private writable blob directory                                                                                       |
| `PXEL_BIND`                 | Backend address, normally `127.0.0.1:8080`                                                                            |
| `PXEL_ALLOWED_ORIGINS`      | Comma-separated exact browser Origins, e.g. `https://pxel.internal`                                                   |
| `PXEL_BOOTSTRAP_TOKEN_FILE` | Private bootstrap code file for the first launch                                                                      |

Use an unprivileged service account. Configure the proxy's connection, body,
timeout and rate limits for the expected team size. `/healthz` checks the process;
`/readyz` checks storage readiness. Neither proves that onboarding, backup or
all deployment gates have passed. The server applies versioned DB migrations at
startup; take a verified backup before upgrading an existing installation.

XLSX uploads require the separately pinned WASI runtime and host resource
configuration in [XLSX WASI setup](setup/xlsx-wasi.md). Without it, XLSX uploads
remain unavailable; this does not silently switch to unsandboxed parsing.

## First run and account administration

Generate a one-time bootstrap file on the server:

```sh
node scripts/bootstrap-token.mjs .local/bootstrap-token
```

The script creates a random code, protects the file and refuses to overwrite an
existing one. It prints the path, never the code. Set `PXEL_BOOTSTRAP_TOKEN_FILE`
to that file's absolute path. Read the code privately on the server and enter it
in the first-run form over HTTPS. Do not put it in a URL or clipboard history.

1. Open PXEL and continue from the welcome screen.
2. Enter the server bootstrap code, a master login and a strong password.
3. Create other accounts from **Пользователи** in the library. Deliver each
   temporary password through a protected channel.
4. A new user's first login requires setting a different password before books
   or administration endpoints become accessible.

Logins use 3–32 lowercase ASCII letters, digits, `.`, `_`, `-`. Passwords use
12–128 Unicode characters, at most 512 UTF-8 bytes. The current bound is 1000
accounts per instance. There is exactly one master; it cannot be disabled,
demoted or reset by another account. Setup succeeds once under a DB lock and
refuses existing books. It does not adopt development data or reopen registration.

After setup, remove `PXEL_BOOTSTRAP_TOKEN_FILE` from service configuration, restart
and verify login. Then securely remove the obsolete bootstrap file. Removing the
file while leaving its configured path causes startup to fail closed.

Sessions expire after eight hours. Password changes, resets and account disabling
revoke sessions; temporary passwords require another first-login change. Five
failed password attempts lock the account for fifteen minutes. Unknown and disabled
accounts receive the same login error. The master can issue a new temporary
password for another user, or disable and enable that account. There is no
automated master password recovery; protect its credentials and verify recovery
procedures before relying on the installation.

## Backups and upgrades

Back up PostgreSQL and blobs as one consistent maintenance bundle; keep the
required keyring separately. PXEL has encrypted backup, recovery-check, restore
and PITR operator commands documented in
[ADR-104](adr/104-encrypted-physical-backups-and-private-recovery.md) and
[development notes](development.md). Restore to a private new target, verify it,
renew write keys and revoke old sessions before publishing the recovered service.
Account passwords, enabled states and permissions revert to the backup snapshot.
Audit them and reset affected credentials after recovery; post-backup security
changes are not recovered by deleting old sessions.
Retain the source commit and lockfiles for rollback. A schema upgrade is not
assumed reversible; rollback may require restoring a verified backup.

## Deployment gates

Source publication and local tests do not close these gates:

- Linux build, PostgreSQL durability/recovery, Compose and GitHub Actions combined verification.
- Actual Linux WASI host limits and XLSX sandbox acceptance.
- Trusted LAN HTTPS, proxy WebSocket/timeouts/rate limits and multi-user load on the chosen host.
- Backup/restore and PITR drills including current account/session revocation and independent key recovery.
- Master credential recovery procedure, upgrade/rollback rehearsal and operational monitoring.
- Accessibility acceptance, including screen-reader testing; local visual checks alone are insufficient.

Record actual outcomes and remaining limits in [RUNBOOK](../RUNBOOK.md). Do not
publish this installation as production-ready until its applicable gates pass.

Fresh-instance HTTP/WebSocket and Chromium acceptance is available as
`just selfhost-check`. It requires native PostgreSQL 18 binaries in `PXEL_PG_BIN`,
an installed Playwright Chromium, generated WASM and a built frontend. It creates
its own private clusters, does not reuse the development database and retains
diagnostic artifacts under ignored `.local/`. This is separate from Compose smoke.
