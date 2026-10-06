//! Parent-side conversion from validated XLSX IR into a durable import journal.
//! The local CLI is feature-gated until the untrusted worker has an OS sandbox.

use spreadsheet_domain::{
    formula::{FormulaSource, Node},
    style::{
        Border, BorderLine, Change, FontFamily, FontSize, HorizontalAlignment, NumberFormat, Rgb,
        StyleChange, TextOverflow, Underline, VerticalAlignment,
    },
    style_write::prepare_style_write,
    workbook::{
        ActorId, CellAddress, CellValue, CellWrite, ColumnId, CommittedEvent, DocumentId,
        DocumentVersion, DomainError, EventPrincipal, MAX_COLUMNS, MAX_ROWS, Operation,
        OperationId, RowId, SheetId, SheetSeed, StyleWriteKind, Workbook,
    },
};
use spreadsheet_formula::binder;
use spreadsheet_xlsx::{NormalizedCellValue, NormalizedStyle, NormalizedWorkbook};
use thiserror::Error;
use uuid::Uuid;

const MAX_IMPORT_CELLS: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LossPolicy {
    Reject,
    AcceptReported,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("invalid XLSX intermediate representation")]
    InvalidIr,
    #[error("XLSX workbook exceeds the supported import profile")]
    UnsupportedProfile,
    #[error("XLSX formula cannot be imported in the supported profile")]
    InvalidFormula,
    #[error("imported workbook violates domain limits")]
    Domain(#[from] DomainError),
}

fn event(
    document: DocumentId,
    actor: ActorId,
    previous: &Workbook,
    operation: Operation,
) -> Result<(CommittedEvent, Workbook), ImportError> {
    let version = DocumentVersion::new(
        previous
            .version()
            .get()
            .checked_add(1)
            .ok_or(DomainError::LimitExceeded)?,
    )?;
    let candidate = previous.candidate(&operation, version)?;
    Ok((
        CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
            principal: EventPrincipal::User(actor),
            version,
            operation,
            state_hash: candidate.hash(),
        },
        candidate,
    ))
}

pub fn prepare(
    imported: &NormalizedWorkbook,
    document: DocumentId,
    actor: ActorId,
    loss_policy: LossPolicy,
) -> Result<Vec<CommittedEvent>, ImportError> {
    imported.validate().map_err(|_| ImportError::InvalidIr)?;
    if !imported.issues.is_empty() && loss_policy == LossPolicy::Reject {
        return Err(ImportError::UnsupportedProfile);
    }
    if imported.sheets.is_empty()
        || imported.sheets.len() > 64
        || imported
            .sheets
            .iter()
            .map(|sheet| {
                sheet
                    .cells
                    .iter()
                    .map(|cell| (cell.row, cell.column))
                    .chain(sheet.styles.iter().map(|style| (style.row, style.column)))
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            })
            .sum::<usize>()
            > MAX_IMPORT_CELLS
    {
        return Err(ImportError::UnsupportedProfile);
    }
    let seeds = imported
        .sheets
        .iter()
        .map(|sheet| {
            let row_extent = sheet
                .cells
                .iter()
                .map(|cell| cell.row as usize)
                .chain(sheet.styles.iter().map(|style| style.row as usize))
                .chain(sheet.merges.iter().map(|merge| merge.last_row as usize))
                .max()
                .unwrap_or(0)
                .max(1000);
            let column_extent = sheet
                .cells
                .iter()
                .map(|cell| cell.column as usize)
                .chain(sheet.styles.iter().map(|style| style.column as usize))
                .chain(sheet.merges.iter().map(|merge| merge.last_column as usize))
                .max()
                .unwrap_or(0)
                .max(26);
            if row_extent > MAX_ROWS || column_extent > MAX_COLUMNS {
                return Err(ImportError::UnsupportedProfile);
            }
            Ok(SheetSeed {
                id: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
                name: sheet.name.clone(),
                rows: (0..row_extent)
                    .map(|_| RowId::from_bytes(*Uuid::now_v7().as_bytes()))
                    .collect::<Result<_, _>>()?,
                columns: (0..column_extent)
                    .map(|_| ColumnId::from_bytes(*Uuid::now_v7().as_bytes()))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    let mut events = Vec::with_capacity(
        MAX_IMPORT_CELLS
            .saturating_add(seeds.len())
            .saturating_add(1),
    );
    let (created, mut current) = event(
        document,
        actor,
        &Workbook::empty(document),
        Operation::CreateWorkbook {
            sheet: seeds[0].clone(),
        },
    )?;
    events.push(created);
    for seed in seeds.iter().skip(1) {
        let (added, next) = event(
            document,
            actor,
            &current,
            Operation::AddSheet {
                sheet: seed.clone(),
            },
        )?;
        events.push(added);
        current = next;
    }
    let mut values = Vec::new();
    let mut formulas = Vec::new();
    for (sheet, seed) in imported.sheets.iter().zip(&seeds) {
        for cell in &sheet.cells {
            let address = CellAddress {
                sheet: seed.id,
                row: *seed
                    .rows
                    .get((cell.row - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
                column: *seed
                    .columns
                    .get((cell.column - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
            };
            match &cell.value {
                NormalizedCellValue::Number(value) => values.push(CellWrite {
                    address,
                    value: CellValue::Number(*value),
                }),
                NormalizedCellValue::Text(value) => values.push(CellWrite {
                    address,
                    value: CellValue::Text(value.clone()),
                }),
                NormalizedCellValue::Boolean(value) => values.push(CellWrite {
                    address,
                    value: CellValue::Boolean(*value),
                }),
                NormalizedCellValue::Formula(raw) => {
                    formulas.push((address, format!("={raw}")));
                }
                NormalizedCellValue::FormulaCopy {
                    master_row,
                    master_column,
                } => {
                    let master = sheet
                        .cells
                        .iter()
                        .find(|candidate| {
                            candidate.row == *master_row && candidate.column == *master_column
                        })
                        .and_then(|candidate| match &candidate.value {
                            NormalizedCellValue::Formula(source) => Some(source),
                            _ => None,
                        })
                        .ok_or(ImportError::InvalidIr)?;
                    let copied = spreadsheet_formula::copy_formula_text(
                        &format!("={master}"),
                        (*master_row - 1, *master_column - 1),
                        (cell.row - 1, cell.column - 1),
                    )
                    .map_err(|_| ImportError::InvalidFormula)?;
                    formulas.push((address, copied));
                }
            }
        }
    }
    if !values.is_empty() {
        let (saved, next) = event(
            document,
            actor,
            &current,
            Operation::SetCellValues { writes: values },
        )?;
        events.push(saved);
        current = next;
    }
    for (address, source_text) in formulas {
        let source = FormulaSource::new(source_text).map_err(|_| ImportError::InvalidFormula)?;
        let formula = binder::bind(source.as_str(), &current, address)
            .map_err(|_| ImportError::InvalidFormula)?;
        if formula
            .ast()
            .nodes()
            .iter()
            .any(|node| matches!(node, Node::InvalidReference(_)))
        {
            return Err(ImportError::InvalidFormula);
        }
        let (saved, next) = event(
            document,
            actor,
            &current,
            Operation::SetCellFormula { address, formula },
        )?;
        events.push(saved);
        current = next;
    }
    for (sheet, seed) in imported.sheets.iter().zip(&seeds) {
        for style in &sheet.styles {
            let address = CellAddress {
                sheet: seed.id,
                row: *seed
                    .rows
                    .get((style.row - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
                column: *seed
                    .columns
                    .get((style.column - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
            };
            let changes = style_changes(style)?;
            let operation =
                prepare_style_write(&current, &[address], &changes, StyleWriteKind::Cell)?;
            let (saved, next) = event(document, actor, &current, operation)?;
            events.push(saved);
            current = next;
        }
    }
    for (sheet, seed) in imported.sheets.iter().zip(&seeds) {
        for merge in &sheet.merges {
            let range = spreadsheet_domain::MergeRange::new(
                *seed
                    .rows
                    .get((merge.first_row - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
                *seed
                    .rows
                    .get((merge.last_row - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
                *seed
                    .columns
                    .get((merge.first_column - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
                *seed
                    .columns
                    .get((merge.last_column - 1) as usize)
                    .ok_or(ImportError::UnsupportedProfile)?,
            );
            let operation = Operation::MergeWrite {
                sheet: seed.id,
                change: spreadsheet_domain::MergeChange {
                    range,
                    merged: true,
                    previous: false,
                },
                undoes: None,
                undoes_version: None,
            };
            let (saved, next) = event(document, actor, &current, operation)?;
            events.push(saved);
            current = next;
        }
    }
    Ok(events)
}

fn style_changes(style: &NormalizedStyle) -> Result<Vec<StyleChange>, ImportError> {
    let mut changes = Vec::with_capacity(16);
    if let Some(value) = &style.font_family {
        changes.push(StyleChange::FontFamily(Change::Set(
            FontFamily::new(value.clone()).map_err(|_| ImportError::InvalidIr)?,
        )));
    }
    if let Some(value) = style.font_size_hundredth_points {
        changes.push(StyleChange::FontSize(Change::Set(
            FontSize::new(value).map_err(|_| ImportError::InvalidIr)?,
        )));
    }
    if let Some(value) = style.bold {
        changes.push(StyleChange::FontBold(Change::Set(value)));
    }
    if let Some(value) = style.italic {
        changes.push(StyleChange::FontItalic(Change::Set(value)));
    }
    if let Some(value) = style.underline {
        changes.push(StyleChange::FontUnderline(Change::Set(match value {
            spreadsheet_xlsx::NormalizedUnderline::Single => Underline::Single,
            spreadsheet_xlsx::NormalizedUnderline::Double => Underline::Double,
        })));
    }
    if let Some(value) = style.strike {
        changes.push(StyleChange::FontStrike(Change::Set(value)));
    }
    if let Some(value) = style.font_color {
        changes.push(StyleChange::FontColor(Change::Set(Rgb::new(value))));
    }
    if let Some(value) = style.fill {
        changes.push(StyleChange::Fill(Change::Set(Rgb::new(value))));
    }
    for (border, property) in [
        (&style.border_top, 0_u8),
        (&style.border_right, 1),
        (&style.border_bottom, 2),
        (&style.border_left, 3),
    ] {
        if let Some(border) = border {
            let value = Border::new(border_line(border.line), Rgb::new(border.color));
            changes.push(match property {
                0 => StyleChange::BorderTop(Change::Set(value)),
                1 => StyleChange::BorderRight(Change::Set(value)),
                2 => StyleChange::BorderBottom(Change::Set(value)),
                _ => StyleChange::BorderLeft(Change::Set(value)),
            });
        }
    }
    if let Some(value) = style.horizontal {
        changes.push(StyleChange::Horizontal(Change::Set(match value {
            spreadsheet_xlsx::NormalizedHorizontal::General => HorizontalAlignment::General,
            spreadsheet_xlsx::NormalizedHorizontal::Left => HorizontalAlignment::Left,
            spreadsheet_xlsx::NormalizedHorizontal::Center => HorizontalAlignment::Center,
            spreadsheet_xlsx::NormalizedHorizontal::Right => HorizontalAlignment::Right,
        })));
    }
    if let Some(value) = style.vertical {
        changes.push(StyleChange::Vertical(Change::Set(match value {
            spreadsheet_xlsx::NormalizedVertical::Top => VerticalAlignment::Top,
            spreadsheet_xlsx::NormalizedVertical::Center => VerticalAlignment::Center,
            spreadsheet_xlsx::NormalizedVertical::Bottom => VerticalAlignment::Bottom,
        })));
    }
    if let Some(value) = style.wrap {
        changes.push(StyleChange::TextOverflow(Change::Set(if value {
            TextOverflow::Wrap
        } else {
            TextOverflow::Clip
        })));
    }
    if let Some(value) = style.number_format {
        changes.push(StyleChange::NumberFormat(Change::Set(match value {
            spreadsheet_xlsx::NormalizedNumberFormat::General => NumberFormat::General,
            spreadsheet_xlsx::NormalizedNumberFormat::Integer => NumberFormat::Integer,
            spreadsheet_xlsx::NormalizedNumberFormat::Decimal2 => NumberFormat::Decimal2,
            spreadsheet_xlsx::NormalizedNumberFormat::Percent0 => NumberFormat::Percent0,
            spreadsheet_xlsx::NormalizedNumberFormat::Percent2 => NumberFormat::Percent2,
            spreadsheet_xlsx::NormalizedNumberFormat::Scientific2 => NumberFormat::Scientific2,
            spreadsheet_xlsx::NormalizedNumberFormat::DateIso => NumberFormat::DateIso,
        })));
    }
    if changes.is_empty() {
        return Err(ImportError::InvalidIr);
    }
    Ok(changes)
}

fn border_line(line: spreadsheet_xlsx::NormalizedBorderLine) -> BorderLine {
    match line {
        spreadsheet_xlsx::NormalizedBorderLine::Thin => BorderLine::Thin,
        spreadsheet_xlsx::NormalizedBorderLine::Medium => BorderLine::Medium,
        spreadsheet_xlsx::NormalizedBorderLine::Thick => BorderLine::Thick,
        spreadsheet_xlsx::NormalizedBorderLine::Dashed => BorderLine::Dashed,
        spreadsheet_xlsx::NormalizedBorderLine::Dotted => BorderLine::Dotted,
        spreadsheet_xlsx::NormalizedBorderLine::Double => BorderLine::Double,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_xlsx::{NormalizedCell, NormalizedSheet};

    #[test]
    fn normalized_values_and_formula_become_replayable_events()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                styles: vec![NormalizedStyle {
                    row: 3,
                    column: 2,
                    font_family: None,
                    font_size_hundredth_points: None,
                    bold: Some(true),
                    italic: None,
                    underline: None,
                    strike: None,
                    font_color: None,
                    fill: None,
                    border_top: None,
                    border_right: None,
                    border_bottom: None,
                    border_left: None,
                    horizontal: None,
                    vertical: None,
                    wrap: None,
                    number_format: None,
                }],
                merges: vec![],
                cells: vec![
                    NormalizedCell {
                        row: 1,
                        column: 1,
                        value: NormalizedCellValue::Number(7.0),
                    },
                    NormalizedCell {
                        row: 1,
                        column: 2,
                        value: NormalizedCellValue::Formula("A1*2".into()),
                    },
                ],
            }],
        };
        let document = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([2; 16])?;
        let events = prepare(&input, document, actor, LossPolicy::Reject)?;
        assert_eq!(events.len(), 4);
        let mut replay = Workbook::empty(document);
        for event in &events {
            replay.apply_event(event)?;
        }
        let sheet = replay.sheets().first().ok_or("missing sheet")?;
        let a1 = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[0],
            column: sheet.columns()[0],
        };
        let b1 = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[0],
            column: sheet.columns()[1],
        };
        assert_eq!(
            replay.cell_content(a1)?,
            Some(&spreadsheet_domain::workbook::CellContent::Value(
                CellValue::Number(7.0)
            ))
        );
        assert!(matches!(
            replay.cell_content(b1)?,
            Some(spreadsheet_domain::workbook::CellContent::Formula(_))
        ));
        let imported_sheet = replay.sheets().first().ok_or("missing sheet")?;
        assert_eq!(imported_sheet.styles().len(), 1);
        Ok(())
    }

    #[test]
    fn shared_formula_copy_is_expanded_before_server_binding()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                cells: vec![
                    NormalizedCell {
                        row: 1,
                        column: 2,
                        value: NormalizedCellValue::Formula("A1*2+B$1+$C1+$D$1".into()),
                    },
                    NormalizedCell {
                        row: 2,
                        column: 2,
                        value: NormalizedCellValue::FormulaCopy {
                            master_row: 1,
                            master_column: 2,
                        },
                    },
                ],
                styles: vec![],
                merges: vec![],
            }],
        };
        let document = DocumentId::from_bytes([5; 16])?;
        let actor = ActorId::from_bytes([6; 16])?;
        let events = prepare(&input, document, actor, LossPolicy::Reject)?;
        let mut replay = Workbook::empty(document);
        for event in &events {
            replay.apply_event(event)?;
        }
        let sheet = replay.sheets().first().ok_or("missing sheet")?;
        let copied = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[1],
            column: sheet.columns()[1],
        };
        let Some(spreadsheet_domain::workbook::CellContent::Formula(formula)) =
            replay.cell_content(copied)?
        else {
            return Err("copied shared formula was not imported as a formula".into());
        };
        assert_eq!(formula.source(), "=((((A2*2)+B$1)+$C2)+$D$1)");
        Ok(())
    }

    #[test]
    fn unsupported_workbook_fails_before_any_event() -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([2; 16])?;
        let input = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                styles: vec![],
                merges: vec![],
                cells: vec![NormalizedCell {
                    row: 1,
                    column: 1,
                    value: NormalizedCellValue::Formula("'Missing'!A1".into()),
                }],
            }],
        };
        assert!(matches!(
            prepare(&input, document, actor, LossPolicy::Reject),
            Err(ImportError::InvalidFormula)
        ));
        Ok(())
    }

    #[test]
    fn accept_reported_keeps_supported_blank_style_and_reject_discards_before_events()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([3; 16])?;
        let actor = ActorId::from_bytes([4; 16])?;
        let input = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![spreadsheet_xlsx::CompatibilityIssue {
                code: spreadsheet_xlsx::CompatibilityIssueCode::CellFormatting,
                sheet_index: Some(0),
            }],
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                cells: vec![],
                styles: vec![NormalizedStyle {
                    row: 2,
                    column: 2,
                    font_family: None,
                    font_size_hundredth_points: None,
                    bold: Some(true),
                    italic: None,
                    underline: None,
                    strike: None,
                    font_color: None,
                    fill: None,
                    border_top: None,
                    border_right: None,
                    border_bottom: None,
                    border_left: None,
                    horizontal: None,
                    vertical: None,
                    wrap: None,
                    number_format: None,
                }],
                merges: vec![],
            }],
        };
        assert!(matches!(
            prepare(&input, document, actor, LossPolicy::Reject),
            Err(ImportError::UnsupportedProfile)
        ));
        let events = prepare(&input, document, actor, LossPolicy::AcceptReported)?;
        assert_eq!(events.len(), 2);
        let mut replay = Workbook::empty(document);
        for event in events {
            replay.apply_event(&event)?;
        }
        let sheet = replay.sheets().first().ok_or("missing imported sheet")?;
        assert!(sheet.cells().is_empty());
        assert_eq!(sheet.styles().len(), 1);
        assert!(
            sheet
                .styles()
                .values()
                .next()
                .is_some_and(|style| style.font_bold())
        );
        Ok(())
    }

    #[test]
    fn normalized_merge_becomes_replayable_domain_operation()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([41; 16])?;
        let actor = ActorId::from_bytes([42; 16])?;
        let input = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Merged".into(),
                cells: vec![NormalizedCell {
                    row: 1,
                    column: 1,
                    value: NormalizedCellValue::Text("anchor".into()),
                }],
                styles: vec![],
                merges: vec![spreadsheet_xlsx::NormalizedMerge {
                    first_row: 1,
                    last_row: 2,
                    first_column: 1,
                    last_column: 2,
                }],
            }],
        };
        let events = prepare(&input, document, actor, LossPolicy::Reject)?;
        let mut replay = Workbook::empty(document);
        for event in &events {
            replay.apply_event(event)?;
        }
        let sheet = replay.sheets().first().ok_or("missing imported sheet")?;
        assert_eq!(sheet.merges().len(), 1);
        assert_eq!(sheet.cells().len(), 1);
        assert!(matches!(
            events.last().map(|event| &event.operation),
            Some(Operation::MergeWrite { .. })
        ));
        Ok(())
    }
}
