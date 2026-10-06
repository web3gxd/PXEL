//! Canonical stable-axis geometry for merged cells.
use crate::workbook::{ColumnId, DomainError, RowId};

pub const MAX_MERGE_CELLS: usize = 10_000;
pub const MAX_MERGE_RANGES_PER_SHEET: usize = 10_000;
pub const MAX_MERGE_COVERED_CELLS_PER_SHEET: usize = 100_000;

/// Rectangle endpoints are stable axis IDs, not presentation coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MergeRange {
    first_row: RowId,
    last_row: RowId,
    first_column: ColumnId,
    last_column: ColumnId,
}

impl MergeRange {
    pub const fn new(
        first_row: RowId,
        last_row: RowId,
        first_column: ColumnId,
        last_column: ColumnId,
    ) -> Self {
        Self {
            first_row,
            last_row,
            first_column,
            last_column,
        }
    }

    pub const fn first_row(self) -> RowId {
        self.first_row
    }
    pub const fn last_row(self) -> RowId {
        self.last_row
    }
    pub const fn first_column(self) -> ColumnId {
        self.first_column
    }
    pub const fn last_column(self) -> ColumnId {
        self.last_column
    }

    pub(crate) fn bounds(
        self,
        row_positions: &std::collections::HashMap<RowId, u32>,
        column_positions: &std::collections::HashMap<ColumnId, u32>,
    ) -> Result<(u32, u32, u32, u32), DomainError> {
        let first_row = *row_positions
            .get(&self.first_row)
            .ok_or(DomainError::MissingRow)?;
        let last_row = *row_positions
            .get(&self.last_row)
            .ok_or(DomainError::MissingRow)?;
        let first_column = *column_positions
            .get(&self.first_column)
            .ok_or(DomainError::MissingColumn)?;
        let last_column = *column_positions
            .get(&self.last_column)
            .ok_or(DomainError::MissingColumn)?;
        if first_row > last_row || first_column > last_column {
            return Err(DomainError::InvalidAxis);
        }
        let height = last_row - first_row + 1;
        let width = last_column - first_column + 1;
        let area = u64::from(height)
            .checked_mul(u64::from(width))
            .ok_or(DomainError::LimitExceeded)?;
        if area > MAX_MERGE_CELLS as u64 {
            return Err(DomainError::LimitExceeded);
        }
        Ok((first_row, last_row, first_column, last_column))
    }
    pub(crate) fn anchor(self) -> (RowId, ColumnId) {
        (self.first_row, self.first_column)
    }
}

/// Persisted target and exact membership preimage for guarded Undo/Redo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MergeChange {
    pub range: MergeRange,
    pub merged: bool,
    pub previous: bool,
}

/// Freeze only occupied/styled non-anchor cells. Clearing and membership are
/// published together; these exact preimages are retained solely for Undo.
pub fn prepare_merge_cells(
    book: &crate::workbook::Workbook,
    sheet: crate::workbook::SheetId,
    range: MergeRange,
) -> Result<crate::workbook::Operation, DomainError> {
    use crate::workbook::*;
    let target = book
        .sheets()
        .iter()
        .find(|s| s.id() == sheet)
        .ok_or(DomainError::MissingSheet)?;
    let (top, bottom, left, right) = target.merge_bounds(range)?;
    let mut content_writes = Vec::new();
    let mut style_writes = Vec::new();
    for row in top..=bottom {
        for column in left..=right {
            if row == top && column == left {
                continue;
            }
            let address = CellAddress {
                sheet,
                row: target.rows()[row as usize],
                column: target.columns()[column as usize],
            };
            let previous = book.cell_content(address)?.cloned();
            let style = target.styles().get(&address);
            if previous.is_some() || style.is_some() {
                content_writes.push(GuardedContentWrite {
                    address,
                    previous,
                    content: None,
                });
                if content_writes.len() > 1_000 {
                    return Err(DomainError::LimitExceeded);
                }
            }
            if let Some(style) = style {
                let previous = style.normalized_changes();
                let default = crate::style::CellStyle::default();
                let changes = previous
                    .iter()
                    .map(|p| default.change(p.property()))
                    .collect();
                style_writes.push(crate::style_write::GuardedStyleWrite {
                    address,
                    previous,
                    changes,
                });
            }
        }
    }
    content_writes.sort_by_key(|w| w.address);
    style_writes.sort_by_key(|w| w.address);
    Ok(Operation::MergeCellsWrite {
        sheet,
        change: MergeChange {
            range,
            merged: true,
            previous: false,
        },
        content_writes,
        style_writes,
        undoes: None,
        undoes_version: None,
    })
}

pub fn inverse_merge_cells(
    operation: &crate::workbook::Operation,
    id: crate::workbook::OperationId,
    version: crate::workbook::DocumentVersion,
) -> Result<crate::workbook::Operation, DomainError> {
    use crate::workbook::*;
    let Operation::MergeCellsWrite {
        sheet,
        change,
        content_writes,
        style_writes,
        ..
    } = operation
    else {
        return Err(DomainError::InvalidAxis);
    };
    Ok(Operation::MergeCellsWrite {
        sheet: *sheet,
        change: MergeChange {
            range: change.range,
            merged: !change.merged,
            previous: change.merged,
        },
        content_writes: content_writes
            .iter()
            .map(|w| GuardedContentWrite {
                address: w.address,
                content: w.previous.clone(),
                previous: w.content.clone(),
            })
            .collect(),
        style_writes: style_writes
            .iter()
            .map(|w| crate::style_write::GuardedStyleWrite {
                address: w.address,
                changes: w.previous.clone(),
                previous: w.changes.clone(),
            })
            .collect(),
        undoes: Some(id),
        undoes_version: Some(version),
    })
}
