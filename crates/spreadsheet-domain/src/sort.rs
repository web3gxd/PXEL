//! Stable geometry and independently bounded sort intent. No calculated values or IO.
use crate::workbook::{CellAddress, ColumnId, DomainError, RowId, Workbook};
use crate::{
    style::{CellStyle, StyleProperty},
    style_write::GuardedStyleWrite,
};
use std::collections::BTreeSet;

pub const MAX_SORT_CELLS: u32 = 100_000;
pub const MAX_SORT_KEYS: usize = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SortKey {
    pub column: ColumnId,
    pub descending: bool,
}

/// Freeze replacement of every style leaf where a row permutation changes style.
/// Empty cells keep sparse formatting; destination-only leaves are reset.
pub fn style_writes(
    book: &Workbook,
    range: &SortRange,
    order: &[RowId],
) -> Result<Vec<GuardedStyleWrite>, DomainError> {
    let bounds = range.validate_order(book, order)?;
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == range.source.sheet)
        .ok_or(DomainError::MissingSheet)?;
    let properties = [
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
    let default = CellStyle::default();
    let mut writes = Vec::new();
    for (offset, row) in order.iter().enumerate() {
        let destination_row = sheet.rows()[(bounds.row + range.header_rows) as usize + offset];
        for column in
            &sheet.columns()[bounds.column as usize..(bounds.column + range.columns) as usize]
        {
            let source = CellAddress {
                sheet: sheet.id(),
                row: *row,
                column: *column,
            };
            let destination = CellAddress {
                row: destination_row,
                ..source
            };
            let source_style = sheet.styles().get(&source).unwrap_or(&default);
            let previous = sheet.styles().get(&destination).unwrap_or(&default);
            if source_style != previous {
                writes.push(GuardedStyleWrite {
                    address: destination,
                    previous: properties.iter().map(|p| previous.change(*p)).collect(),
                    changes: properties.iter().map(|p| source_style.change(*p)).collect(),
                });
            }
        }
    }
    writes.sort_by_key(|w| w.address);
    Ok(writes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortRange {
    pub source: CellAddress,
    pub rows: u32,
    pub columns: u32,
    /// The selected header and preceding rows remain in place.
    pub header_rows: u32,
    pub keys: Vec<SortKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SortBounds {
    pub row: u32,
    pub column: u32,
}

impl SortRange {
    pub fn validate_shape(&self) -> Result<(), DomainError> {
        if self.rows == 0
            || self.columns == 0
            || self
                .rows
                .checked_mul(self.columns)
                .is_none_or(|n| n > MAX_SORT_CELLS)
            || self.header_rows >= self.rows
            || self.keys.is_empty()
            || self.keys.len() > MAX_SORT_KEYS
            || self.keys.len() > self.columns as usize
        {
            return Err(DomainError::LimitExceeded);
        }
        let mut columns = BTreeSet::new();
        if self.keys.iter().any(|key| !columns.insert(key.column)) {
            return Err(DomainError::InvalidAxis);
        }
        Ok(())
    }

    pub fn validate(&self, book: &Workbook) -> Result<SortBounds, DomainError> {
        self.validate_shape()?;
        let sheet = book
            .sheets()
            .iter()
            .find(|s| s.id() == self.source.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let row = sheet
            .row_position(self.source.row)
            .ok_or(DomainError::MissingRow)?;
        let column = sheet
            .column_position(self.source.column)
            .ok_or(DomainError::MissingColumn)?;
        let end_row = row
            .checked_add(self.rows)
            .filter(|n| *n as usize <= sheet.rows().len())
            .ok_or(DomainError::InvalidAxis)?;
        let end_column = column
            .checked_add(self.columns)
            .filter(|n| *n as usize <= sheet.columns().len())
            .ok_or(DomainError::InvalidAxis)?;
        for key in &self.keys {
            let position = sheet
                .column_position(key.column)
                .ok_or(DomainError::MissingColumn)?;
            if position < column || position >= end_column {
                return Err(DomainError::InvalidAxis);
            }
        }
        for merge in sheet.merges() {
            let first_row = sheet
                .row_position(merge.first_row())
                .ok_or(DomainError::MissingRow)?;
            let last_row = sheet
                .row_position(merge.last_row())
                .ok_or(DomainError::MissingRow)?;
            let first_column = sheet
                .column_position(merge.first_column())
                .ok_or(DomainError::MissingColumn)?;
            let last_column = sheet
                .column_position(merge.last_column())
                .ok_or(DomainError::MissingColumn)?;
            if first_row < end_row
                && last_row >= row + self.header_rows
                && first_column < end_column
                && last_column >= column
            {
                return Err(DomainError::MergeConflict);
            }
        }
        Ok(SortBounds { row, column })
    }

    /// Frozen source rows for each destination body row must be an exact permutation.
    pub fn validate_order(
        &self,
        book: &Workbook,
        order: &[RowId],
    ) -> Result<SortBounds, DomainError> {
        let bounds = self.validate(book)?;
        let sheet = book
            .sheets()
            .iter()
            .find(|s| s.id() == self.source.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let expected = &sheet.rows()
            [(bounds.row + self.header_rows) as usize..(bounds.row + self.rows) as usize];
        if expected.len() != order.len()
            || expected.iter().copied().collect::<BTreeSet<_>>() != order.iter().copied().collect()
        {
            return Err(DomainError::InvalidAxis);
        }
        Ok(bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MergeRange, workbook::*};

    #[test]
    fn sort_extends_only_its_own_atomic_batch_limit() -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([2; 16])?;
        let column = ColumnId::from_bytes([3; 16])?;
        let rows = (1..=1001_u128)
            .map(|n| RowId::from_bytes(n.to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        let source = CellAddress {
            sheet,
            row: rows[0],
            column,
        };
        let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Sheet1".into(),
                    rows: rows.clone(),
                    columns: vec![column],
                },
            },
            DocumentVersion::new(1)?,
        )?;
        let mut operation = Operation::CellBatchWrite {
            intent: CellBatchIntent::SortRange {
                range: SortRange {
                    source,
                    rows: 1001,
                    columns: 1,
                    header_rows: 0,
                    keys: vec![SortKey {
                        column,
                        descending: false,
                    }],
                },
                order: rows.clone(),
            },
            content_writes: rows
                .iter()
                .map(|row| GuardedContentWrite {
                    address: CellAddress {
                        row: *row,
                        ..source
                    },
                    content: None,
                    previous: None,
                })
                .collect(),
            style_writes: vec![],
            undoes: None,
            undoes_version: None,
        };
        assert!(book.candidate(&operation, DocumentVersion::new(2)?).is_ok());
        let Operation::CellBatchWrite { intent, .. } = &mut operation else {
            return Err("missing batch".into());
        };
        *intent = CellBatchIntent::SetValues;
        assert_eq!(
            book.candidate(&operation, DocumentVersion::new(2)?).err(),
            Some(DomainError::LimitExceeded)
        );
        Ok(())
    }

    fn fixture() -> Result<(Workbook, SortRange), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([2; 16])?;
        let rows = (3..7)
            .map(|n| RowId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let columns = (7..9)
            .map(|n| ColumnId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let range = SortRange {
            source: CellAddress {
                sheet,
                row: rows[0],
                column: columns[0],
            },
            rows: 4,
            columns: 2,
            header_rows: 1,
            keys: vec![SortKey {
                column: columns[0],
                descending: false,
            }],
        };
        let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Sheet1".into(),
                    rows,
                    columns,
                },
            },
            DocumentVersion::new(1)?,
        )?;
        Ok((book, range))
    }

    #[test]
    fn order_excludes_headers_and_requires_every_body_row_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let (book, range) = fixture()?;
        let rows = book.sheets()[0].rows();
        assert!(
            range
                .validate_order(&book, &[rows[3], rows[1], rows[2]])
                .is_ok()
        );
        for invalid in [
            vec![rows[1], rows[1], rows[2]],
            vec![rows[0], rows[2], rows[3]],
            vec![rows[1], rows[2]],
        ] {
            assert_eq!(
                range.validate_order(&book, &invalid),
                Err(DomainError::InvalidAxis)
            );
        }
        Ok(())
    }

    #[test]
    fn limit_is_full_selected_rectangle_and_keys_are_inside_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let (book, mut range) = fixture()?;
        range.rows = 50_000;
        assert!(range.validate_shape().is_ok());
        range.rows += 1;
        assert_eq!(range.validate_shape(), Err(DomainError::LimitExceeded));
        range.rows = 4;
        range.columns = 1;
        range.keys[0].column = book.sheets()[0].columns()[1];
        assert_eq!(range.validate(&book), Err(DomainError::InvalidAxis));
        range.keys[0].column = book.sheets()[0].columns()[0];
        range.keys.push(range.keys[0]);
        assert!(range.validate_shape().is_err());
        Ok(())
    }

    #[test]
    fn any_body_merge_rejects_but_header_only_merge_is_allowed()
    -> Result<(), Box<dyn std::error::Error>> {
        let (book, range) = fixture()?;
        let sheet = &book.sheets()[0];
        for (last, accepted) in [(0, true), (1, false)] {
            let merge = MergeRange::new(
                sheet.rows()[0],
                sheet.rows()[last],
                sheet.columns()[0],
                sheet.columns()[1],
            );
            let operation = Operation::MergeWrite {
                sheet: sheet.id(),
                change: crate::MergeChange {
                    range: merge,
                    merged: true,
                    previous: false,
                },
                undoes: None,
                undoes_version: None,
            };
            let merged = book.candidate(&operation, DocumentVersion::new(2)?)?;
            assert_eq!(range.validate(&merged).is_ok(), accepted);
        }
        Ok(())
    }
}
