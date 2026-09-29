# ADR-044: bounded XLSX scalar intermediate representation

Status: accepted for the second phase-6 slice (2026-09-29).

The package inspector now has a second entry point, `normalize_reader`, which
returns an XLSX-specific `NormalizedWorkbook`. It contains sheet names, one-based
OOXML coordinates and scalar values. It assigns no stable row/column/sheet IDs,
does not parse or bind formulas with the canonical engine, and cannot publish a
document. The server binary exposes a local `xlsx-preview <path>` CLI mode that
prints only sheet/cell counts through the child process specified in ADR-045.
No upload enters the server process.

Supported extraction is deliberately narrow: finite numeric values, booleans,
simple shared strings, simple inline strings and formula source text. XLSX
formula caches are discarded. Numeric `-0` normalizes to `0`. Standard XML
predefined entities and valid numeric character references are resolved;
unknown entities remain rejected. Duplicate cell addresses, invalid shared
string indices, malformed coordinates, unsupported rich text and unsupported
cell types reject the whole extraction. A global cap of 100,000 cell elements
includes empty elements. Expanded text in the resulting IR is capped at 32 MiB,
so repeated references to a large shared string cannot amplify memory without
bound. Per-string and formula text caps and package byte/time limits from
ADR-043 still apply.

This IR is a parsing result, not an import result. Styles, number formats,
dates, merges, panes, defined names and unsupported OOXML metadata need an
explicit compatibility report before publication. Formula source requires
server-side parsing/binding against the imported axes. A child-process protocol,
OS-enforced time/memory limits, parent schema validation, malware scan and
authorization are required before a user upload endpoint. The synthetic
`minimal_scalars.xlsx` fixture is reproducibly generated with Python's standard
ZIP library; it is not an Excel-verified golden fixture.
