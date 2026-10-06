//! Position-based file adapter. Durable import identities are fresh and retry-stable.
use crate::pxel_file::Sheet;
use crate::{documents::CellStyleView, pxel_file::*};
use sha2::{Digest, Sha256};
use spreadsheet_domain::{
    axis_size::{AxisSizeChange, AxisSizeTarget},
    style::CellStyle,
    workbook::*,
};
use spreadsheet_formula::{
    binder,
    diagnostics::{self, SeriesFormula},
};
use spreadsheet_storage::{ColumnFilter, PersonalFilter, PersonalFilters, PrivacyRules};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn invalid<T>(_: T) -> FileError {
    FileError::Invalid
}
pub struct Prepared {
    pub events: Vec<CommittedEvent>,
    pub personal: PersonalFilters,
    pub privacy: PrivacyRules,
    pub book: Workbook,
}
struct Ids {
    namespace: [u8; 32],
    timestamp: [u8; 6],
    next: u64,
}
impl Ids {
    fn new(request: Uuid, document: DocumentId, digest: &[u8]) -> Self {
        let mut hash = Sha256::new();
        hash.update(request.as_bytes());
        hash.update(document.as_bytes());
        hash.update(digest);
        let mut timestamp = [0; 6];
        timestamp.copy_from_slice(&request.as_bytes()[..6]);
        Self {
            namespace: hash.finalize().into(),
            timestamp,
            next: 0,
        }
    }
    fn id(&mut self) -> [u8; 16] {
        let mut hash = Sha256::new();
        hash.update(self.namespace);
        hash.update(self.next.to_be_bytes());
        self.next += 1;
        let digest = hash.finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[..6].copy_from_slice(&self.timestamp);
        bytes[6] = (bytes[6] & 15) | 0x70;
        bytes[8] = (bytes[8] & 63) | 0x80;
        bytes
    }
}
fn append(
    current: &mut Workbook,
    events: &mut Vec<CommittedEvent>,
    actor: ActorId,
    ids: &mut Ids,
    operation: Operation,
) -> Result<(), FileError> {
    let version = DocumentVersion::new(
        current
            .version()
            .get()
            .checked_add(1)
            .ok_or(FileError::Limit)?,
    )
    .map_err(invalid)?;
    let candidate = current.candidate(&operation, version).map_err(invalid)?;
    let event = CommittedEvent {
        document_id: events
            .first()
            .map_or(current.document_id(), |e| e.document_id),
        operation_id: OperationId::from_bytes(ids.id()).map_err(invalid)?,
        principal: EventPrincipal::User(actor),
        version,
        operation,
        state_hash: candidate.hash(),
    };
    spreadsheet_storage::validate_import_event_size(&event).map_err(invalid)?;
    events.push(event);
    *current = candidate;
    Ok(())
}
fn address(seed: &SheetSeed, row: u32, column: u32) -> Result<CellAddress, FileError> {
    Ok(CellAddress {
        sheet: seed.id,
        row: *seed.rows.get(row as usize).ok_or(FileError::Invalid)?,
        column: *seed
            .columns
            .get(column as usize)
            .ok_or(FileError::Invalid)?,
    })
}
fn location(book: &Workbook, address: CellAddress) -> Result<Location, FileError> {
    let (index, sheet) = book
        .sheets()
        .iter()
        .enumerate()
        .find(|(_, s)| s.id() == address.sheet)
        .ok_or(FileError::Invalid)?;
    Ok(Location {
        sheet: index as u32,
        row: sheet.row_position(address.row).ok_or(FileError::Invalid)?,
        column: sheet
            .column_position(address.column)
            .ok_or(FileError::Invalid)?,
    })
}
fn hardcodes(book: &Workbook) -> Result<Vec<(Location, String)>, FileError> {
    let mut owned = Vec::new();
    for sheet in book.sheets() {
        for (addr, content) in sheet.cells() {
            if let CellContent::Formula(f) = content {
                owned.push((
                    *addr,
                    sheet.row_position(addr.row).ok_or(FileError::Invalid)?,
                    sheet
                        .column_position(addr.column)
                        .ok_or(FileError::Invalid)?,
                    spreadsheet_formula::bound::render_current(f, book).map_err(invalid)?,
                ));
            }
        }
    }
    let input: Vec<_> = owned
        .iter()
        .map(|(address, row, column, source)| SeriesFormula {
            address: *address,
            row: *row,
            column: *column,
            source,
        })
        .collect();
    diagnostics::analyze(&input)
        .map_err(invalid)?
        .into_iter()
        .map(|w| {
            Ok((
                location(book, w.address)?,
                w.fingerprint.iter().map(|b| format!("{b:02x}")).collect(),
            ))
        })
        .collect()
}
fn same(a: &Location, b: &Location) -> bool {
    a.sheet == b.sheet && a.row == b.row && a.column == b.column
}

pub fn prepare(
    input: &Book,
    document: DocumentId,
    actor: ActorId,
    request: Uuid,
    digest: &[u8],
) -> Result<Prepared, FileError> {
    if input.schema_version != 1 {
        return Err(FileError::Version);
    }
    if input.engine_version != spreadsheet_collab::calculation::ENGINE_VERSION
        || input.compatibility_profile != spreadsheet_collab::calculation::COMPATIBILITY_PROFILE
    {
        return Err(FileError::Version);
    }
    if !spreadsheet_storage::valid_title(&input.title)
        || input.sheets.is_empty()
        || input.sheets.len() > 64
    {
        return Err(FileError::Invalid);
    }
    if input.settings.filters.len() > 5
        || input.settings.hidden_hardcode_warnings.len() > 1000
        || input.settings.ignored_sensitive_cells.len() > 1000
    {
        return Err(FileError::Limit);
    }
    let mut axes = 0usize;
    let mut positions = 0usize;
    let mut ids = Ids::new(request, document, digest);
    let mut seeds = Vec::new();
    for sheet in &input.sheets {
        if sheet.rows == 0
            || sheet.rows as usize > MAX_ROWS
            || sheet.columns == 0
            || sheet.columns as usize > MAX_COLUMNS
        {
            return Err(FileError::Limit);
        }
        axes = axes
            .checked_add(sheet.rows as usize + sheet.columns as usize)
            .filter(|n| *n <= 1_000_000)
            .ok_or(FileError::Limit)?;
        let cells: BTreeSet<_> = sheet.cells.iter().map(|c| (c.row, c.column)).collect();
        let styles: BTreeSet<_> = sheet.styles.iter().map(|c| (c.row, c.column)).collect();
        if cells.len() != sheet.cells.len() || styles.len() != sheet.styles.len() {
            return Err(FileError::Invalid);
        }
        positions += cells.union(&styles).count();
        if positions > 100_000 {
            return Err(FileError::Limit);
        }
        if cells
            .union(&styles)
            .any(|(r, c)| *r >= sheet.rows || *c >= sheet.columns)
        {
            return Err(FileError::Invalid);
        }
        if input.anonymized
            && sheet
                .cells
                .iter()
                .any(|c| matches!(c.content, Content::Formula(_)))
        {
            return Err(FileError::Invalid);
        }
        seeds.push(SheetSeed {
            id: SheetId::from_bytes(ids.id()).map_err(invalid)?,
            name: sheet.name.clone(),
            rows: (0..sheet.rows)
                .map(|_| RowId::from_bytes(ids.id()).map_err(invalid))
                .collect::<Result<_, _>>()?,
            columns: (0..sheet.columns)
                .map(|_| ColumnId::from_bytes(ids.id()).map_err(invalid))
                .collect::<Result<_, _>>()?,
        });
    }
    let mut current = Workbook::empty(document);
    let mut events = Vec::new();
    for (i, seed) in seeds.iter().enumerate() {
        append(
            &mut current,
            &mut events,
            actor,
            &mut ids,
            if i == 0 {
                Operation::CreateWorkbook {
                    sheet: seed.clone(),
                }
            } else {
                Operation::AddSheet {
                    sheet: seed.clone(),
                }
            },
        )?;
    }
    for (sheet, seed) in input.sheets.iter().zip(&seeds) {
        let cells: BTreeMap<_, _> = sheet
            .cells
            .iter()
            .map(|c| ((c.row, c.column), &c.content))
            .collect();
        let styles: BTreeMap<_, _> = sheet
            .styles
            .iter()
            .map(|s| ((s.row, s.column), s))
            .collect();
        let positions: BTreeSet<_> = cells.keys().chain(styles.keys()).copied().collect();
        // 128 targets also bound aggregate AST memory and encoded event size.
        let mut chunks: Vec<Vec<(u32, u32)>> = Vec::new();
        let mut chunk = Vec::new();
        let mut text_bytes = 0;
        for position in positions {
            let bytes = match cells.get(&position) {
                Some(Content::Text(s) | Content::Formula(s)) => s.len(),
                _ => 0,
            };
            if !chunk.is_empty() && (chunk.len() == 128 || text_bytes + bytes > 256 * 1024) {
                chunks.push(std::mem::take(&mut chunk));
                text_bytes = 0;
            }
            chunk.push(position);
            text_bytes += bytes;
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        for chunk in &chunks {
            let mut content_writes = Vec::new();
            let mut style_writes = Vec::new();
            for (row, column) in chunk {
                let addr = address(seed, *row, *column)?;
                let content = match cells.get(&(*row, *column)) {
                    None | Some(Content::Blank) => None,
                    Some(Content::Number(v)) => Some(CellContent::Value(CellValue::Number(*v))),
                    Some(Content::Text(v)) => Some(CellContent::Value(CellValue::Text(v.clone()))),
                    Some(Content::Boolean(v)) => Some(CellContent::Value(CellValue::Boolean(*v))),
                    Some(Content::Formula(source)) => Some(CellContent::Formula(
                        binder::bind(source, &current, addr).map_err(invalid)?,
                    )),
                };
                content_writes.push(GuardedContentWrite {
                    address: addr,
                    content,
                    previous: None,
                });
                if let Some(style) = styles.get(&(*row, *column)) {
                    let view: CellStyleView = serde_json::from_value(serde_json::Value::Object(
                        style.properties.clone().into_iter().collect(),
                    ))
                    .map_err(invalid)?;
                    let style = CellStyle::try_from(view).map_err(invalid)?;
                    let changes = style.normalized_changes();
                    if !changes.is_empty() {
                        style_writes.push(GuardedStyleWrite {
                            address: addr,
                            previous: changes
                                .iter()
                                .map(|c| CellStyle::default().change(c.property()))
                                .collect(),
                            changes,
                        });
                    }
                }
            }
            content_writes.sort_by_key(|w| w.address);
            style_writes.sort_by_key(|w| w.address);
            append(
                &mut current,
                &mut events,
                actor,
                &mut ids,
                Operation::CellBatchWrite {
                    intent: CellBatchIntent::SetValues,
                    content_writes,
                    style_writes,
                    undoes: None,
                    undoes_version: None,
                },
            )?;
        }
        let mut sizes = Vec::new();
        let mut seen = BTreeSet::new();
        for (row, values) in [(true, &sheet.row_heights), (false, &sheet.column_widths)] {
            for size in values {
                if !seen.insert((row, size.position)) {
                    return Err(FileError::Invalid);
                }
                let target = if row {
                    AxisSizeTarget::Row(
                        *seed
                            .rows
                            .get(size.position as usize)
                            .ok_or(FileError::Invalid)?,
                    )
                } else {
                    AxisSizeTarget::Column(
                        *seed
                            .columns
                            .get(size.position as usize)
                            .ok_or(FileError::Invalid)?,
                    )
                };
                sizes.push(AxisSizeChange {
                    target,
                    size: Some(size.size_css_px),
                    previous: None,
                });
            }
        }
        for chunk in sizes.chunks(1000) {
            append(
                &mut current,
                &mut events,
                actor,
                &mut ids,
                Operation::AxisSizeWrite {
                    sheet: seed.id,
                    writes: chunk.to_vec(),
                },
            )?;
        }
        if sheet.frozen_rows != 0 {
            append(
                &mut current,
                &mut events,
                actor,
                &mut ids,
                Operation::FrozenRowsWrite {
                    sheet: seed.id,
                    rows: sheet.frozen_rows,
                    previous: 0,
                },
            )?;
        }
        for merge in &sheet.merges {
            let first = address(seed, merge.first_row, merge.first_column)?;
            let last = address(seed, merge.last_row, merge.last_column)?;
            append(
                &mut current,
                &mut events,
                actor,
                &mut ids,
                Operation::MergeWrite {
                    sheet: seed.id,
                    change: spreadsheet_domain::MergeChange {
                        range: spreadsheet_domain::MergeRange::new(
                            first.row,
                            last.row,
                            first.column,
                            last.column,
                        ),
                        merged: true,
                        previous: false,
                    },
                    undoes: None,
                    undoes_version: None,
                },
            )?;
        }
    }
    let mut personal = PersonalFilters::default();
    let mut locations = BTreeSet::new();
    for loc in &input.settings.hidden_hardcode_warnings {
        let seed = seeds.get(loc.sheet as usize).ok_or(FileError::Invalid)?;
        address(seed, loc.row, loc.column)?;
        if !locations.insert((loc.sheet, loc.row, loc.column)) {
            return Err(FileError::Invalid);
        }
    }
    let mut sensitive_locations = BTreeSet::new();
    for value in &input.settings.ignored_sensitive_cells {
        let loc = &value.location;
        let seed = seeds.get(loc.sheet as usize).ok_or(FileError::Invalid)?;
        address(seed, loc.row, loc.column)?;
        let rules = PrivacyRules {
            enabled: false,
            types: vec![value.kind.clone()],
            exceptions: vec![],
        };
        rules.kinds().map_err(invalid)?;
        if !sensitive_locations.insert((loc.sheet, loc.row, loc.column, value.kind.clone())) {
            return Err(FileError::Invalid);
        }
    }
    for filter in &input.settings.filters {
        let seed = seeds.get(filter.sheet as usize).ok_or(FileError::Invalid)?;
        let first = address(seed, filter.first_row, filter.first_column)?;
        let last = address(seed, filter.last_row, filter.last_column)?;
        personal.filters.push(PersonalFilter {
            id: Uuid::from_bytes(ids.id()),
            sheet_id: Uuid::from_bytes(*seed.id.as_bytes()),
            first_row_id: Uuid::from_bytes(*first.row.as_bytes()),
            last_row_id: Uuid::from_bytes(*last.row.as_bytes()),
            first_column_id: Uuid::from_bytes(*first.column.as_bytes()),
            last_column_id: Uuid::from_bytes(*last.column.as_bytes()),
            header_row_id: filter
                .header_row
                .map(|r| {
                    address(seed, r, filter.first_column)
                        .map(|a| Uuid::from_bytes(*a.row.as_bytes()))
                })
                .transpose()?,
            conditions: filter
                .conditions
                .iter()
                .map(|c| {
                    Ok(ColumnFilter {
                        column_id: Uuid::from_bytes(
                            *address(seed, filter.first_row, c.column)?.column.as_bytes(),
                        ),
                        predicate: c.predicate.clone(),
                    })
                })
                .collect::<Result<_, FileError>>()?,
        });
    }
    for (loc, hash) in hardcodes(&current)? {
        if input
            .settings
            .hidden_hardcode_warnings
            .iter()
            .any(|l| same(&loc, l))
        {
            personal.hardcode_dismissals.push(hash)
        }
    }
    personal.validate_geometry(&current).map_err(invalid)?;
    let mut privacy = PrivacyRules {
        enabled: input.settings.recognition_enabled,
        types: input.settings.recognition_types.clone(),
        exceptions: vec![],
    };
    privacy.kinds().map_err(invalid)?;
    if !input.settings.ignored_sensitive_cells.is_empty() {
        let scan = PrivacyRules {
            enabled: true,
            ..privacy.clone()
        };
        for finding in crate::privacy_api::analyze(&current, &scan)
            .map_err(invalid)?
            .findings
        {
            let index = seeds
                .iter()
                .position(|s| Uuid::from_bytes(*s.id.as_bytes()).to_string() == finding.sheet_id)
                .ok_or(FileError::Invalid)?;
            let loc = Location {
                sheet: index as u32,
                row: finding.row,
                column: finding.column,
            };
            if input
                .settings
                .ignored_sensitive_cells
                .iter()
                .any(|v| same(&loc, &v.location) && v.kind == finding.kind)
                && !privacy.exceptions.contains(&finding.fingerprint)
            {
                privacy.exceptions.push(finding.fingerprint)
            }
        }
    }
    privacy.kinds().map_err(invalid)?;
    events.iter().try_fold(0usize, |sum, event| {
        sum.checked_add(spreadsheet_storage::validate_import_event_size(event).map_err(invalid)?)
            .filter(|n| *n <= 96 * 1024 * 1024)
            .ok_or(FileError::Limit)
    })?;
    Ok(Prepared {
        events,
        personal,
        privacy,
        book: current,
    })
}

pub fn export(
    book: &Workbook,
    personal: &PersonalFilters,
    rules: &PrivacyRules,
    anonymous: bool,
    title: String,
) -> Result<Book, FileError> {
    let analysis = if anonymous {
        Some(crate::privacy_api::analyze(book, rules).map_err(invalid)?)
    } else {
        None
    };
    let mask = |text: &str| -> Result<String, FileError> {
        if anonymous {
            crate::privacy_api::text_mask(text, rules).map_err(invalid)
        } else {
            Ok(text.into())
        }
    };
    let mut sheets = Vec::new();
    for sheet in book.sheets() {
        let mut cells = Vec::new();
        for (addr, content) in sheet.cells() {
            let source = if let Some(scan) = &analysis {
                match scan.values.get(addr).ok_or(FileError::Invalid)? {
                    CellValue::Blank => Content::Blank,
                    CellValue::Number(n) => Content::Number(*n),
                    CellValue::Text(t) => Content::Text(t.clone()),
                    CellValue::Boolean(b) => Content::Boolean(*b),
                }
            } else {
                match content {
                    CellContent::Value(CellValue::Blank) => Content::Blank,
                    CellContent::Value(CellValue::Number(n)) => Content::Number(*n),
                    CellContent::Value(CellValue::Text(t)) => Content::Text(t.clone()),
                    CellContent::Value(CellValue::Boolean(b)) => Content::Boolean(*b),
                    CellContent::Formula(f) => Content::Formula(
                        spreadsheet_formula::bound::render_current(f, book).map_err(invalid)?,
                    ),
                }
            };
            cells.push(Cell {
                row: sheet.row_position(addr.row).ok_or(FileError::Invalid)?,
                column: sheet
                    .column_position(addr.column)
                    .ok_or(FileError::Invalid)?,
                content: source,
            });
        }
        let mut styles = Vec::new();
        for (addr, style) in sheet.styles() {
            let serde_json::Value::Object(mut properties) =
                serde_json::to_value(CellStyleView::from(style)).map_err(invalid)?
            else {
                return Err(FileError::Invalid);
            };
            if anonymous
                && let Some(serde_json::Value::String(family)) = properties.get_mut("font_family")
            {
                *family = mask(family)?;
            }
            styles.push(Style {
                row: sheet.row_position(addr.row).ok_or(FileError::Invalid)?,
                column: sheet
                    .column_position(addr.column)
                    .ok_or(FileError::Invalid)?,
                properties: properties.into_iter().collect(),
            });
        }
        cells.sort_by_key(|cell| (cell.row, cell.column));
        styles.sort_by_key(|style| (style.row, style.column));
        let mut exported_sheet = Sheet {
            name: mask(sheet.name())?,
            rows: sheet.rows().len() as u32,
            columns: sheet.columns().len() as u32,
            frozen_rows: sheet.frozen_rows(),
            cells,
            styles,
            row_heights: sheet
                .row_heights()
                .iter()
                .map(|(id, size)| {
                    Ok(Size {
                        position: sheet.row_position(*id).ok_or(FileError::Invalid)?,
                        size_css_px: *size,
                    })
                })
                .collect::<Result<_, FileError>>()?,
            column_widths: sheet
                .column_widths()
                .iter()
                .map(|(id, size)| {
                    Ok(Size {
                        position: sheet.column_position(*id).ok_or(FileError::Invalid)?,
                        size_css_px: *size,
                    })
                })
                .collect::<Result<_, FileError>>()?,
            merges: sheet
                .merges()
                .iter()
                .map(|m| {
                    Ok(Merge {
                        first_row: sheet
                            .row_position(m.first_row())
                            .ok_or(FileError::Invalid)?,
                        last_row: sheet.row_position(m.last_row()).ok_or(FileError::Invalid)?,
                        first_column: sheet
                            .column_position(m.first_column())
                            .ok_or(FileError::Invalid)?,
                        last_column: sheet
                            .column_position(m.last_column())
                            .ok_or(FileError::Invalid)?,
                    })
                })
                .collect::<Result<_, FileError>>()?,
        };
        exported_sheet.row_heights.sort_by_key(|size| size.position);
        exported_sheet
            .column_widths
            .sort_by_key(|size| size.position);
        exported_sheet.merges.sort_by_key(|merge| {
            (
                merge.first_row,
                merge.first_column,
                merge.last_row,
                merge.last_column,
            )
        });
        sheets.push(exported_sheet);
    }
    let mut settings = Settings {
        recognition_enabled: rules.enabled,
        recognition_types: rules.types.clone(),
        ..Settings::default()
    };
    if anonymous {
        settings.omitted_stale_filters = personal.filters.len() as u32;
    } else {
        for f in &personal.filters {
            let index = book
                .sheets()
                .iter()
                .position(|s| Uuid::from_bytes(*s.id().as_bytes()) == f.sheet_id);
            let converted = (|| {
                let index = index?;
                let sheet = &book.sheets()[index];
                Some(Filter {
                    sheet: index as u32,
                    first_row: sheet
                        .row_position(RowId::from_bytes(*f.first_row_id.as_bytes()).ok()?)?,
                    last_row: sheet
                        .row_position(RowId::from_bytes(*f.last_row_id.as_bytes()).ok()?)?,
                    first_column: sheet.column_position(
                        ColumnId::from_bytes(*f.first_column_id.as_bytes()).ok()?,
                    )?,
                    last_column: sheet.column_position(
                        ColumnId::from_bytes(*f.last_column_id.as_bytes()).ok()?,
                    )?,
                    header_row: match f.header_row_id {
                        Some(id) => {
                            Some(sheet.row_position(RowId::from_bytes(*id.as_bytes()).ok()?)?)
                        }
                        None => None,
                    },
                    conditions: f
                        .conditions
                        .iter()
                        .map(|c| {
                            Some(Condition {
                                column: sheet.column_position(
                                    ColumnId::from_bytes(*c.column_id.as_bytes()).ok()?,
                                )?,
                                predicate: c.predicate.clone(),
                            })
                        })
                        .collect::<Option<_>>()?,
                })
            })();
            if let Some(f) = converted {
                settings.filters.push(f)
            } else {
                settings.omitted_stale_filters += 1
            }
        }
        settings.hidden_hardcode_warnings = hardcodes(book)?
            .into_iter()
            .filter(|(_, hash)| personal.hardcode_dismissals.contains(hash))
            .map(|(loc, _)| loc)
            .collect();
        if !rules.exceptions.is_empty() {
            let scan = PrivacyRules {
                enabled: true,
                exceptions: vec![],
                ..rules.clone()
            };
            for finding in crate::privacy_api::analyze(book, &scan)
                .map_err(invalid)?
                .findings
            {
                if rules.exceptions.contains(&finding.fingerprint) {
                    let index = book
                        .sheets()
                        .iter()
                        .position(|s| {
                            Uuid::from_bytes(*s.id().as_bytes()).to_string() == finding.sheet_id
                        })
                        .ok_or(FileError::Invalid)?;
                    settings.ignored_sensitive_cells.push(SensitiveLocation {
                        location: Location {
                            sheet: index as u32,
                            row: finding.row,
                            column: finding.column,
                        },
                        kind: finding.kind.into(),
                    });
                }
            }
        }
    }
    Ok(Book {
        schema_version: 1,
        engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
        compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
        title: mask(&title)?,
        anonymized: anonymous,
        sheets,
        settings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Book {
        Book {
            schema_version: 1,
            engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
            compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
            title: "Book".into(),
            anonymized: false,
            sheets: vec![Sheet {
                name: "Data".into(),
                rows: 100,
                columns: 5,
                frozen_rows: 1,
                cells: vec![
                    Cell {
                        row: 1,
                        column: 0,
                        content: Content::Number(7.0),
                    },
                    Cell {
                        row: 1,
                        column: 1,
                        content: Content::Formula("=A2*$A$2".into()),
                    },
                ],
                styles: vec![],
                row_heights: vec![Size {
                    position: 2,
                    size_css_px: 41,
                }],
                column_widths: vec![Size {
                    position: 1,
                    size_css_px: 151,
                }],
                merges: vec![Merge {
                    first_row: 4,
                    last_row: 5,
                    first_column: 0,
                    last_column: 1,
                }],
            }],
            settings: Settings::default(),
        }
    }
    #[test]
    fn complete_round_trip_rebinds_formulas_and_preserves_layout_and_personal_view()
    -> Result<(), Box<dyn std::error::Error>> {
        let doc = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let request = Uuid::now_v7();
        let mut input = fixture();
        let style = CellStyle::default().apply(&[
            spreadsheet_domain::style::StyleChange::FontBold(
                spreadsheet_domain::style::Change::Set(true),
            ),
            spreadsheet_domain::style::StyleChange::TextOverflow(
                spreadsheet_domain::style::Change::Set(
                    spreadsheet_domain::style::TextOverflow::Overflow,
                ),
            ),
        ])?;
        let serde_json::Value::Object(properties) =
            serde_json::to_value(CellStyleView::from(&style))?
        else {
            return Err("style".into());
        };
        input.sheets[0].styles.push(Style {
            row: 1,
            column: 1,
            properties: properties.into_iter().collect(),
        });
        input.settings.filters.push(Filter {
            sheet: 0,
            first_row: 0,
            last_row: 3,
            first_column: 0,
            last_column: 1,
            header_row: Some(0),
            conditions: vec![Condition {
                column: 0,
                predicate: spreadsheet_storage::FilterPredicate::Number {
                    operator: spreadsheet_storage::NumberFilterOperator::Gt,
                    value: 5.0,
                },
            }],
        });
        let first = prepare(&input, doc, actor, request, b"digest")?;
        let retry = prepare(&input, doc, actor, request, b"digest")?;
        assert_eq!(first.events, retry.events);
        let encoded = export(
            &first.book,
            &first.personal,
            &first.privacy,
            false,
            input.title.clone(),
        )?;
        let second = prepare(
            &encoded,
            DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?,
            actor,
            Uuid::now_v7(),
            b"other",
        )?;
        assert_ne!(first.book.sheets()[0].id(), second.book.sheets()[0].id());
        assert_ne!(
            first.book.sheets()[0].rows(),
            second.book.sheets()[0].rows()
        );
        assert_eq!(
            serde_json::to_value(&encoded)?,
            serde_json::to_value(export(
                &second.book,
                &second.personal,
                &second.privacy,
                false,
                input.title
            )?)?
        );
        let limits = spreadsheet_formula::calculation::CalculationLimits::default();
        let mut calc = spreadsheet_formula::calculation::CalculationProjection::from_workbook(
            &second.book,
            limits,
        )?;
        calc.calculate(
            &second.book,
            limits,
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        assert!(
            calc.results()
                .values()
                .any(|v| *v == spreadsheet_formula::Value::Number(49.0))
        );
        assert_eq!(second.book.sheets()[0].merges().len(), 1);
        assert_eq!(second.book.sheets()[0].frozen_rows(), 1);
        assert_eq!(second.personal.filters.len(), 1);
        Ok(())
    }
    #[test]
    fn rejects_duplicates_bounds_unknown_versions_and_forged_styles()
    -> Result<(), Box<dyn std::error::Error>> {
        let doc = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let run = |b: &Book| prepare(b, doc, actor, Uuid::now_v7(), b"x");
        let original = fixture();
        let mut b = original.clone();
        let duplicate = b.sheets[0].cells[0].clone();
        b.sheets[0].cells.push(duplicate);
        assert!(run(&b).is_err());
        b = original.clone();
        b.sheets[0].cells[0].row = 100;
        assert!(run(&b).is_err());
        b = original.clone();
        b.schema_version = 2;
        assert!(matches!(run(&b), Err(FileError::Version)));
        b = original.clone();
        b.sheets[0].styles.push(Style {
            row: 0,
            column: 0,
            properties: BTreeMap::from([(
                "secret_unknown_property".into(),
                serde_json::json!(true),
            )]),
        });
        assert!(run(&b).is_err());
        b = original.clone();
        b.settings.hidden_hardcode_warnings.push(Location {
            sheet: 9,
            row: 0,
            column: 0,
        });
        assert!(run(&b).is_err());
        b = original.clone();
        b.anonymized = true;
        assert!(run(&b).is_err());
        Ok(())
    }
    #[test]
    fn anonymous_file_has_only_safe_values_and_no_sensitive_filter_conditions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut input = fixture();
        input.sheets[0].cells[0].content = Content::Text("private@example.test".into());
        input.sheets[0].cells[1].content = Content::Formula("=LEN(A2)".into());
        input.sheets[0].name = "private@example.test".into();
        let doc = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let prepared = prepare(&input, doc, actor, Uuid::now_v7(), b"x")?;
        let privacy = PrivacyRules {
            enabled: true,
            types: vec!["email".into()],
            exceptions: vec![],
        };
        let file = export(
            &prepared.book,
            &prepared.personal,
            &privacy,
            true,
            "private@example.test".into(),
        )?;
        let plain = serde_json::to_string(&file)?;
        assert!(!plain.contains("private@example.test"));
        assert!(!plain.contains("LEN("));
        assert!(plain.contains("ЗАВИСИМОСТЬ"));
        assert!(file.anonymized);
        assert!(
            export(
                &prepared.book,
                &prepared.personal,
                &PrivacyRules::default(),
                true,
                "Book".into()
            )
            .is_err()
        );
        Ok(())
    }
}
