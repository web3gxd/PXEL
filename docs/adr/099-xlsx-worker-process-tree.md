# ADR-099: terminate the complete XLSX worker process tree

Status: locally verified on Windows (2026-10-02); Linux runtime verification
remains open.

## Context

The framed worker timeout killed only the direct Tokio child. If that process
created a descendant, the descendant could outlive the timeout and keep inherited
stdio handles open. Cancellation of the async parent also needed a synchronous
cleanup request. This was process-tree lifecycle control, separate from the still
missing hard resource and filesystem/network sandbox.

## Decision

1. Pin `process-wrap` 10.0.1 in `pxel-server`, with only its Tokio integration,
   process-group, Windows Job Object and kill-on-drop features enabled.
2. On Windows, use its Job Object wrapper. It creates the child suspended,
   assigns it to a kill-on-close Job Object, and resumes it after assignment.
   Explicit timeout/protocol cleanup terminates the whole Job; dropping the
   parent operation closes the kill-on-close Job.
3. On Unix, place the worker in a new process group. Explicit cleanup sends
   termination to that group. `WorkerProcessGuard::drop` makes this request
   synchronous when the parent async operation is cancelled.
4. Continue to use the existing ten-second timeout, framed protocol, response
   size limit, parent IR validation and environment clearing. No workbook bytes
   or parse errors are logged.

The Windows lifecycle follows the Windows Job Object model described in
[Microsoft's Job Objects documentation](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
and the safe wrapper contract in
[`process-wrap` 10.0.1](https://docs.rs/process-wrap/10.0.1/process_wrap/tokio/struct.JobObject.html).

## Verification and limits

An isolated test worker starts a same-executable descendant that inherits the
stdout pipe. The test waits for readiness, terminates the root, waits for tree
completion and requires the pipe to reach EOF. This exercises process-tree
termination rather than just checking the direct child's exit status.

Windows verification passed: the targeted process-tree test, `pxel-server`
unit/CLI tests (27 + 3), and strict Clippy. `cargo fmt --all -- --check` also
passed. Linux runtime behavior is not claimed. Job/process-group termination is
not a resource/security sandbox: hard CPU and committed-memory caps, restricted
filesystem/network access, descriptor limits and broad hostile-file coverage
remain open. Browser upload remains gated on those controls, malware-scanning
policy and complete compatibility reporting.
