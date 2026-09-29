# ADR-063: Pure calendar and DATE parts

Date: 2026-09-30. Status: ACCEPTED FOR STATIC CALENDAR SLICE.

## Decision

DATE/YEAR/MONTH/DAY use pure integer Gregorian arithmetic and explicit DateSystem
1900/1904. Existing workbooks retain the explicit 1900 profile; selecting 1904 in
durable workbook metadata/import remains a separate slice. No host clock,
timezone or locale parsing; no new dependency. DATE returns Number(serial), as
Excel formulas return serial numbers, not an automatic cell formatting change.
Existing Value::DateTime retains its system when passed to date-part functions.

| Operation              | Contract                                                                                                                         |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| DATE(year,month,day)   | Three numeric-coerced scalars, fractional parts truncated before validation; truncated year0–1899 adds1900; year<0 or>=10000 NUM |
| Month/day overflow     | Month normalized with Euclidean division; first-of-month serial plus day−1                                                       |
| YEAR/MONTH/DAY(serial) | One scalar, fraction ignored, return integer calendar part                                                                       |
| 1900 serial60          | Fictional1900-02-29 preserved for DATE and part extraction                                                                       |
| 1900 serial0           | YEAR1900/MONTH1/DAY0, explicit Excel legacy profile                                                                              |
| 1904 serial0           | 1904-01-01                                                                                                                       |
| Final serial           | Nonnegative through9999-12-31; out-of-range NUM                                                                                  |
| Text                   | Existing numeric text coercion; locale/date-text parsing not supported                                                           |
| Errors / arity         | Existing spreadsheet errors propagated, wrong arity VALUE                                                                        |

Checked integer arithmetic admits bounded work independent of month/day magnitude;
unrepresentable integer intermediates NUM. Calendar conversion uses at most14
binary-search steps plus12 month steps and charges work. Numeric text coercion
checks32KiB cap and charges scans; limits/cancellation never become IFERROR fallback.

The two systems differ by1462 for dates on/after1904-01-01. DATE counts the fake
day when normalizing1900 month/day overflow. Leap-century rules otherwise follow
Gregorian4/100/400. Fractional negative year/month/day edge expectations are
project-derived and remain manual-Excel-unverified.

Sources: [DATE](https://support.microsoft.com/en-us/excel/functions/date-function?nochrome=true),
[date systems](https://support.microsoft.com/en-us/excel/change-the-date-system-format-or-two-digit-year-interpretation),
[1900 leap-year defect](https://learn.microsoft.com/en-us/troubleshoot/microsoft-365-apps/excel/wrongly-assumes-1900-is-leap-year).
Engine identity advances to0.7.0; old contexts require explicit epoch advance,
schemas unchanged. NOW/TODAY/RAND and persisted timezone/system configuration
remain required subsequent work, using injected persisted context rather than clocks.

## Evidence

Serial0/1/59/60/61, leap1900/2000/2100, month/day overflow, 1904/1462 offsets,
last supported day/fraction, coercion/arity/budget and generated serial roundtrips.
Native/WASM expected corpus, stateful patches, live dependent recalculation/reload
and pure calendar benchmark. Manual Excel and workbook1904 metadata are not claimed.

Actual evidence: five Rust calendar tests (including generated serial roundtrip,
DateTime system override, numeric text resource regressions and i64 boundaries),
337 expected/native/browser cases, 1001 stateful books/2002 patches, targeted live
PostgreSQL1/1 and browser dependent-edit/reload2/2 at DPR1/2. Benchmark report:
[calendar observations](../benchmarks/2026-09-30-calendar-local.md).
Fresh-context read-only review found no actionable defect; independently
transcribed full serial algorithm matched Python Gregorian dates except the
intentional zero/fake-day cases. That oracle did not execute Rust or Excel.
