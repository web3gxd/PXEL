//! Reverse rectangle lookup. Rebuilt when dependency geometry changes, not on value edits.
use crate::dependency::DependencyLimitExceeded;
use spreadsheet_domain::workbook::{CellAddress, SheetId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeDependency {
    pub formula: CellAddress,
    pub sheet: SheetId,
    pub first_row: u32,
    pub last_row: u32,
    pub first_column: u32,
    pub last_column: u32,
}

#[derive(Clone, Debug)]
struct Node {
    range: RangeDependency,
    max_row: u32,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct Tree {
    nodes: Vec<Node>,
    root: Option<usize>,
}

impl Tree {
    // Median construction has logarithmic depth, independently of user input ordering.
    fn build(&mut self, ranges: &[RangeDependency]) -> Option<usize> {
        if ranges.is_empty() {
            return None;
        }
        let middle = ranges.len() / 2;
        let left = self.build(&ranges[..middle]);
        let right = self.build(&ranges[middle + 1..]);
        let range = ranges[middle];
        let mut max_row = range.last_row;
        for child in [left, right].into_iter().flatten() {
            max_row = max_row.max(self.nodes[child].max_row);
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            range,
            max_row,
            left,
            right,
        });
        Some(index)
    }
}

#[derive(Clone, Debug, Default)]
pub struct RangeIndex {
    sheets: BTreeMap<SheetId, Tree>,
}

impl RangeIndex {
    pub const MAX_RANGES: usize = 100_000;

    pub fn build(
        ranges: impl IntoIterator<Item = RangeDependency>,
    ) -> Result<Self, DependencyLimitExceeded> {
        let mut grouped: BTreeMap<SheetId, Vec<RangeDependency>> = BTreeMap::new();
        for (count, range) in ranges.into_iter().enumerate() {
            if count >= Self::MAX_RANGES
                || range.first_row > range.last_row
                || range.first_column > range.last_column
            {
                return Err(DependencyLimitExceeded);
            }
            grouped.entry(range.sheet).or_default().push(range);
        }
        let mut sheets = BTreeMap::new();
        for (sheet, mut ranges) in grouped {
            ranges.sort_by_key(|range| {
                (
                    range.first_row,
                    range.last_row,
                    range.first_column,
                    range.last_column,
                    range.formula,
                )
            });
            let mut tree = Tree::default();
            tree.root = tree.build(&ranges);
            sheets.insert(sheet, tree);
        }
        Ok(Self { sheets })
    }

    /// Returns matching formulas. Work counts visited interval nodes, including column misses.
    pub fn affected(
        &self,
        sheet: SheetId,
        row: u32,
        column: u32,
        remaining: &mut usize,
    ) -> Result<BTreeSet<CellAddress>, DependencyLimitExceeded> {
        let mut affected = BTreeSet::new();
        let Some(tree) = self.sheets.get(&sheet) else {
            return Ok(affected);
        };
        let mut stack = Vec::new();
        if let Some(root) = tree.root {
            stack.push(root);
        }
        while let Some(index) = stack.pop() {
            *remaining = remaining.checked_sub(1).ok_or(DependencyLimitExceeded)?;
            let node = &tree.nodes[index];
            if node.max_row < row {
                continue;
            }
            if let Some(left) = node.left {
                stack.push(left);
            }
            if node.range.first_row <= row {
                if node.range.last_row >= row
                    && node.range.first_column <= column
                    && node.range.last_column >= column
                {
                    affected.insert(node.range.formula);
                }
                if let Some(right) = node.right {
                    stack.push(right);
                }
            }
        }
        Ok(affected)
    }
}
