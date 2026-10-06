//! Admission of delayed intents across stable axis identity reactivation.
use crate::formula::{Formula, Node};
use crate::workbook::*;

pub fn max_activation_version(book: &Workbook) -> DocumentVersion {
    book.row_activation_floors()
        .map(|(_, version)| version)
        .chain(book.column_activation_floors().map(|(_, version)| version))
        .fold(DocumentVersion::ZERO, DocumentVersion::max)
}

pub fn check_command_activation(book: &Workbook, command: &Command) -> Result<(), DomainError> {
    if command.document_id != book.document_id() {
        return Err(DomainError::VersionConflict);
    }
    let mut floor = DocumentVersion::ZERO;
    let mut address = |target: CellAddress| -> Result<(), DomainError> {
        floor = floor.max(book.address_admission_floor(target)?);
        Ok(())
    };
    match &command.operation {
        CommandPayload::SortRange { range } => {
            let bounds = range.validate(book)?;
            let sheet = book
                .sheets()
                .iter()
                .find(|s| s.id() == range.source.sheet)
                .ok_or(DomainError::MissingSheet)?;
            for row in &sheet.rows()[bounds.row as usize..(bounds.row + range.rows) as usize] {
                floor = floor.max(book.row_activation_floor(*row));
            }
            for column in
                &sheet.columns()[bounds.column as usize..(bounds.column + range.columns) as usize]
            {
                floor = floor.max(book.column_activation_floor(*column));
            }
        }
        CommandPayload::SetRangeStyle { addresses, .. } => {
            for target in addresses {
                address(*target)?;
            }
        }
        CommandPayload::SetCellStyle {
            address: target, ..
        } => address(*target)?,
        CommandPayload::SetCellValue {
            address: target, ..
        }
        | CommandPayload::ClearCell { address: target }
        | CommandPayload::SetCellFormula {
            address: target, ..
        } => address(*target)?,
        CommandPayload::SetCellValues { writes } => {
            for write in writes {
                address(write.address)?;
            }
        }
        CommandPayload::SetCellBatch { writes, styles } => {
            for write in writes {
                address(write.address)?;
            }
            for (target, _) in styles {
                address(*target)?;
            }
        }
        CommandPayload::CopyRange {
            source,
            destination,
            ..
        }
        | CommandPayload::MoveRange {
            source,
            destination,
            ..
        } => {
            address(*source)?;
            address(*destination)?;
        }
        CommandPayload::InsertRows { before, .. } => floor = book.row_activation_floor(*before),
        CommandPayload::DeleteRows { rows, .. } => {
            for row in rows {
                floor = floor.max(book.row_activation_floor(*row));
            }
        }
        CommandPayload::InsertColumns { before, .. } => {
            floor = book.column_activation_floor(*before)
        }
        CommandPayload::DeleteColumns { columns, .. } => {
            for column in columns {
                floor = floor.max(book.column_activation_floor(*column));
            }
        }
        CommandPayload::SetAxisSizes { writes, .. } => {
            for (target, _) in writes {
                floor = floor.max(match target {
                    AxisSizeTarget::Row(id) => book.row_activation_floor(*id),
                    AxisSizeTarget::Column(id) => book.column_activation_floor(*id),
                });
            }
        }
        CommandPayload::SetMergeRange { range, .. } => {
            floor = floor
                .max(book.row_activation_floor(range.first_row()))
                .max(book.row_activation_floor(range.last_row()))
                .max(book.column_activation_floor(range.first_column()))
                .max(book.column_activation_floor(range.last_column()));
        }
        CommandPayload::CreateWorkbook { .. }
        | CommandPayload::AddSheet { .. }
        | CommandPayload::ExtendRows { .. }
        | CommandPayload::UndoCell { .. }
        | CommandPayload::SetFrozenRows { .. } => {}
    }
    if command.base_version < floor {
        Err(DomainError::VersionConflict)
    } else {
        Ok(())
    }
}

/// Check references against the axes used to bind their A1 text, before a
/// deletion transform can erase target identity. `head` owns durable floors.
pub fn check_formula_activation(
    head: &Workbook,
    axes: &Workbook,
    formula: &Formula,
    base: DocumentVersion,
) -> Result<(), DomainError> {
    if head.document_id() != axes.document_id() || base > head.version() {
        return Err(DomainError::VersionConflict);
    }
    // Admission relies on hash-covered floors, not mutable snapshot metadata.
    if base >= max_activation_version(head) {
        return Ok(());
    }
    let mut budget = 1_000_000usize;
    let mut charge = || -> Result<(), DomainError> {
        budget = budget.checked_sub(1).ok_or(DomainError::LimitExceeded)?;
        Ok(())
    };
    for node in formula.ast().nodes() {
        charge()?;
        let references = match node {
            Node::Cell(reference) => vec![reference],
            Node::Range(range) => vec![&range.start, &range.end],
            _ => continue,
        };
        for reference in references {
            if head.row_activation_floor(reference.target_row) > base
                || head.column_activation_floor(reference.target_column) > base
            {
                return Err(DomainError::VersionConflict);
            }
        }
        let Node::Range(range) = node else {
            continue;
        };
        let sheet = axes
            .sheets()
            .iter()
            .find(|sheet| sheet.id() == range.start.target_sheet)
            .ok_or(DomainError::MissingSheet)?;
        let rows = sheet
            .row_position(range.start.target_row)
            .zip(sheet.row_position(range.end.target_row));
        let columns = sheet
            .column_position(range.start.target_column)
            .zip(sheet.column_position(range.end.target_column));
        if let Some((start, end)) = rows {
            for (id, floor) in head.row_activation_floors() {
                charge()?;
                if floor > base
                    && sheet.row_position(id).is_some_and(|position| {
                        position >= start.min(end) && position <= start.max(end)
                    })
                {
                    return Err(DomainError::VersionConflict);
                }
            }
        }
        if let Some((start, end)) = columns {
            for (id, floor) in head.column_activation_floors() {
                charge()?;
                if floor > base
                    && sheet.column_position(id).is_some_and(|position| {
                        position >= start.min(end) && position <= start.max(end)
                    })
                {
                    return Err(DomainError::VersionConflict);
                }
            }
        }
    }
    Ok(())
}
