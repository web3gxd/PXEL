# ADR-133: XLSX active-content rejection

Status: implementation in progress, 2026-10-07.

Browser import will parse a bounded package in the isolated worker and publish
only validated normalized workbook content, never the original ZIP or opaque
parts. Original uploaded packages must not become downloadable attachments or
durable plaintext files. This is a content-disarm policy, not an antivirus claim.

Reject the complete package for macros/VBA, ActiveX and control properties,
Excel macro/dialog sheets, external relationships/links, embedded objects,
connections, custom XML and DTDs. Existing pivot-table rejection is retained
until pivot import has an explicit supported contract. Other unsupported passive
features keep the existing compatibility-report contract; they are never copied
into the normalized workbook.

ActiveX and macro/dialog parts were previously accepted by package inspection
as unsupported passive parts. Reject them before XML decoding/normalization.
Synthetic committed fixtures contain only inert `<payload/>` XML, no executable
or malware sample. Verify both library inspection and actual WASI backend refusal
without partial preview, source echo or native fallback.

This does not establish a complete malware detector, host-process memory cap,
browser publication endpoint or Linux deployment verification. Those gates remain
open. No new dependencies, persisted codecs or wire versions are introduced.
