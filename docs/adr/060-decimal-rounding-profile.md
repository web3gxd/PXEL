# ADR-060: Bounded decimal ROUND family

Date: 2026-09-29. Status: ACCEPTED FOR THE VERIFIED ROUND FAMILY SLICE.

## Contract

[ROUND](https://support.microsoft.com/en-us/excel/functions/round-function),
[ROUNDUP](https://support.microsoft.com/en-us/excel/functions/roundup-function)
and [ROUNDDOWN](https://support.microsoft.com/en-us/excel/functions/rounddown-function)
each require two scalar arguments. Existing arithmetic coercion and error
propagation apply to both arguments. Blank number/digits become zero.
Fractional num_digits truncates toward zero; this subset rule needs manual Excel
verification. Arrays/implicit intersection remain unsupported.

| Function  | Direction                                      | Example                        |
| --------- | ---------------------------------------------- | ------------------------------ |
| ROUND     | Nearest decimal quantum; ties away from zero   | ROUND(-1.475,2) = -1.48        |
| ROUNDUP   | Away from zero if discarded digits are nonzero | ROUNDUP(-3.14159,3) = -3.142   |
| ROUNDDOWN | Toward zero                                    | ROUNDDOWN(-3.14159,3) = -3.141 |

Positive digits quantize fractional decimal places, zero quantizes integers,
negative digits quantize powers of ten left of the decimal point.

## Numerical algorithm

Input remains binary64. Quantize its shortest round-trip decimal representation
using bounded digit inspection and integer carry, then parse the resulting
decimal coefficient/exponent back to binary64. This avoids overflowing a scaled
binary64 intermediate and the common 1.005*100 midpoint problem. It defines
the PXEL decimal-rounding profile; it is not a claim about Excel's undocumented
internal algorithms or bitwise Excel parity. Values just below a midpoint retain
their shortest-representation digits and must not acquire an epsilon adjustment.

No platform pow/log/transcendental math, fast-math or large num_digits allocation.
At digits >=324 all finite binary64 values retain their shortest representation.
At digits <=-309, ROUND/ROUNDDOWN return normalized zero; ROUNDUP of a nonzero
value returns NUM because the decimal quantum is outside finite binary64.
Intermediate digit work is charged to the existing evaluator budget. Final
overflow becomes NUM. Signed zero is normalized. Finite values and fractional
digits are accepted across the profile without an arbitrary small digit limit.

Engine identity advances to 0.4.0. AST/authored-state/event/snapshot schemas and
compatibility profile name remain unchanged. Persisted old contexts require
authorized explicit recalculation; history is not rewritten. No new dependency.

## Required evidence

Microsoft examples, decimal midpoint/neighbor cases, negative precision,
fractional precision, extreme/subnormal values, arity/coercion/errors and
resource exhaustion in the common native/browser corpus; stateful patch parity;
real PostgreSQL dependent recalculation and UI reload; scalar benchmark.
All documentation-derived fixtures remain Excel-unverified. Full Excel precision
compatibility and portable noninteger POWER remain separate work.

Local evidence on 2026-09-30: formula/WASM unit suites passed; 202 common
native/browser fixtures and 1001 stateful workbooks/2002 patches converged.
Real PostgreSQL dependent recalculation and browser reload passed, including
DPR1/2. Scalar Criterion timings are recorded separately in
../benchmarks/2026-09-30-rounding-local.md. No manual Excel run is claimed.
