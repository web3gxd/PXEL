# ADR-045: initial framed XLSX child process

Status: accepted for the third phase-6 slice (2026-09-29).

The counts-only response described below was superseded by the bounded IR
response in [ADR-046](046-bounded-xlsx-worker-ir.md). The process isolation
limitations and timeout policy remain.

`pxel-server xlsx-preview <path>` now canonicalizes the input path, starts the
same binary in `xlsx-worker` mode, clears its inherited environment, and talks
through a fixed version-1 binary request/response frame. Request: `PXIW`,
big-endian u16 version, big-endian u16 UTF-8 path length (1–4096), path bytes.
Response: `PXOW`, big-endian u16 version, u8 status, big-endian u32 sheet count
and cell count. The parent accepts only one exact response frame and validates
the counts. The worker returns no cell content, formula source or detailed
parser error. Unknown versions and malformed frames fail closed.

The parent uses a ten-second timeout, kills and waits for the worker tree on
timeout or protocol failure, and does not pass DB/S3 credentials in environment
variables. The parser's own ZIP/XML/IR budgets still apply. Process-tree
containment is now implemented through `process-wrap` 10.0.1: Windows starts the
child suspended, assigns it to a kill-on-close Job Object, then resumes it;
Unix starts a new process group and sends tree termination to that group. A
parent-side guard also requests tree termination if its async operation is
dropped. The worker still runs with the server's filesystem privileges. This
does **not** establish a resource or security sandbox: hard CPU/memory limits,
restricted filesystem/network access, private input staging and descriptor
limits remain open. No HTTP upload or document publication uses this worker yet.

An early end-to-end run timed out because the worker waited for stdin EOF while
the parent retained the pipe handle. The parent now explicitly drops stdin
after sending the request. Binary CLI tests cover valid and malformed files,
the fixed response codec covers version/status/count rejection, and the
untrusted bytes are not echoed to stdout/stderr.

Before import, the remaining gates are hard resource limits, restricted
filesystem/network access and a hostile-file corpus against Linux and Windows
adapters. Do not treat this worker as a document import channel before those
gates, malware scanning policy and full compatibility reporting are complete.
