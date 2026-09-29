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

The parent uses a ten-second timeout, kills and reaps the child on timeout or
protocol failure, and does not pass DB/S3 credentials in environment variables.
The parser's own ZIP/XML/IR budgets still apply. This makes cancellation and
crashes containable at the process level. It does **not** establish an OS
sandbox: the worker currently runs with the parent's filesystem privileges,
and cgroups/Job Objects, private temp directory, process-group termination,
file descriptor limits and read-only input remain open. No HTTP upload or
document publication uses this worker yet.

An early end-to-end run timed out because the worker waited for stdin EOF while
the parent retained the pipe handle. The parent now explicitly drops stdin
after sending the request. Binary CLI tests cover valid and malformed files,
the fixed response codec covers version/status/count rejection, and the
untrusted bytes are not echoed to stdout/stderr.

Before import, extend the worker protocol to carry a bounded, versioned IR,
validate that IR again in the parent, add platform OS restrictions, and run a
hostile-file corpus against Linux and Windows adapters. Do not treat this
counts-only preview protocol as a document import channel.
