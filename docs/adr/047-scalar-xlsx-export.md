# ADR-047: scalar XLSX package writer

Status: accepted for the fifth phase-6 slice (2026-09-29).

`spreadsheet-xlsx::export_scalar_xlsx` accepts only the validated
`NormalizedWorkbook` IR. It creates a ZIP/OOXML package with content types,
root/workbook relationships, one workbook part and one worksheet per sheet.
Values use finite numeric `<v>`, boolean `<v>`, inline strings with
`xml:space="preserve"`, and formula source without a cached result. XML text
and attributes are escaped. Sheet names, coordinates, XML characters and
formula source are checked before writing. No domain IDs, server calculation
cache, macros, external links or unsupported styling enter the file.

Entries are stored without compression. This keeps output within the reader's
100:1 anti-bomb ratio; each part must also fit the reader's 8 MiB cap, total
uncompressed content 32 MiB, and the complete ZIP 32 MiB. The function returns
bytes only after the archive is finished and all limits pass. The public
function deliberately does not accept domain entities: a later authorized
export use case must project the current workbook into this IR and audit the
export before publishing an artifact.

Tests round-trip the synthetic two-sheet fixture through the writer and our
own reader, including special XML characters, significant whitespace, formula
source and `XFD` coordinates. This is **not** an Excel-verified compatibility
claim. Excel/LibreOffice manual fixtures, styles, dates, merges, frozen panes,
defined names, object-storage artifacts and document HTTP export remain open.
