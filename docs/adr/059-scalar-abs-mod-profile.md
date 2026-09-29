# ADR-059: Scalar ABS and MOD

Date: 2026-09-29. Status: ACCEPTED FOR THE MATH SLICE.

## Specification

Microsoft documents [ABS](https://support.microsoft.com/en-us/excel/functions/abs-function)
as the absolute value and [MOD](https://support.microsoft.com/en-us/excel/functions/mod-function)
as a divisor-signed remainder, with DIV/0 for a zero divisor. This subset uses
scalar arguments only; arrays and implicit intersection remain unsupported.

| Function | Arity     | Numeric policy                                          | Blank                                             | Errors                                                                 |
| -------- | --------- | ------------------------------------------------------- | ------------------------------------------------- | ---------------------------------------------------------------------- |
| ABS      | Exactly 1 | Existing scalar numeric coercion; finite absolute value | 0                                                 | Spreadsheet errors propagate                                           |
| MOD      | Exactly 2 | Existing coercion, left to right; n - d × floor(n / d)  | Number blank becomes 0; divisor blank gives DIV/0 | First evaluated spreadsheet error propagates; zero divisor gives DIV/0 |

Numeric text and booleans follow the existing arithmetic adapter, including
references. Nonnumeric/empty text gives VALUE. Arity mismatch gives VALUE.
Resource/cancellation failures remain typed evaluator errors. No range expansion
or implicit intersection is added for these scalar functions.

| Example               | Expected subset result                                       |
| --------------------- | ------------------------------------------------------------ |
| ABS(-3)               | 3                                                            |
| ABS(-0)               | Normalized positive zero                                     |
| ABS("-2.5")           | 2.5                                                          |
| MOD(3,2), MOD(-3,2)   | 1                                                            |
| MOD(3,-2), MOD(-3,-2) | -1                                                           |
| MOD(5.5,2)            | 1.5                                                          |
| MOD(3,0)              | DIV/0                                                        |
| MOD(1e308,1e-308)     | NUM: nonfinite quotient                                      |
| MOD(-5e-324,1e308)    | NUM: cancellation/underflow cannot preserve remainder bounds |

## Determinism and versioning

Use binary64 division, floor, multiplication and subtraction in that order,
without fused multiply-add, platform remainder, fast-math or epsilon correction.
Nonfinite results become NUM. A nonzero remainder whose sign differs from its
divisor or whose magnitude reaches/exceeds it becomes NUM. Signed zero is
normalized by the shared numeric constructor. This is an explicit numerical
subset; extreme cancellation and Excel-specific precision corrections are not
claimed compatible. Documentation-derived fixtures remain Excel-unverified.

Engine identity advances to `pxel-formula-0.3.0`; native, WASM and browser guards
agree. AST, authored domain state, compatibility profile and persisted formats
are unchanged. Existing explicit contexts retain their identity and require
the authorized “Пересчитать” action to advance the epoch. No historical event
or snapshot is rewritten. No dependency is added.

## Evidence

Native unit/property tests, common native/browser fixtures, stateful patch parity,
real PostgreSQL dependency recalculation and browser reload cases cover this
slice. Criterion measures the scalar evaluator excluding parsing/storage/UI;
[conditions and measurement](../benchmarks/2026-09-29-abs-mod-local.md).
Full rounding/POWER/text/date/lookup functions and manual Excel results remain
separate implementation work.
