# ADR-087: independent sparse cell formatting

Status: Core profile implemented; broader XLSX, cross-platform and production gates remain.

## Context and intended slice

ADR086 is committed as d41a7bb. The current sparse CellContent stores only values
and formulas. ClearCell, existing copies/moves, axis preimages, snapshot3 and the
XLSX intermediate model have no formatting representation. Formatting must now
become a durable independent property, including on cells without content.

This decision prepares a complete path: typed styles, guarded scalar/range writes,
versioned journal/snapshots, server authorization, confirmed and optimistic client
state, Canvas rendering, history and native/browser state parity. The core profile
is implemented and locally verified. Bounded profile-1 XLSX formatting round-trip
is completed under [ADR-088](088-xlsx-cell-style-profile.md); merges, broader XLSX
compatibility, and structural style preservation remain follow-up work.

## Pre-planning review

The think-tank skill was applied as a single-context simulated discussion with
fictional roles: moderator Pavel, domain engineer Irina, interaction designer
Lena, recovery engineer Oleg and test engineer Mikhail. These are analysis roles,
not external endorsements or interviews. The user has already authorized all
remaining phases and autonomous routine choices; no new approval pause is needed.

Irina prefers a separate sparse style map: an empty styled cell must survive
ClearCell. Lena agrees but challenges whole-style replacement: concurrent font
and fill changes should preserve one another. Oleg challenges interning style IDs
before measurement: it introduces snapshot identity and migration work. Mikhail
points out that a complete style hash without Worker hydration would break parity
even when numeric calculation remains unchanged. The moderator separates
independent authored properties from derived calculation cache and legacy events.

Consensus: sparse normalized styles; property patches rather than whole-style
last-write replacement; frozen exact preimages; no reinterpretation of legacy
content-only Copy/Move; explicit adapter schemas; no range/structural partial
application. A compact shared style table is reconsidered only if measured memory
requires it. This trades some repeated small style values for simpler durable
identity and recovery. No new libraries or services are needed.

## Candidate contract to validate

- Store non-default CellStyle separately from CellContent, keyed by stable address.
  Styled blanks exist; content clear leaves style intact. Default normalization
  removes empty style entries. Unknown fields/enums are typed errors.
- Use typed per-property patches for font, fill, border, alignment and number
  format. Clearing a property differs from leaving it unchanged. A committed
  range write freezes target IDs and exact before/after properties; commands are
  bounded by cells, decoded bytes and persisted envelope bytes together.
- Font/fill edits and content edits commute. Competing writes to the same style
  property follow server order. Inverse admission inspects the written properties,
  not all content in the cell, and retains foreign equal-value barriers.
- Candidate validation, current ACL, activation floors, journal-derived inverse,
  CAS and durable commit still precede publication. Undo creates a new event.
  Discovery must classify new style originals/compensations once supported.
- Preserve legacy content-only event semantics. New UI copy/paste/move behavior
  that carries formatting requires an explicit new versioned operation; do not
  silently upgrade previously stored Copy/Move events or erase source formatting.
- Axis deletion/restoration captures exact formatting preimages in additive
  schema 26/27 events. See [ADR-092](092-structural-style-preimages.md).
- New style state participates in canonical domain hashing and snapshot encoding.
  Existing style-free hash fixtures and snapshot readers remain compatible.
  Worker hydration/patch adapters must represent the same authored style state
  under an explicit schema; numeric formula semantics do not change by intuition.
- Render only visible cells, including styled blanks. Editing/navigation remains
  independent from rendering. Toolbar actions use the existing selection and
  command queue, and show confirmed/pending status through the existing flow.

## Frozen core contract

The conflict key is a leaf, not a whole font/alignment object. Sixteen ordered
properties are font family, font size, bold, italic, underline, strike, font color,
fill color, top/right/bottom/left border, horizontal alignment, vertical alignment,
wrap and number format. A border edge is an atomic line/color pair. Typed
StyleChange variants contain Change<T> = Set(T) | Reset; absence means unchanged.
Duplicate keys and empty patches are errors. Canonical normalization preserves a
write's footprint even when Set(default) becomes Reset: an equal-value foreign
write still remains a durable conflict barrier.

Default profile1 is Arial, 10pt, black font, white fill, normal weight, no italic,
underline or strike, General horizontal alignment, Bottom vertical alignment,
no wrap, no borders and General number format. These are authored defaults, not
OS-dependent font discovery. Font size is integer hundredths of a point in our
bounded 1..200pt profile, not a claim about Excel's maximum. Font family is a
validated 1..128-byte UTF-8 name without controls or outer whitespace; the browser
uses installed fonts/fallback and never loads a font URL from the document.
Profile1 has no inherited row/column styles. A later inheritance feature needs
an explicit profile/migration decision for inherited versus explicit-default
properties; it must not silently change the meaning of existing Reset events.

Underline, alignment, border line and number format use closed enums. Initial
number formats are General, Integer, Decimal2, Percent0, Percent2, Scientific2
and DateIso; formatting must have its own fixtures and must not alter numeric
values. This is a limited profile, not arbitrary Excel format-string support.

Preserve the existing per-sheet content capacity: count the union of content and
styled addresses, at most100000 per sheet, without double-counting a styled value.
Add a workbook-wide limit of100000 styled entries and64MiB allocation charge;
initial charge is512bytes/entry plus font-name bytes, an admission budget rather
than a measured allocator bound. Measure actual memory before optimizing. Do not
retroactively introduce a workbook-wide content cap for old style-free books.

Before this integration, limits differed by boundary: durable candidates allowed
100000 content cells per sheet, while hydrate_sheets, snapshot3 restoration and
Worker initialization allowed100000 content cells across the workbook. This was an existing
integration limitation, not permission to pretend all candidate states hydrate.
Integration issue STYLE-01: preserve legacy candidate semantics, make snapshot4
restoration support valid candidates within its explicit binary/decoded budgets,
and publish a separate bounded Worker profile with a typed preview refusal when
too large. Completion requires multi-sheet boundary/round-trip fixtures and no
false parity claim. Domain hydration now counts input cells per sheet with the
existing global text/AST budgets retained. A real100002-cell two-sheet snapshot
image restoration preserves the domain hash. Legacy snapshot codecs retain their
old reader profiles; snapshot4 binary limits and Worker initialization still need
their own verification. Dependency: Workbook/snapshot4/Worker integration, before
claiming the style vertical slice complete.

Commands address explicit stable cells, at most1000 addresses and16 distinct leaf
changes per address. This avoids interpreting stale range geometry at current
axes. Incoming/decoded, retained, forward/preimage and persisted4MiB envelope
budgets all apply; reserve the immediate inverse size before accepting an original.
Styles require the same activation-floor and receipt-first admission as content.

Reserve storage families23/24 for style originals/compensations, semantic event
hash tags25/26 and snapshot4 with readers1..4. Old snapshots reject new style
fields rather than silently dropping them. Conditional canonical style suffix
encodes profile1 and ordered stable addresses/properties; no suffix for an empty
style map preserves old hash fixtures. No legacy event bytes are rewritten.

Inverse guards reduce exact own pairs per(address,leaf). Foreign equal-value
writes and foreign compensations remain barriers; content and other leaves do not
enter that ledger. Conversely style changes must not be added indiscriminately
to content-history footprints. Destructive original/undo/redo axis paths require
style preimages or reject before commit, including styled blank inserted rows.

Current Worker result_hash covers calculated results, not the authored Workbook
hash. Worker init4 must support optional calculation context, styles and all
hash-covered metadata using real domain hydration, including retired IDs and
activation floors. Also hydrate and validate last_axis_change_version for
structural semantics; that derived marker is not currently included in the
Workbook hash. Patch2/output2 distinguish
calculated hash from independently computed authored hash. No server hash echo
is parity evidence. Init4 must also hydrate the actual normalized bound formula
AST: rebinding authored A1 source at current axes can change targets after an
insert/delete/move. The additive HTTP formula field bound_formula_wire_hex carries
the versioned public BoundFormula message as bounded lowercase hex. The adapter
decodes and validates it rather than deriving canonical references from text.
These transport bytes are not used as a domain hash. Old adapter schemas reject
style payloads. Private candidate
and serialization failure rollback remains atomic; styles create no new dirty
formula dependencies or volatile epochs.

## Profile 2 text overflow extension (2026-10-02)

The original style profile used durable property tag 15 for the boolean `wrap`.
Keep that reader intact: absent `StyleWrite.profile_version` means profile 1,
and its boolean continues to decode as Clip or Wrap. Durable profile 2 reuses tag
15 as the closed Clip/Wrap/Overflow enum and is chosen when Overflow needs to be
represented. The independent public wire profile adds a distinct enum oneof
while retaining the old boolean oneof. Style snapshot and projection schemas
accept both durable versions; old authored hashes for Clip and Wrap stay
byte-compatible, and Overflow uses the profile-2 hash prefix.

`CellStyle` carries one canonical `TextOverflow` leaf, defaulting to Clip. The
browser accepts old projection objects containing `wrap` and normalizes them to
Clip or Wrap. New projections and operations use `text_overflow`; range style
aggregation includes mixed state and the existing guarded style operation
provides durable Undo/Redo.

Canvas Overflow applies to text with left/general alignment and extends across
visible, default-style empty cells. It stops at content, formatting or a merge,
including formulas whose displayed result is empty. Both the authored value and
the optimistic display are considered so a draft value blocks text before
confirmation. Incremental invalidation includes both old and new spans, clears
downstream cells before repainting their source, and does not change the
confirmed calculation projection. A DPR1/DPR2 browser regression checks actual
Canvas pixels across adjacent cells, a value blocker, an empty-result formula,
Undo, style Undo and reload. Wrap and Clip continue to paint within their cell
bounds.

The bounded XLSX style profile still represents only OOXML `wrapText`; it does
not preserve PXEL's distinct Clip versus Overflow mode. This does not change
the XLSX profile contract or claim broader OOXML round-trip support.

## Remaining implementation checks

The typed style contract, journal and snapshot codecs, Worker hydration, client
optimistic state, Canvas rendering, formatting history, native/browser parity,
clip/overflow/wrap and bounded profile-1 XLSX import/export/reimport are
implemented and locally verified. The specific XLSX evidence and its limits are recorded in ADR-088 and
the linked handoff.

Rich formatting copy/move and a Windows sparse-style memory reference measurement
are recorded in ADR-094/095 and the roadmap. Broader XLSX compatibility and
cross-platform memory measurements remain open; benchmarks must record the real
machine/configuration.

## Consequences

Domain, storage codec, wire adapter, actor/application, snapshot mapping, Worker,
projection decoder and Canvas changed together in tested increments. No empty
crate was introduced. RUNBOOK records each increment. This ADR accepts the core
formatting profile; it does not close the broader XLSX or deployment gates.
