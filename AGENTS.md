# PXEL development contract

Read RUNBOOK.md, docs/roadmap.md and applicable ADRs before each stage.
The user has now authorized continuing all remaining stages (2026-09-28).
Implement and verify consecutive vertical slices; do not claim later phases done early.
The locally verified phase-0 foundation may be extended; Linux/Compose/remote CI
verification remains an explicit open deployment gate, not a passed check.
Retain the corrected formula/collaboration contracts and record changes in ADRs.

- Keep the modular monolith and Rust crate dependency boundaries in docs/architecture.md.
- PostgreSQL is authoritative. Future commits require candidate-state validation,
  CAS on head_version and durable commit before publication or acknowledgement.
- Never mix confirmed content, calculated results and optimistic operations.
- Wire protocol and persisted codecs have separate versioning lifecycles.
- Phase 0 uses LocalFilesystemBlobStore. Do not add MinIO or Garage.
- Do not add unused crates, ports, operations or mocked production persistence.
- Our Rust code forbids unsafe; domain/library code must not unwrap, expect or panic.
- Keep document content, credentials and request/response bodies out of logs.
- Use typed boundary errors; unknown protocol versions fail atomically.
- Pin dependencies and tooling. Candidate versions become tested baseline only
  after recorded combined verification. MSRV policy is not established before v1.
- Run meaningful tests, report actual outcomes and record all changes in RUNBOOK.md.
- Keep secrets and local toolchains/data under ignored .local/.tools paths.
- Sub-agents may work on explicitly separate files; coordinate shared contracts first.

Commands and verification prerequisites are documented in README.md.
