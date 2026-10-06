# ADR-128: browser tabular import

Status: CSV slice locally verified; XLSX slice pending, 2026-10-07.

Browser imports create a new book through the existing fully validated,
encrypted `create_pxel_imported` transaction. They never write directly to cell
tables or substitute client validation for server checks. The existing file
publication path supplies deterministic IDs, receipts and metadata in one commit.

The first vertical slice accepts UTF-8 CSV with optional BOM, comma/semicolon/tab
separators, quoted fields, escaped quotes, CRLF/LF and quoted newlines. It caps
source bytes, dimensions, field bytes and total cells. Malformed quoting,
inconsistent row widths, invalid UTF-8 and conversion failures reject the whole
file. Per-column text/number/boolean types are explicit; default text preserves
identifiers, leading zeros and formula-looking text. An optional first header
row always remains text. No CSV cell becomes executable formula implicitly.

Authorization and rate checks precede JSON body buffering. The shared import/
export permit is held from body reading through preview or durable publication.
HTTP JSON is limited to 24 MiB; body reading has a 15-second deadline. Decoded
source remains limited to 16 MiB and conversion/candidate limits apply separately.

Preview/import DTO version 1 is separate from wire and durable codecs. Requests
contain base64 source, delimiter, column types, header choice, title, fresh-book
ID and request ID. Preview returns bounded samples, dimensions and a digest of
the complete validated source/options (domain separator and source byte length
precede the source). Publication requires the preview digest,
revalidates the same source/options and current member authorization, then uses
the existing deterministic publication path. Changing settings requires a new
preview. File bytes and samples are never logged or written to plaintext disk.

The browser uses the existing modal and custom select controls, with file and
conversion errors, preview and explicit creation. Closing a preview aborts its
request; closing during publication waits for its outcome. Late file/preview
responses cannot overwrite a newer selection. Retrying a lost
publication response retains its document/request IDs and original payload.

Focusing an existing cell starts an unmodified editor draft. Blur/Enter without
input neither reinterprets literal text nor creates an authored operation or
calculation overlay. Explicit initial typing or an input event still authors a
write, including same-value rewrites. This preserves imported text identifiers
and formula-looking text and the established collaboration/history barriers.

XLSX upload and CSV export are subsequent sub-slices of step 1; they are not
claimed implemented by this initial CSV slice. XLSX upload retains ADR-099's
OS sandbox gate until resource/filesystem/network controls and malware policy
have been addressed. No weakened parser or trusted client AST is introduced.

CSV syntax reference: [RFC 4180](https://www.rfc-editor.org/rfc/rfc4180).
Actual verification, failures and remaining gates belong in RUNBOOK.
