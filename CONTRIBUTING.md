# Contributing to PXEL

Open an issue with the problem, expected behavior and a minimal example. Keep
real workbook contents, credentials and private server details out of issues.
Use synthetic data for reports and tests. Security reports follow [SECURITY.md](SECURITY.md).

Read [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), the relevant
[ADRs](docs/adr) and [RUNBOOK](RUNBOOK.md) before changing a contract. PostgreSQL
is authoritative; validate a candidate and commit durably before acknowledging
or publishing it. Keep confirmed content, calculated results and optimistic
operations separate. Version wire and storage codecs independently.

Use the pinned tools in `rust-toolchain.toml`, `package.json` and `justfile`.
The [development notes](docs/development.md) describe installation and extended
test fixtures. For a change touching storage or authorization, test against an
isolated real PostgreSQL, including failure and recovery paths.

Before submitting a pull request:

- Run `just check`, `just sqlx-check`, `just audit` and the relevant HTTP/browser/recovery tests.
- Run `pnpm repository:check`; never add `.local`, `.tools`, secrets, logs, generated builds or database files.
- Add a regression test for a behavior or security fix; report which checks actually ran.
- Document changed contracts in an ADR and record verification in `RUNBOOK.md`.

Our Rust code forbids unsafe; domain/library code must not unwrap, expect or
panic. Logs must not contain document content, credentials or request bodies.
Keep dependencies necessary, pinned and reviewed.

Contributions are made under AGPL-3.0-only, the repository's license. You retain
copyright in your contributions. Include appropriate attribution for any
third-party code or assets; do not submit material you cannot license compatibly.
No copyright assignment is required.
