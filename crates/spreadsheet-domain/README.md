# Sparse workbook domain

`workbook` has no persistence, transport, clock, UUID generator or evaluator
dependency. ID allocation belongs to the server boundary. It rejects nil IDs,
non-finite numbers, oversized UTF-8 text and invalid axes before publishing state.
Typed IDs do not interpret A1 coordinates. `CellValue::Debug` and `SheetSeed::Debug`
redact user content.

`candidate` prepares a private clone, applies one operation and validates it.
`apply_event` checks document identity, exact next version and canonical hash before
replacing the live projection. It rejects duplicate versions; journal idempotency
must return the original receipt instead of trying to replay a duplicate event.

`hydrate(document_id, version, seed, cells)` builds one sheet from bounded authored
content in production builds. It validates nonzero version, axes, addresses,
formula origins/binding versions/references and all existing memory budgets;
duplicate input addresses, including Blank entries, are rejected. It consumes at
most 100,001 iterator entries before rejecting excess input. It does not establish
that input came from a trusted server. Exact bound AST hydration preserves the
canonical content hash; reparsing formula source alone does not promise that hash.

`preview_candidate(operations, target_version)` applies one atomic preview batch
using a single workbook clone. It allows equal or increasing versions, checks
formula metadata against the target version, rejects creation and duplicate cell
addresses, and caps batches at 100,000 operations. Budget checks follow supplied
operation order; a clear can release capacity for a subsequent insertion. Failure
leaves the source projection intact. The caller must coalesce repeated edits to
the same cell before calling it. This API does not commit events or authenticate
versions and must not replace strict `candidate`/`apply_event` in durable writers.

Axes hold ordered IDs plus ID-to-position maps. Cells use a sparse BTreeMap keyed
by stable IDs. Blank removes content; empty text remains a text value. Current
bounds are 100,000 rows, 1,000 columns, 100,000 populated cells, 32 KiB UTF-8 text.
Additionally, the workbook has a 16 MiB aggregate UTF-8 cell text budget. A derived
byte counter checks replacement deltas before cloning a candidate; clearing and
overwriting text release the corresponding budget. The counter is reconstructed
through replay and is excluded from the canonical hash.
Sparse content is `CellContent::Value` or `CellContent::Formula`. Calculated formula
results are not stored in this authored projection. `SetCellFormula` canonical
operations contain an immutable bound Formula; client `CommandPayload` carries
only a bounded, redacted `FormulaSource`. Binding belongs to the server use case.
Formula origin must match its cell, binding version cannot exceed projection head,
and concrete references must point to live axes. Explicit InvalidReference AST
nodes retain typed REF semantics instead of pointing at missing identities.

Formula text budget includes original source, canonical text and AST-owned text.
An additional 32 MiB workbook AST charge counts 256 bytes per node, 4 bytes per
call argument handle and AST-owned text. This is a deterministic conservative
charge rather than measured allocator capacity. Counter deltas are checked before
candidate cloning; replacing/clearing formulas releases both budgets.

## Canonical domain hash v1

SHA-256 input begins with ASCII `PXEL-DOMAIN-1` followed by one zero byte.
All lengths and versions are unsigned 64-bit big-endian. IDs are 16 raw bytes.
No padding or platform-native integer representation is used.

1. Document ID, document version, sheet count.
2. Per sheet in visible order: sheet ID, UTF-8 name byte length and bytes,
   row count and ordered row IDs, column count and ordered column IDs, cell count.
3. Per populated cell in lexicographic stable-ID order: row ID, column ID, value.
4. Value tags: Blank `0`; Number `1` plus normalized binary64 bits big-endian;
   Text `2` plus UTF-8 byte length and bytes; Boolean `3` plus `0` or `1`.
5. Formula content uses tag `4` plus the 32-byte semantic formula hash. Legacy
   value-only workbook byte streams are exactly unchanged; the previous golden
   remains a regression test.

Stored cells never contain Blank. All zero numbers normalize to positive zero;
NaN/infinity cannot enter the projection. Actor, operation ID, calculated cache,
HashMap iteration, protobuf bytes and timestamps do not enter this hash.
Changing this encoding requires a versioned hash migration, not a silent edit.
An independently encoded Node SHA-256 golden guards the byte contract.

The formula digest begins with `PXEL-FORMULA-1` plus a zero byte. Fields: length-
prefixed source/canonical UTF-8; schema u32; length-prefixed compatibility profile;
origin sheet/row/column IDs; binding version u64; root node u32; node count u64;
postorder nodes with explicit tags and operands. Lengths/integers/binary64 bits
are big-endian. Reference modes use tags Absolute=0 and Relative=1 followed by
signed i64 delta. Error codes, function names, targets, edge ordering and missing
arguments enter the digest. `src/formula/hash.rs` defines the versioned node tag
mapping. No protobuf, Debug output or Rust enum discriminant is hashed. An
independent Node crypto golden checks a full formula digest.

## Tests and measurement

`cargo test -p spreadsheet-domain --all-features` exercises bounded fixtures,
candidate isolation, corrupt replay rejection, property-based convergence,
normalization and 100,000 populated cells.

`cargo bench -p spreadsheet-domain --features test-support --bench workbook`
measures allocation for 1,000/100,000 rows and candidate/hash cost for
1,000/100,000 sparse cells. Fixture construction uses a validated bulk loader
behind non-default `test-support`, delegating to production hydration; fixture
construction is excluded from the measured operation.
This is a baseline for the simple clone approach, not a claim of constant-time
updates. Results and hardware belong in RUNBOOK; shared runner timing is not an
acceptance threshold. The harness exits unsuccessfully if fixture setup fails.
