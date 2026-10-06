//! Frozen bounded cell insertion/deletion, including styles and inbound formulas.
use crate::SessionError;
use spreadsheet_domain::{
    formula::Node,
    style::{CellStyle, StyleProperty},
    workbook::{
        CellAddress, CellContent, CellShiftAxis, DomainError, GuardedContentWrite,
        GuardedStyleWrite, Sheet, Workbook,
    },
};
use spreadsheet_formula::relocation::{MoveError, MoveRequest, plan_move};
use std::collections::BTreeMap;

const MAX_WRITES: usize = 1000;
const MAX_VISITED_NODES: usize = 1_000_000;
const PROPERTIES: [StyleProperty; 16] = [
    StyleProperty::FontFamily,
    StyleProperty::FontSize,
    StyleProperty::FontBold,
    StyleProperty::FontItalic,
    StyleProperty::FontUnderline,
    StyleProperty::FontStrike,
    StyleProperty::FontColor,
    StyleProperty::Fill,
    StyleProperty::BorderTop,
    StyleProperty::BorderRight,
    StyleProperty::BorderBottom,
    StyleProperty::BorderLeft,
    StyleProperty::Horizontal,
    StyleProperty::Vertical,
    StyleProperty::TextOverflow,
    StyleProperty::NumberFormat,
];

pub(crate) struct CellShiftPlan {
    pub(crate) content_writes: Vec<GuardedContentWrite>,
    pub(crate) style_writes: Vec<GuardedStyleWrite>,
}

#[derive(Clone, Copy)]
struct Rectangle {
    row: u32,
    column: u32,
    rows: u32,
    columns: u32,
}

impl Rectangle {
    fn contains(self, sheet: &Sheet, address: CellAddress) -> bool {
        address.sheet == sheet.id()
            && sheet
                .row_position(address.row)
                .is_some_and(|row| row >= self.row && row < self.row + self.rows)
            && sheet
                .column_position(address.column)
                .is_some_and(|column| column >= self.column && column < self.column + self.columns)
    }

    fn intersects(self, row: u32, last_row: u32, column: u32, last_column: u32) -> bool {
        row < self.row + self.rows
            && last_row >= self.row
            && column < self.column + self.columns
            && last_column >= self.column
    }
}

fn address_at(sheet: &Sheet, row: u32, column: u32) -> Result<CellAddress, DomainError> {
    Ok(CellAddress {
        sheet: sheet.id(),
        row: *sheet
            .rows()
            .get(row as usize)
            .ok_or(DomainError::MissingRow)?,
        column: *sheet
            .columns()
            .get(column as usize)
            .ok_or(DomainError::MissingColumn)?,
    })
}

/// Reject dependencies on cells whose identity is erased by the shift. The
/// existing move planner follows surviving cells; #REF rewriting for erased
/// cells and partially intersected ranges is intentionally not approximated.
fn validate_erased_references(
    book: &Workbook,
    sheet: &Sheet,
    erased: Rectangle,
) -> Result<(), SessionError> {
    let mut visited = 0usize;
    for other in book.sheets() {
        for (&address, content) in other.cells() {
            if erased.contains(sheet, address) {
                continue;
            }
            let CellContent::Formula(formula) = content else {
                continue;
            };
            visited = visited
                .checked_add(formula.ast().nodes().len())
                .ok_or(DomainError::LimitExceeded)?;
            if visited > MAX_VISITED_NODES {
                return Err(DomainError::LimitExceeded.into());
            }
            for node in formula.ast().nodes() {
                let intersects = match node {
                    Node::Cell(reference) => erased.contains(
                        sheet,
                        CellAddress {
                            sheet: reference.target_sheet,
                            row: reference.target_row,
                            column: reference.target_column,
                        },
                    ),
                    Node::Range(range) if range.start.target_sheet == sheet.id() => {
                        let r1 = sheet
                            .row_position(range.start.target_row)
                            .ok_or(DomainError::MissingRow)?;
                        let r2 = sheet
                            .row_position(range.end.target_row)
                            .ok_or(DomainError::MissingRow)?;
                        let c1 = sheet
                            .column_position(range.start.target_column)
                            .ok_or(DomainError::MissingColumn)?;
                        let c2 = sheet
                            .column_position(range.end.target_column)
                            .ok_or(DomainError::MissingColumn)?;
                        erased.intersects(r1.min(r2), r1.max(r2), c1.min(c2), c1.max(c2))
                    }
                    _ => false,
                };
                if intersects {
                    return Err(SessionError::UnsupportedMoveDependency);
                }
            }
        }
    }
    Ok(())
}

/// Plan the complete allocated suffix of the selection's row/column stripe.
/// Blank cells participate in relocation so references follow even empty cells.
/// All contents and styles are read from one immutable input workbook.
pub(crate) fn plan_shift(
    book: &Workbook,
    source: CellAddress,
    rows: u32,
    columns: u32,
    axis: CellShiftAxis,
    insert: bool,
) -> Result<CellShiftPlan, SessionError> {
    book.cell_content(source)?;
    let sheet = book
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == source.sheet)
        .ok_or(DomainError::MissingSheet)?;
    let row = sheet
        .row_position(source.row)
        .ok_or(DomainError::MissingRow)?;
    let column = sheet
        .column_position(source.column)
        .ok_or(DomainError::MissingColumn)?;
    if rows == 0
        || columns == 0
        || u64::from(row) + u64::from(rows) > sheet.rows().len() as u64
        || u64::from(column) + u64::from(columns) > sheet.columns().len() as u64
    {
        return Err(SessionError::InvalidCommand);
    }
    let stripe = match axis {
        CellShiftAxis::Row => Rectangle {
            row,
            column,
            rows: sheet.rows().len() as u32 - row,
            columns,
        },
        CellShiftAxis::Column => Rectangle {
            row,
            column,
            rows,
            columns: sheet.columns().len() as u32 - column,
        },
    };
    if u64::from(stripe.rows) * u64::from(stripe.columns) > MAX_WRITES as u64 {
        return Err(DomainError::LimitExceeded.into());
    }
    for range in sheet.merges() {
        if stripe.intersects(
            sheet
                .row_position(range.first_row())
                .ok_or(DomainError::MissingRow)?,
            sheet
                .row_position(range.last_row())
                .ok_or(DomainError::MissingRow)?,
            sheet
                .column_position(range.first_column())
                .ok_or(DomainError::MissingColumn)?,
            sheet
                .column_position(range.last_column())
                .ok_or(DomainError::MissingColumn)?,
        ) {
            return Err(DomainError::MergeConflict.into());
        }
    }
    let erased = if !insert {
        Rectangle {
            row,
            column,
            rows,
            columns,
        }
    } else {
        match axis {
            CellShiftAxis::Row => Rectangle {
                row: row + stripe.rows - rows,
                column,
                rows,
                columns,
            },
            CellShiftAxis::Column => Rectangle {
                row,
                column: column + stripe.columns - columns,
                rows,
                columns,
            },
        }
    };
    if insert {
        for r in erased.row..erased.row + erased.rows {
            for c in erased.column..erased.column + erased.columns {
                let address = address_at(sheet, r, c)?;
                if book.cell_content(address)?.is_some() || sheet.styles().contains_key(&address) {
                    return Err(SessionError::InvalidCommand);
                }
            }
        }
    }
    validate_erased_references(book, sheet, erased)?;
    let mut contents = BTreeMap::new();
    let mut styles = BTreeMap::new();
    for r in stripe.row..stripe.row + stripe.rows {
        for c in stripe.column..stripe.column + stripe.columns {
            let address = address_at(sheet, r, c)?;
            contents.insert(
                address,
                GuardedContentWrite {
                    address,
                    content: None,
                    previous: book.cell_content(address)?.cloned(),
                },
            );
            styles.insert(address, CellStyle::default());
        }
    }
    let (from_row, from_column, to_row, to_column, moved_rows, moved_columns) = match (axis, insert)
    {
        (CellShiftAxis::Row, true) => {
            (row, column, row + rows, column, stripe.rows - rows, columns)
        }
        (CellShiftAxis::Row, false) => {
            (row + rows, column, row, column, stripe.rows - rows, columns)
        }
        (CellShiftAxis::Column, true) => (
            row,
            column,
            row,
            column + columns,
            rows,
            stripe.columns - columns,
        ),
        (CellShiftAxis::Column, false) => (
            row,
            column + columns,
            row,
            column,
            rows,
            stripe.columns - columns,
        ),
    };
    if moved_rows > 0 && moved_columns > 0 {
        let plan = plan_move(
            book,
            MoveRequest {
                source: address_at(sheet, from_row, from_column)?,
                destination: address_at(sheet, to_row, to_column)?,
                rows: moved_rows,
                columns: moved_columns,
            },
        )
        .map_err(|error| match error {
            MoveError::PartialRange => SessionError::UnsupportedMoveDependency,
            MoveError::Domain(error) => SessionError::Domain(error),
            MoveError::LimitExceeded => SessionError::Domain(DomainError::LimitExceeded),
            _ => SessionError::InvalidCommand,
        })?;
        for write in plan.writes {
            contents.insert(write.address, write);
        }
        if contents.len() > MAX_WRITES {
            return Err(DomainError::LimitExceeded.into());
        }
        for r in 0..moved_rows {
            for c in 0..moved_columns {
                let from = address_at(sheet, from_row + r, from_column + c)?;
                let to = address_at(sheet, to_row + r, to_column + c)?;
                styles.insert(to, sheet.styles().get(&from).cloned().unwrap_or_default());
            }
        }
    }
    let style_writes = styles
        .into_iter()
        .map(|(address, style)| {
            let previous = sheet.styles().get(&address).cloned().unwrap_or_default();
            GuardedStyleWrite {
                address,
                changes: PROPERTIES
                    .iter()
                    .map(|property| style.change(*property))
                    .collect(),
                previous: PROPERTIES
                    .iter()
                    .map(|property| previous.change(*property))
                    .collect(),
            }
        })
        .collect();
    Ok(CellShiftPlan {
        content_writes: contents.into_values().collect(),
        style_writes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::{
        merge::MergeRange,
        style::{Change, StyleChange},
        workbook::{
            CellBatchIntent, CellValue, ColumnId, DocumentId, DocumentVersion, Operation, RowId,
            SheetId, SheetSeed, StyleWriteKind,
        },
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn seeded(rows: u32, columns: u32) -> TestResult<Workbook> {
        let ids = |count: u32| -> Vec<[u8; 16]> {
            (1..=count)
                .map(|n| {
                    let mut bytes = [1; 16];
                    bytes[..4].copy_from_slice(&n.to_le_bytes());
                    bytes
                })
                .collect()
        };
        Ok(Workbook::empty(DocumentId::from_bytes([3; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: SheetId::from_bytes([4; 16])?,
                    name: "Sheet".into(),
                    rows: ids(rows)
                        .into_iter()
                        .map(RowId::from_bytes)
                        .collect::<Result<_, _>>()?,
                    columns: ids(columns)
                        .into_iter()
                        .map(ColumnId::from_bytes)
                        .collect::<Result<_, _>>()?,
                },
            },
            DocumentVersion::new(1)?,
        )?)
    }

    fn value(book: Workbook, row: u32, column: u32, text: &str) -> TestResult<Workbook> {
        let address = address_at(&book.sheets()[0], row, column)?;
        let version = DocumentVersion::new(book.version().get() + 1)?;
        Ok(book.candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Text(text.into()),
            },
            version,
        )?)
    }

    fn apply(
        book: &Workbook,
        source: CellAddress,
        rows: u32,
        columns: u32,
        axis: CellShiftAxis,
        insert: bool,
    ) -> TestResult<Workbook> {
        let plan = plan_shift(book, source, rows, columns, axis, insert)?;
        Ok(book.candidate(
            &Operation::CellBatchWrite {
                intent: CellBatchIntent::ShiftCells {
                    source,
                    rows,
                    columns,
                    axis,
                    insert,
                },
                content_writes: plan.content_writes,
                style_writes: plan.style_writes,
                undoes: None,
                undoes_version: None,
            },
            DocumentVersion::new(book.version().get() + 1)?,
        )?)
    }

    #[test]
    fn insert_right_moves_sparse_values_styles_and_blanks_simultaneously() -> TestResult {
        let book = value(value(seeded(2, 5)?, 0, 0, "first")?, 0, 2, "third")?;
        let source = address_at(&book.sheets()[0], 0, 0)?;
        let styled = address_at(&book.sheets()[0], 0, 2)?;
        let book = book.candidate(
            &spreadsheet_domain::style_write::prepare_style_write(
                &book,
                &[styled],
                &[StyleChange::FontBold(Change::Set(true))],
                StyleWriteKind::Cell,
            )?,
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let shifted = apply(&book, source, 1, 1, CellShiftAxis::Column, true)?;
        assert_eq!(shifted.cell_content(source)?, None);
        assert_eq!(
            shifted.cell_content(address_at(&book.sheets()[0], 0, 1)?)?,
            book.cell_content(source)?
        );
        assert_eq!(
            shifted.cell_content(address_at(&book.sheets()[0], 0, 2)?)?,
            None
        );
        assert_eq!(
            shifted.cell_content(address_at(&book.sheets()[0], 0, 3)?)?,
            book.cell_content(styled)?
        );
        assert_eq!(shifted.sheets()[0].styles().get(&styled), None);
        assert_eq!(
            shifted.sheets()[0]
                .styles()
                .get(&address_at(&book.sheets()[0], 0, 3)?),
            book.sheets()[0].styles().get(&styled)
        );
        Ok(())
    }

    #[test]
    fn delete_up_shifts_remainder_and_clears_tail() -> TestResult {
        let book = value(value(seeded(5, 2)?, 1, 0, "deleted")?, 3, 0, "moved")?;
        let source = address_at(&book.sheets()[0], 1, 0)?;
        let shifted = apply(&book, source, 2, 1, CellShiftAxis::Row, false)?;
        assert_eq!(
            shifted.cell_content(source)?,
            book.cell_content(address_at(&book.sheets()[0], 3, 0)?)?
        );
        for row in 2..5 {
            assert_eq!(
                shifted.cell_content(address_at(&book.sheets()[0], row, 0)?)?,
                None
            );
        }
        Ok(())
    }

    #[test]
    fn insert_refuses_edge_data_or_style_loss_and_large_footprint() -> TestResult {
        let book = value(seeded(2, 5)?, 0, 4, "edge")?;
        let source = address_at(&book.sheets()[0], 0, 0)?;
        assert!(matches!(
            plan_shift(&book, source, 1, 1, CellShiftAxis::Column, true),
            Err(SessionError::InvalidCommand)
        ));
        let empty = seeded(2, 5)?;
        let edge = address_at(&empty.sheets()[0], 0, 4)?;
        let styled = empty.candidate(
            &spreadsheet_domain::style_write::prepare_style_write(
                &empty,
                &[edge],
                &[StyleChange::FontBold(Change::Set(true))],
                StyleWriteKind::Cell,
            )?,
            DocumentVersion::new(2)?,
        )?;
        assert!(matches!(
            plan_shift(&styled, source, 1, 1, CellShiftAxis::Column, true),
            Err(SessionError::InvalidCommand)
        ));
        let empty = seeded(1001, 1)?;
        assert!(matches!(
            plan_shift(
                &empty,
                address_at(&empty.sheets()[0], 0, 0)?,
                1,
                1,
                CellShiftAxis::Row,
                false
            ),
            Err(SessionError::Domain(DomainError::LimitExceeded))
        ));
        Ok(())
    }

    #[test]
    fn guarded_inverse_restores_all_authored_content_and_styles() -> TestResult {
        let book = value(value(seeded(3, 2)?, 0, 0, "deleted")?, 2, 0, "moved")?;
        let source = address_at(&book.sheets()[0], 0, 0)?;
        let book = book.candidate(
            &spreadsheet_domain::style_write::prepare_style_write(
                &book,
                &[source],
                &[StyleChange::FontItalic(Change::Set(true))],
                StyleWriteKind::Cell,
            )?,
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let plan = plan_shift(&book, source, 1, 1, CellShiftAxis::Row, false)?;
        let inverse_content = plan
            .content_writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect();
        let inverse_styles = plan
            .style_writes
            .iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                changes: write.previous.clone(),
                previous: write.changes.clone(),
            })
            .collect();
        let intent = CellBatchIntent::ShiftCells {
            source,
            rows: 1,
            columns: 1,
            axis: CellShiftAxis::Row,
            insert: false,
        };
        let moved = book.candidate(
            &Operation::CellBatchWrite {
                intent: intent.clone(),
                content_writes: plan.content_writes,
                style_writes: plan.style_writes,
                undoes: None,
                undoes_version: None,
            },
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let restored = moved.candidate(
            &Operation::CellBatchWrite {
                intent,
                content_writes: inverse_content,
                style_writes: inverse_styles,
                undoes: Some(spreadsheet_domain::workbook::OperationId::from_bytes(
                    [8; 16],
                )?),
                undoes_version: Some(moved.version()),
            },
            DocumentVersion::new(moved.version().get() + 1)?,
        )?;
        assert_eq!(restored.sheets()[0].cells(), book.sheets()[0].cells());
        assert_eq!(restored.sheets()[0].styles(), book.sheets()[0].styles());
        Ok(())
    }

    #[test]
    fn durable_candidate_refuses_forged_values_styles_and_incomplete_footprint() -> TestResult {
        let book = value(seeded(2, 4)?, 0, 0, "source")?;
        let source = address_at(&book.sheets()[0], 0, 0)?;
        let operation = |plan: CellShiftPlan| Operation::CellBatchWrite {
            intent: CellBatchIntent::ShiftCells {
                source,
                rows: 1,
                columns: 1,
                axis: CellShiftAxis::Column,
                insert: true,
            },
            content_writes: plan.content_writes,
            style_writes: plan.style_writes,
            undoes: None,
            undoes_version: None,
        };
        let next = DocumentVersion::new(book.version().get() + 1)?;
        let destination = address_at(&book.sheets()[0], 0, 1)?;
        let mut forged = plan_shift(&book, source, 1, 1, CellShiftAxis::Column, true)?;
        forged
            .content_writes
            .iter_mut()
            .find(|write| write.address == destination)
            .ok_or("destination missing")?
            .content = Some(CellContent::Value(CellValue::Text("forged".into())));
        assert!(book.candidate(&operation(forged), next).is_err());
        let mut forged = plan_shift(&book, source, 1, 1, CellShiftAxis::Column, true)?;
        forged
            .style_writes
            .first_mut()
            .ok_or("style footprint missing")?
            .changes[2] = StyleChange::FontBold(Change::Set(true));
        assert!(matches!(
            book.candidate(&operation(forged), next),
            Err(DomainError::InvalidStyle)
        ));
        let mut incomplete = plan_shift(&book, source, 1, 1, CellShiftAxis::Column, true)?;
        incomplete.content_writes.remove(0);
        assert!(matches!(
            book.candidate(&operation(incomplete), next),
            Err(DomainError::InvalidAxis)
        ));
        Ok(())
    }

    #[test]
    fn deletion_of_whole_remainder_is_valid_and_preserves_other_stripes() -> TestResult {
        let book = value(value(seeded(2, 3)?, 0, 1, "delete")?, 1, 1, "keep")?;
        let shifted = apply(
            &book,
            address_at(&book.sheets()[0], 0, 1)?,
            1,
            2,
            CellShiftAxis::Column,
            false,
        )?;
        assert_eq!(
            shifted.cell_content(address_at(&book.sheets()[0], 0, 1)?)?,
            None
        );
        assert_eq!(
            shifted.cell_content(address_at(&book.sheets()[0], 1, 1)?)?,
            book.cell_content(address_at(&book.sheets()[0], 1, 1)?)?
        );
        Ok(())
    }

    #[test]
    fn inbound_reference_follows_move_and_deleted_reference_refuses_atomically() -> TestResult {
        let mut book = value(seeded(2, 5)?, 0, 2, "target")?;
        let origin = address_at(&book.sheets()[0], 1, 0)?;
        let formula = spreadsheet_formula::binder::bind("=C1", &book, origin)?;
        book = book.candidate(
            &Operation::SetCellFormula {
                address: origin,
                formula,
            },
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let source = address_at(&book.sheets()[0], 0, 1)?;
        let shifted = apply(&book, source, 1, 1, CellShiftAxis::Column, true)?;
        let Some(CellContent::Formula(formula)) = shifted.cell_content(origin)? else {
            return Err("formula missing".into());
        };
        assert!(formula.ast().nodes().iter().any(|node| matches!(node, Node::Cell(reference) if reference.target_column == book.sheets()[0].columns()[3])));
        assert!(matches!(
            plan_shift(
                &book,
                address_at(&book.sheets()[0], 0, 2)?,
                1,
                1,
                CellShiftAxis::Column,
                false
            ),
            Err(SessionError::UnsupportedMoveDependency)
        ));
        Ok(())
    }

    #[test]
    fn merges_in_shifted_stripe_refuse_before_writes() -> TestResult {
        let book = seeded(2, 5)?;
        let sheet = &book.sheets()[0];
        let range = MergeRange::new(
            sheet.rows()[0],
            sheet.rows()[0],
            sheet.columns()[3],
            sheet.columns()[4],
        );
        let book = book.candidate(
            &Operation::MergeWrite {
                sheet: sheet.id(),
                change: spreadsheet_domain::merge::MergeChange {
                    range,
                    merged: true,
                    previous: false,
                },
                undoes: None,
                undoes_version: None,
            },
            DocumentVersion::new(2)?,
        )?;
        assert!(matches!(
            plan_shift(
                &book,
                address_at(&book.sheets()[0], 0, 1)?,
                1,
                1,
                CellShiftAxis::Column,
                false
            ),
            Err(SessionError::Domain(DomainError::MergeConflict))
        ));
        Ok(())
    }
}
