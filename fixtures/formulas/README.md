# Formula corpus v1

`profile-v1.json` is a shared native/browser input corpus for
`pxel-excel-subset-v1`. It records expectations, not executed Excel results.
Every case currently has `verified_in_excel: false`; `excel_environment` is null.

Schema:

- `schema_version`: fixture format, independent of formula AST schema.
- `formula`: English invariant syntax, leading `=`.
- `origin`: `Sheet1!C1` in this corpus; adapters bind stable IDs separately.
- `cells`: map of `sheet-name!A1` to `{type,value}`. Missing cells are Blank.
  Split fixture keys at the final `!`; sheet names in fixture keys are decoded,
  without formula quote escaping. `type` is number/text/boolean/blank/error.
- `expected`: `{type,value}`; error values use displayed codes such as `#DIV/0!`.
  Diagnostic values use stable names such as `UnsupportedCompatibilityFeature`.
- `provenance.sources`: keys into the source dictionary. `basis` distinguishes
  published examples (`documented`), derived rules (`derived`), and explicit
  project contracts (`project_contract`). Empty source lists make no external
  compatibility claim.

Use binary64 parsing and normalized zero for numeric comparison. Error and
boolean variants must remain typed. Native/WASM result equality is exact for
this corpus; it does not assert bitwise Excel parity. Corpus cases do not
replace whole-workbook cycle, structural binding, budget, or dirty-graph tests.

Known evidence issue: current Microsoft Support AVERAGE documentation conflicts
with Microsoft Learn WorksheetFunction.Average on directly supplied boolean and
numeric-text arguments. The chosen counted behavior is explicitly provisional.
See [ADR-009](../../docs/adr/009-formula-slice.md).

A verified Excel fixture must add Excel edition/build, OS, locale, date systems,
exact inputs, observed output, observation date and verifier/provenance. Never
change `verified_in_excel` just because our evaluator passes the expectation.
