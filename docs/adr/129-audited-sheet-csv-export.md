# ADR-129: audited selected-sheet CSV export

Status: locally verified, 2026-10-07; external deployment gates remain open.

Step 1 adds CSV download of one selected sheet. The request names stable sheet
identity and expected confirmed document version; stale versions reject before
release. UTF-8 with BOM, CRLF records and doubled quotes follow
[RFC 4180](https://www.rfc-editor.org/rfc/rfc4180). Comma, semicolon and tab match
the existing import choices. Empty trailing allocated axes are omitted; the
used rectangle includes every authored content cell. An empty sheet exports one
empty field. Styles, comments, merges and other sheets are not represented.

Values come from the captured confirmed workbook and a bounded native calculation
of that exact version/context. Formula source and optimistic operations are never
exported as results. Formula errors are rendered as error tokens; unredacted
arrays or failed calculations reject the whole download. Privacy analysis uses
the existing dependency-aware safe values when anonymization is required or
requested, including its existing array placeholders. Hidden rows/columns remain
part of the sheet's exported data.

The optional safe-text mode defaults on. Potential formula-leading text is
prefixed with an apostrophe before ordinary CSV quoting; real numeric values
remain numeric. Turning it off preserves original literal text bytes. The UI
states this transformation. Quoting alone does not prevent spreadsheet formula
interpretation, as documented by
[OWASP CSV Injection](https://owasp.org/www-community/attacks/CSV_Injection).
This does not claim safety after another application edits/re-saves the file.

CSV is an independent disclosure format, never an XLSX or clipboard audit alias.
The existing effective book/link policy, participant rights, early admission,
final version/policy check, durable audit and delivery lease/gate apply. Book
defaults enable CSV for the owner; saved old policies do not acquire a new format
implicitly. Guests need explicit book and link permission. One existing export
permit bounds conversion; output and the used rectangle are limited to 16 MiB
and 100000 fields. No plaintext temporary file or new dependency is needed.

Policy HTTP DTO 2, audit HTTP DTO 2 and capabilities DTO 3 introduce CSV.
Audit DTO 2 distinguishes preparation_failed (serialization/calculation/limits)
from privacy_analysis_failed. A concurrent policy/head denial retains its own
reason; neither failure releases bytes. The additional reason has a separate
database constraint migration; earlier migration checksums remain unchanged.
Readers retain their existing versions and default old capabilities to no CSV;
an old policy/audit version cannot contain CSV. Policy PUT 1 accepts only old
formats; PUT 2 accepts CSV. Unknown versions reject atomically.
Stored disclosure policy codec 2 is recorded explicitly in a new column and
authenticated in the existing record AAD. Old rows remain codec 1 and are read
without rewriting encrypted bytes. New policy writers, including privacy/share
flows, write codec 2. Workbook, wire and snapshot versions do not change.

Acceptance requires serializer limits/escaping/formulas tests, policy/version
boundary tests, real PostgreSQL member/guest denial and durable audit checks,
privacy dependency redaction, reload/restart and browser download/reimport.
Actual results belong in RUNBOOK. This slice does not close XLSX upload safety
or the remaining nine functional steps.
