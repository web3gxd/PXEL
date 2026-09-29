# Formula engine: initial scalar profile

This crate is pure Rust and has no HTTP, SQL, UI, system clock, platform timezone
or wasm-bindgen dependency. Its parser/evaluator run unchanged on native/WASM.
`dependency`, `range_index` and `binder` implement the separate incremental/binding
contracts; syntax AST is not itself a stable-ID-bound persisted formula.

## API and formats

`parse(&str)` returns `DiagnosticAst { ast, spans }`. Its normalized `Ast` has
schema version 1, a flat node arena and root `NodeId`. Byte spans are indexed by
node ID and stay outside normalized syntax. Public AST nodes can be malformed;
evaluation and printing reject invalid indexes, schema, cycles/depth and limits.
UTF-8 byte offsets are not JavaScript UTF-16 offsets. A transport/UI adapter must
convert them against the same original source. Debug for values/AST redacts
document content; diagnostics contain codes and source offsets only.

`canonical(&Ast)` returns English function names, escaped strings/sheet names,
A1 mixed/absolute references and explicit operator parentheses. Source and
canonical text are independently bounded to 32 KiB; an expression that exceeds
the normalized-text bound is rejected before persistence by the binder.

`evaluate(&Ast, &resolver, &mut budget)` returns a spreadsheet `Value`, or a typed
resource/unsupported-profile diagnostic. Spreadsheet error values are separate
from calculation cancellation/failure. Unknown function -> `#NAME?`; invalid
function arity -> `#VALUE!`. Formula `=...` never executes application code.

The trusted `ReferenceResolver` resolves cells and visits ranges in row-major
order. It must immediately propagate visitor errors. Work is charged by the
engine per visited range cell; skipping cells without an error or producing
extra cells is rejected. A whole-range error can be reported by one error value.
No full range `Vec<Value>` allocation is necessary. Range evaluation is supported
as aggregate arguments; implicit intersection and dynamic arrays are excluded.

## Semantic contract

All fixture expectations in `fixtures/formulas/profile-v1.json` are documented,
derived or explicit project contracts, **not manually verified Excel results**.
Source provenance and documentation conflicts are retained in the corpus.

- Binary64; input literals must be finite, computed nonfinite -> `#NUM!`, zero
  normalization removes negative zero. `/0` -> `#DIV/0!`.
- Operators execute in AST order. Negation binds above exponentiation; repeated
  exponentiation is left associative. `=-2^2` -> 4; `=2^3^2` -> 64.
- `^` accepts integral exponents in [-1024,1024]; otherwise it returns
  `UnsupportedCompatibilityFeature`. Fixed exponentiation by squaring is used.
  Negative exponents take the reciprocal **before** squaring, preserving
  representable subnormals such as `2^-1024`. This refines the earlier proposal
  of reciprocal-after-squaring; no persisted formula version used that proposal.
  Nonfinite reciprocal -> NUM; zero with negative exponent -> DIV/0; `0^0` -> NUM.
- SUM/AVERAGE/MIN/MAX accept 1–255 arguments. Direct booleans and numeric text
  convert to numbers; text/bools/blanks obtained from references/ranges are
  ignored. Direct unparseable text -> VALUE. Missing argument -> numeric zero.
  Empty numeric set: SUM/MIN/MAX -> 0, AVERAGE -> DIV/0. Reduction order is fixed.
- IF takes 2–3 arguments, evaluates only the selected branch. Omitted third
  argument -> FALSE; an explicitly empty selected argument or blank reference
  -> 0; explicit empty text stays text. Numeric tests compare against zero,
  TRUE/FALSE text is accepted; other text -> VALUE.
- Comparisons do not equate text `"1"` with number 1. Mixed scalar ordering is
  number < text < boolean. Blank compares as zero/empty text/FALSE according to
  its other operand. Text compares through deterministic Unicode uppercase;
  Excel locale-specific collation parity is not promised.
- Concatenation formats primitive values invariantly. DateTime/Array are explicit
  value types for subsequent stages; date functions, spill and array operations
  are not implemented here. Arithmetic can coerce a DateTime serial; timezone is
  never consulted.

- AND/OR take 1–255 arguments, ignore referenced text/blanks and return VALUE
  when no logical values participate. Numeric zero is false. They evaluate all
  conditions, preserving later errors even after a decisive boolean; range
  traversal is budgeted and checked. NOT takes one scalar and inverts the same
  scalar coercion as IF. Arrays/implicit intersection remain unsupported.
  Engine identity is `pxel-formula-0.4.0`; old persisted contexts require a new
  authorized calculation epoch. See ADR-055 and the common fixture corpus.

- ABS takes one scalar; MOD takes two and follows the divisor sign. Blank is
  coerced to zero, a zero divisor gives DIV/0, and unsafe numeric remainder
  bounds give NUM. See ADR-059 for the supported precision/coercion profile.

- ROUND/ROUNDUP/ROUNDDOWN take two scalar arguments and quantize the shortest
  round-trip decimal representation, with fractional precision truncated toward
  zero. Work is bounded and budgeted; overflow becomes NUM. This explicit PXEL
  profile is described in ADR-060 and does not promise bitwise Excel parity.

Bounds: 32 KiB source, 4,096 AST nodes, AST depth 128 (parser call depth 256 to
admit canonical parentheses), 255 call arguments, 100,000
cells per rectangular range, 1,000,000 work units per evaluation. Resource limits
produce typed errors and never substitute Blank/0. Syntax supports quoted sheets
with doubled apostrophes, escaped text quotes, mixed references, comparisons,
percent, concatenation and the five scalar functions. Named ranges, external/3D
references, array literals, locale separators and implicit intersection are not
part of this slice.

## Verification

`cargo test -p spreadsheet-formula` runs scalar/reference fixtures, canonical
round trips, generated UTF-8 parsing (both raw and prefixed `=`), malformed arena
and depth tests, range budgets and the separate graph/binder suites.
`cargo check -p spreadsheet-formula --target wasm32-unknown-unknown` proves build
portability only; browser/native differential tests belong to the WASM adapter.
