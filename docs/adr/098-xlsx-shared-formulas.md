# ADR-098: expand OOXML shared formula groups during XLSX import

Status: locally verified on Windows (2026-10-02).

## Context

Excel commonly stores formulas filled through a range as one formula master and
empty shared-formula follower tags. The previous bounded reader rejected the
master's `t`, `si`, and `ref` attributes, so ordinary Excel-authored workbooks
could not be imported.

## Decision

1. The streaming worksheet parser recognizes a shared master and follower tags,
   verifies one master per shared index, requires the master at the declared
   rectangle's top-left, and rejects followers outside that rectangle.
2. XLSX normalized IR schema 3 represents a follower as a reference to its
   direct formula master. Parent validation checks coordinate bounds and that
   the target is a direct formula in the same sheet; it rejects cycles and
   missing masters.
3. The server expands each follower with the existing formula engine's
   `copy_formula_text` before binding the resulting source to stable IDs. Cached
   values remain untrusted and are discarded. The XLSX crate does not depend on
   the formula crate.
4. Shared-formula group metadata is an input encoding detail. Export writes
   ordinary independent formulas, preserving their calculated meaning without
   reproducing Excel's storage optimization.

## Verification and limits

The committed Excel 16 fixture covers relative, mixed and absolute references,
ranges, IF/string literals and multiple shared groups. Parser golden and parent-IR
validation tests passed (25/25 for `spreadsheet-xlsx`); the importer formula-copy
unit test passed. An isolated PostgreSQL integration imported the fixture with
explicit `--allow-loss` for reported unrelated Excel metadata/style features and
confirmed durable formulas at shared followers, exported the document, and
reimported it with translated relative/mixed/absolute references intact. Server
unit tests passed (26/26 plus 3 worker CLI tests), and strict Clippy passed for
the touched crates. This does not close browser upload, OS process sandboxing,
broad OOXML coverage, or Excel calculation parity.
