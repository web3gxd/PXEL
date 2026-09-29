# ADR-061: Portable POWER and exponentiation

Date: 2026-09-30. Status: ACCEPTED FOR THE VERIFIED POWER SLICE.

## Decision and old behavior

Previous `^` supported integer exponents with magnitude at most 1024 using
exponentiation by squaring; POWER was not recognized. Fractional exponents
returned an unsupported-feature diagnostic. Replace both with a shared portable
implementation using pinned libm 0.2.16, default features disabled and
force-soft-floats enabled. No host powf, architecture-specific optimizations,
fast-math, FMA or user-defined math implementation.

The dependency is an ordinary numerical library, not a formula engine. Its
registry metadata and downloaded sources identify MIT licensing, Rust 1.63
minimum and rust-lang/compiler-builtins maintenance. License/advisory and
feature-unification checks are required before acceptance. Dependency unsafe is
not a change to the prohibition on unsafe in our own code.

## Contract and examples

[Microsoft POWER documentation](https://support.microsoft.com/en-us/excel/functions/power-function)
specifies two required arguments and permits `^` as an alternative. Examples
include POWER(5,2)=25, POWER(98.6,3.2) approximately 2401077.222 and
POWER(4,5/4) approximately 5.656854249. Published rounded examples are compared
using their stated precision; exact corpus values are project fixtures and
must not be called manually Excel-verified.

| Case                                      | Result                                |
| ----------------------------------------- | ------------------------------------- |
| Positive base, finite fractional exponent | portable binary64 power               |
| Negative base, integral exponent          | signed portable binary64 power        |
| Negative base, nonintegral exponent       | NUM                                   |
| Zero base, positive exponent              | normalized zero                       |
| Zero base, zero exponent                  | NUM                                   |
| Zero base, negative exponent              | DIV/0                                 |
| Overflow / nonfinite library output       | NUM                                   |
| Underflow                                 | binary64 subnormal or normalized zero |
| Wrong arity / nonnumeric text             | VALUE                                 |
| Blank / boolean / numeric text            | existing arithmetic scalar coercion   |
| Existing spreadsheet error in an argument | propagated                            |

No exponent magnitude cap, no array/implicit intersection support. Unary minus
and left-associative `^` keep the parser's existing Excel profile. Charge a fixed
64 work units before invoking the bounded library algorithm; budget/cancellation
errors remain evaluator errors and cannot be swallowed by IFERROR. This cost is
an admission accounting choice, not a hard CPU-time guarantee or benchmark result.

Engine identity advances to 0.5.0, requiring a new persisted calculation epoch
for old contexts. AST/event/snapshot codecs remain unchanged; old contexts and
historical cache identities are never rewritten. Binary64 results may differ
from the old squaring sequence for some integer powers, hence the identity bump.

## Acceptance evidence

Expected corpus for fractional powers, zero/domain errors, coercion, overflow,
underflow, large exponents, unary precedence and budget exhaustion; native/browser
normalized-hash parity and seeded stateful book patches; PostgreSQL dependent
recalculation, live UI/reload and scalar benchmark. Add generated finite base/
exponent pairs to differential parity. Bitwise Excel parity is not promised.
Linux/other architectures and manual Excel verification remain separate gates.

Local evidence: 236 native/browser fixtures and 1001 stateful books with 2002
patches converged; formula/WASM unit suites, live PostgreSQL 41/41 and targeted
UI DPR1/2 passed. Cargo-deny licenses/advisories passed, feature tree showed only
force-soft-floats. Independent read-only review found no concrete contract issue.
Scalar timing conditions are in [benchmark report](../benchmarks/2026-09-30-power-local.md).
