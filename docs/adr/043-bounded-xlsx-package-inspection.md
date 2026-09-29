# ADR-043: bounded XLSX package inspection before import

Status: accepted for the first phase-6 slice (2026-09-29); direct CLI entry
point replaced by the child-process preview in ADR-045.

The untrusted XLSX reader is a separate `spreadsheet-xlsx` crate. Its first
consumer was the server binary's `xlsx-inspect <path>` mode, which started before
database configuration and printed only counts. That mode did not publish a
document, parse cell values, or expose file contents in logs.

Inspection reads at most 32 MiB of ZIP input, 256 entries, 8 MiB per entry,
32 MiB decompressed total, 100:1 expansion per entry and 5 seconds of wall
time. It rejects traversal, ambiguous duplicate names, symlinks, encryption,
unsupported compression, DTDs, unknown entities, external relationships and
recognized active-content parts. XML depth, attributes, text and relationship
counts are bounded. It resolves the package's officeDocument relationship,
workbook and worksheet relationships into sheet headers only. Package DTOs do
not enter the domain model.

The current implementation holds bounded part bodies in memory and runs as a
CLI mode, not yet an OS-sandboxed child with a framed parent protocol. It is
not an import or export implementation. A later slice must add normalized IR,
streaming cell parsing, process isolation and parent-side validation before
accepting uploaded files. The 5-second check is cooperative and not a hard
CPU or wall-time kill; only the process-isolation slice can establish that.

`zip` 8.6.0 (deflate only) and `quick-xml` 0.42.0 are general ZIP/XML libraries,
not spreadsheet frameworks. Their versions are pinned by Cargo.lock. The
small in-memory ZIP corpus covers a valid two-sheet package, traversal,
external relationships, DTD and duplicate sheet identifiers.
