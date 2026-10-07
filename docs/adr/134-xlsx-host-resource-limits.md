# ADR-134: XLSX host-process resource limits

Status: accepted for locally verified Windows CLI execution, 2026-10-07.

ADR-131 guest limits do not bound Wasmtime's host allocations. For configured
WASI parsing use a kernel-owned process container with 512 MiB committed memory,
one live process on Windows and one core worth of CPU quota. Keep the independent fuel,
guest-memory and wall-clock limits. Failure to enforce a requested limit must
reject execution, with no native fallback.

Add exact processkit3.3.4 with only the limits feature. Existing process-wrap10.0.1
provides tree teardown but no resource-limit API. processkit's safe API allows
our Rust to retain forbid(unsafe_code); on Windows it creates the process
suspended, assigns it to the capped Job Object and resumes only after assignment.
Use the group for the complete transfer lifetime; drop kills the contained tree.
The unchanged operator-only native CLI retains its existing process-wrap path.

Candidate prototype under ignored .local confirmed a 128 MiB synthetic allocation
fails at a 64 MiB Windows job cap and passes with only the cap raised to 256 MiB.
This is initial mechanism evidence, not application acceptance. Application
parity, actual cap violation, cancellation, dependency audit and combined checks
must pass before treating the new dependency as a tested baseline.

The complete component checks now pass: 750 Rust tests/17 ignored, both Clippy
profiles, native/WASM/protocol builds, 334 frontend tests and lint/typecheck/build,
plus five adapter tests with allocation canaries and 15 real runtime/backend
checks. The initial exact just check stopped at RUNBOOK formatting; formatting
was fixed and its remaining frontend commands passed. Exact post-rebase just
check subsequently passed, and real runtime/backend15/15 passed again. Results
are recorded separately in RUNBOOK. Linux/public upload remain open.

Linux requires enforceable cgroup v2 limits and must fail closed where unavailable;
Linux pids.max counts threads, so its cap is 32 tasks to permit bounded runtime
threads; memory.max bounds cgroup memory rather than Windows commit charge.
Linux execution remains unverified. Windows CPU quota is an approximation of one
core's share of total host CPU. A process suspended between creation and job
assignment can remain inert if the parent dies in that small window; no execution
before assignment is permitted. Public browser import remains unfinished.

Sources: [processkit group contract](https://docs.rs/crate/processkit/3.3.4/source/docs/process-groups.md),
[Microsoft job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).
