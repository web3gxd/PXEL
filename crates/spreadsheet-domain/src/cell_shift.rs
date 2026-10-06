//! Replay/admission validation for frozen cell shifts. Formula relocation uses
//! the existing domain move validator, retaining the same integrity contract.
use crate::{
    formula::Node,
    style::{CellStyle, StyleProperty},
    workbook::{
        CellAddress, CellContent, CellShiftAxis, DocumentVersion, DomainError, GuardedContentWrite,
        GuardedStyleWrite, Operation, Sheet, Workbook,
    },
};
use std::collections::{BTreeMap, BTreeSet};

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

fn at(sheet: &Sheet, row: u32, column: u32) -> Result<CellAddress, DomainError> {
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

fn erased_dependency(
    book: &Workbook,
    sheet: &Sheet,
    erased: &BTreeSet<CellAddress>,
) -> Result<(), DomainError> {
    let first = erased.iter().next().ok_or(DomainError::InvalidAxis)?;
    let mut first_row = sheet
        .row_position(first.row)
        .ok_or(DomainError::MissingRow)?;
    let mut last_row = first_row;
    let mut first_column = sheet
        .column_position(first.column)
        .ok_or(DomainError::MissingColumn)?;
    let mut last_column = first_column;
    for address in erased {
        let row = sheet
            .row_position(address.row)
            .ok_or(DomainError::MissingRow)?;
        let column = sheet
            .column_position(address.column)
            .ok_or(DomainError::MissingColumn)?;
        first_row = first_row.min(row);
        last_row = last_row.max(row);
        first_column = first_column.min(column);
        last_column = last_column.max(column);
    }
    let mut visited = 0usize;
    for current in book.sheets() {
        for (&origin, content) in current.cells() {
            if erased.contains(&origin) {
                continue;
            }
            let CellContent::Formula(formula) = content else {
                continue;
            };
            visited = visited
                .checked_add(formula.ast().nodes().len())
                .ok_or(DomainError::LimitExceeded)?;
            if visited > 1_000_000 {
                return Err(DomainError::LimitExceeded);
            }
            for node in formula.ast().nodes() {
                let touches = match node {
                    Node::Cell(reference) => erased.contains(&CellAddress {
                        sheet: reference.target_sheet,
                        row: reference.target_row,
                        column: reference.target_column,
                    }),
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
                        r1.min(r2) <= last_row
                            && r1.max(r2) >= first_row
                            && c1.min(c2) <= last_column
                            && c1.max(c2) >= first_column
                    }
                    _ => false,
                };
                if touches {
                    return Err(DomainError::UnsupportedStructuralDependency);
                }
            }
        }
    }
    Ok(())
}

/// Validate an authored shift against its input state, including the complete
/// stripe footprint, frozen scalar/formula relocation and all sixteen styles.
/// Inverses use the existing durable-history and exact swapped-preimage guards.
#[allow(clippy::too_many_arguments)]
pub fn validate(
    book: &Workbook,
    source: CellAddress,
    rows: u32,
    columns: u32,
    axis: CellShiftAxis,
    insert: bool,
    content_writes: &[GuardedContentWrite],
    style_writes: &[GuardedStyleWrite],
) -> Result<(), DomainError> {
    book.cell_content(source)?;
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == source.sheet)
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
        return Err(DomainError::InvalidAxis);
    }
    let stripe_rows = if axis == CellShiftAxis::Row {
        sheet.rows().len() as u32 - row
    } else {
        rows
    };
    let stripe_columns = if axis == CellShiftAxis::Column {
        sheet.columns().len() as u32 - column
    } else {
        columns
    };
    if u64::from(stripe_rows) * u64::from(stripe_columns) > 1000
        || content_writes.len() > 1000
        || style_writes.len() > 1000
    {
        return Err(DomainError::LimitExceeded);
    }
    for merge in sheet.merges() {
        let r1 = sheet
            .row_position(merge.first_row())
            .ok_or(DomainError::MissingRow)?;
        let r2 = sheet
            .row_position(merge.last_row())
            .ok_or(DomainError::MissingRow)?;
        let c1 = sheet
            .column_position(merge.first_column())
            .ok_or(DomainError::MissingColumn)?;
        let c2 = sheet
            .column_position(merge.last_column())
            .ok_or(DomainError::MissingColumn)?;
        if r1 < row + stripe_rows && r2 >= row && c1 < column + stripe_columns && c2 >= column {
            return Err(DomainError::MergeConflict);
        }
    }
    let mut stripe = BTreeSet::new();
    let mut erased = BTreeSet::new();
    let erased_row = if insert && axis == CellShiftAxis::Row {
        row + stripe_rows - rows
    } else {
        row
    };
    let erased_column = if insert && axis == CellShiftAxis::Column {
        column + stripe_columns - columns
    } else {
        column
    };
    for r in row..row + stripe_rows {
        for c in column..column + stripe_columns {
            stripe.insert(at(sheet, r, c)?);
        }
    }
    for r in erased_row..erased_row + rows {
        for c in erased_column..erased_column + columns {
            let address = at(sheet, r, c)?;
            if insert
                && (book.cell_content(address)?.is_some() || sheet.styles().contains_key(&address))
            {
                return Err(DomainError::InvalidValue);
            }
            erased.insert(address);
        }
    }
    erased_dependency(book, sheet, &erased)?;
    let by_address = content_writes
        .iter()
        .map(|write| (write.address, write))
        .collect::<BTreeMap<_, _>>();
    if by_address.len() != content_writes.len()
        || stripe
            .iter()
            .any(|address| !by_address.contains_key(address))
    {
        return Err(DomainError::InvalidAxis);
    }
    for write in content_writes {
        if book.cell_content(write.address)? != write.previous.as_ref() {
            return Err(DomainError::PreimageConflict);
        }
    }
    let (from_row, from_column, to_row, to_column, moved_rows, moved_columns) = match (axis, insert)
    {
        (CellShiftAxis::Row, true) => {
            (row, column, row + rows, column, stripe_rows - rows, columns)
        }
        (CellShiftAxis::Row, false) => {
            (row + rows, column, row, column, stripe_rows - rows, columns)
        }
        (CellShiftAxis::Column, true) => (
            row,
            column,
            row,
            column + columns,
            rows,
            stripe_columns - columns,
        ),
        (CellShiftAxis::Column, false) => (
            row,
            column + columns,
            row,
            column,
            rows,
            stripe_columns - columns,
        ),
    };
    let mut remap = BTreeMap::new();
    let mut moved_footprint = BTreeSet::new();
    let mut styles = stripe
        .iter()
        .map(|address| (*address, CellStyle::default()))
        .collect::<BTreeMap<_, _>>();
    for r in 0..moved_rows {
        for c in 0..moved_columns {
            let from = at(sheet, from_row + r, from_column + c)?;
            let to = at(sheet, to_row + r, to_column + c)?;
            remap.insert(from, to);
            moved_footprint.insert(from);
            moved_footprint.insert(to);
            styles.insert(to, sheet.styles().get(&from).cloned().unwrap_or_default());
        }
    }
    let next = DocumentVersion::new(
        book.version()
            .get()
            .checked_add(1)
            .ok_or(DomainError::LimitExceeded)?,
    )?;
    let moved = if remap.is_empty() {
        if by_address.keys().any(|address| !stripe.contains(address)) {
            return Err(DomainError::InvalidAxis);
        }
        None
    } else {
        let writes = content_writes
            .iter()
            .filter(|write| {
                moved_footprint.contains(&write.address) || !stripe.contains(&write.address)
            })
            .cloned()
            .collect();
        Some(book.candidate(
            &Operation::MoveRange {
                source: at(sheet, from_row, from_column)?,
                destination: at(sheet, to_row, to_column)?,
                rows: moved_rows,
                columns: moved_columns,
                writes,
            },
            next,
        )?)
    };
    for (&address, write) in &by_address {
        let expected = if moved_footprint.contains(&address) || !stripe.contains(&address) {
            moved
                .as_ref()
                .ok_or(DomainError::InvalidAxis)?
                .cell_content(address)?
        } else {
            None
        };
        if write.content.as_ref() != expected {
            return Err(DomainError::InvalidValue);
        }
    }
    let expected_styles = styles
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
        .collect::<Vec<_>>();
    if expected_styles != style_writes {
        return Err(DomainError::InvalidStyle);
    }
    Ok(())
}
