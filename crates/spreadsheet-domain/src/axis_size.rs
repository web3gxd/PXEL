//! Sparse, stable-ID row heights and column widths.
use crate::workbook::{ColumnId, RowId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum AxisSizeTarget {
    Row(RowId),
    Column(ColumnId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AxisSizeChange {
    pub target: AxisSizeTarget,
    /// CSS pixels. `None` restores the axis default.
    pub size: Option<u32>,
    /// Exact sparse preimage, captured by the server.
    pub previous: Option<u32>,
}

pub const DEFAULT_ROW_HEIGHT: u32 = 28;
pub const DEFAULT_COLUMN_WIDTH: u32 = 112;
pub const MIN_ROW_HEIGHT: u32 = 12;
pub const MAX_ROW_HEIGHT: u32 = 1_000;
pub const MIN_COLUMN_WIDTH: u32 = 24;
pub const MAX_COLUMN_WIDTH: u32 = 2_000;
pub const MAX_AXIS_SIZE_WRITES: usize = 1_000;
pub const MAX_FROZEN_ROWS: u32 = 1_000;

pub fn valid_size(target: AxisSizeTarget, size: u32) -> bool {
    match target {
        AxisSizeTarget::Row(_) => (MIN_ROW_HEIGHT..=MAX_ROW_HEIGHT).contains(&size),
        AxisSizeTarget::Column(_) => (MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(&size),
    }
}
