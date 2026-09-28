# Dependency review — phase 0

Runtime не требует SaaS, Google API или внешнего formula/collaboration engine.
Rust direct versions закреплены в Cargo.toml, полный graph в Cargo.lock;
JS direct versions и graph — package.json / pnpm-lock.yaml. Major upgrades ручные.

| Группа                                                    | Назначение и лицензии                                                                               |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Axum, Tokio, Tower HTTP, tracing                          | HTTP/WS, bounded async IO, security layers/logging; MIT / Apache-2.0                                |
| SQLx / rustls / ring                                      | PostgreSQL и TLS без OpenSSL runtime; MIT / Apache-2.0 / ISC-compatible upstream terms              |
| Prost / Buf / protobuf-es                                 | Собственный protobuf schema, Rust/TS генерация; Apache-2.0, protobuf-es Apache-2.0 AND BSD-3-Clause |
| thiserror, anyhow, Serde, UUID, bytes, futures-util, sha2 | Typed errors, application boundaries, metadata, IDs, streaming и SHA-256; MIT / Apache-2.0          |
| React / Vite / Vitest / ESLint / Babel / Prettier / ws    | UI, сборка, проверки и реальный WS test client; MIT                                                 |
| TypeScript / Playwright                                   | Strict types и браузерные тесты; Apache-2.0                                                         |
| PostgreSQL                                                | База данных; PostgreSQL License                                                                     |
| webpki-roots                                              | TLS root data; CDLA-Permissive-2.0                                                                  |

`cargo deny check` проверяет полный Rust graph; неизвестные licenses/sources,
wildcards и security advisories отклоняются. Transitive duplicate versions
показываются предупреждениями: нельзя принудительно объединять несовместимые major.
CDLA-Permissive-2.0 добавлена после чтения LICENSE TLS-root dataset; при распространении
данных необходимо сохранить текст лицензии. Это не исключение из vulnerability policy.

`pnpm audit` проверяет JS runtime и dev dependencies. Rust security advisories
дополнительно проверяются `cargo audit`. Локальные tooling downloads взяты из
official release assets с SHA-256; tool executables не являются runtime dependencies.
Актуальные результаты записаны в RUNBOOK, а не предполагаются из этой политики.

Известное tooling exception: deprecated ESLint 9 до совместимого stable parser
для TS7/ESLint10, [ADR-007](adr/007-phase-zero-toolchain-validation.md).
SQLx CLI 0.9.0 с upstream locked dependencies выдает предупреждения о yanked
tool-only crates; они не равны workspace Cargo.lock и не подавляются в установке.
Перед production release нужен полный attribution bundle из конкретных lockfiles.
