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
