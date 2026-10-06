# ADR-107: encrypted comments on stable cells

Date: 2026-10-03. Status: IMPLEMENTED; local PostgreSQL and browser checks passed.

Comments are an independent append-only encrypted journal with its own revision.
They do not change workbook versions, formula results or workbook state hashes.
Each record binds the document, stable sheet/row/column IDs, author, request ID,
revision and operation purpose into the storage envelope AAD. PostgreSQL remains
authoritative and the journal transaction commits before a mutation is returned.

The profile supports listing comments at a stable cell address, adding a comment,
deleting one's own comment and owner moderation. It has no replies or mentions.
Text is limited to 4 KiB UTF-8; pages contain at most 100 records and encoded
responses are capped at 512 KiB. Removed axis IDs hide their comments. Inserting
or moving cell contents does not move comment identity; restoring the same stable
IDs makes the comments visible again.

Member routes require a member principal. Shared routes require a valid guest
session for the same document and `can_comment`; view-only guests cannot write.
Deletion checks the authenticated author or owner in PostgreSQL. Comment text
and request bodies are not logged. Plaintext exists only in the bounded server
process path and is erased from owned request buffers after use.

Verification: the encrypted-comments PostgreSQL test passed; Chromium DPR1/2
acceptance adds and reads a guest comment through the real UI and closes that
guest after owner revocation. The storage shares/comments tests and workspace
gates are recorded in RUNBOOK. This does not establish replies, mentions,
distributed comment delivery, or production owner authentication.

The 2026-10-05 full Chromium acceptance passed 198/198 at DPR1/DPR2 and includes
the comment-link workflow, adding a cell comment, revoking the guest and seeing
the terminal closed-access state on an idle session. Full workspace, SQLx and
dependency gates and their local-only limits are recorded in RUNBOOK.
