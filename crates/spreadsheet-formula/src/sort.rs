//! Confirmed-value ordering and formula relocation; no clocks or IO.
use crate::Value;
use spreadsheet_domain::{sort::SortRange, workbook::*};
use std::{cmp::Ordering, collections::BTreeMap};

#[derive(Debug, thiserror::Error)]
pub enum SortError {
    #[error("sort geometry is invalid")]
    Domain(#[from] DomainError),
    #[error("confirmed formula results are unavailable")]
    CalculationUnavailable,
    #[error("formula relocation failed")]
    FormulaRelocation,
}

pub struct SortPlan {
    pub order: Vec<RowId>,
    pub content_writes: Vec<GuardedContentWrite>,
}

fn rank(value: &Value) -> u8 {
    match value {
        Value::Number(n) | Value::DateTime { serial: n, .. } if n.is_finite() => 0,
        Value::Text(text) if !text.is_empty() => 1,
        Value::Boolean(_) => 2,
        Value::Blank | Value::Text(_) => 3,
        _ => 4,
    }
}

fn compare(left: &Value, right: &Value, descending: bool) -> Ordering {
    let (a, b) = (rank(left), rank(right));
    // Empty values and errors remain at the end for either direction.
    if a >= 3 || b >= 3 {
        return a.cmp(&b);
    }
    let numeric = |value: &Value| match value {
        Value::Number(n) | Value::DateTime { serial: n, .. } => *n,
        _ => 0.0,
    };
    let order = if a != b {
        a.cmp(&b)
    } else {
        match (left, right) {
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
            _ => numeric(left)
                .partial_cmp(&numeric(right))
                .unwrap_or(Ordering::Equal),
        }
    };
    if descending { order.reverse() } else { order }
}

/// The caller must stamp the supplied confirmed result map with book version/context.
pub fn plan(
    book: &Workbook,
    range: &SortRange,
    calculated: &BTreeMap<CellAddress, Value>,
) -> Result<SortPlan, SortError> {
    let bounds = range.validate(book)?;
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == range.source.sheet)
        .ok_or(DomainError::MissingSheet)?;
    let mut rows = Vec::with_capacity((range.rows - range.header_rows) as usize);
    for index in bounds.row + range.header_rows..bounds.row + range.rows {
        let row = sheet.rows()[index as usize];
        let mut keys = Vec::with_capacity(range.keys.len());
        for key in &range.keys {
            let address = CellAddress {
                sheet: sheet.id(),
                row,
                column: key.column,
            };
            let value = match book.cell_content(address)? {
                None | Some(CellContent::Value(CellValue::Blank)) => Value::Blank,
                Some(CellContent::Value(CellValue::Number(n))) => Value::Number(*n),
                Some(CellContent::Value(CellValue::Text(text))) => Value::Text(text.clone()),
                Some(CellContent::Value(CellValue::Boolean(value))) => Value::Boolean(*value),
                Some(CellContent::Formula(_)) => calculated
                    .get(&address)
                    .cloned()
                    .ok_or(SortError::CalculationUnavailable)?,
            };
            keys.push(value);
        }
        rows.push((index, row, keys));
    }
    rows.sort_by(|a, b| {
        a.2.iter()
            .zip(&b.2)
            .zip(&range.keys)
            .map(|((a, b), key)| compare(a, b, key.descending))
            .find(|order| *order != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    });
    let order = rows.iter().map(|(_, row, _)| *row).collect::<Vec<_>>();
    let mut content_writes =
        Vec::with_capacity((range.rows - range.header_rows) as usize * range.columns as usize);
    for (offset, (source_index, source_row, _)) in rows.iter().enumerate() {
        let destination_index = bounds.row + range.header_rows + offset as u32;
        for column_index in bounds.column..bounds.column + range.columns {
            let column = sheet.columns()[column_index as usize];
            let source = CellAddress {
                sheet: sheet.id(),
                row: *source_row,
                column,
            };
            let destination = CellAddress {
                sheet: sheet.id(),
                row: sheet.rows()[destination_index as usize],
                column,
            };
            let content = match book.cell_content(source)? {
                Some(CellContent::Formula(formula)) if source != destination => {
                    let current = crate::bound::render_current(formula, book)
                        .map_err(|_| SortError::FormulaRelocation)?;
                    let copied = crate::copy_formula_text(
                        &current,
                        (*source_index, column_index),
                        (destination_index, column_index),
                    )
                    .map_err(|_| SortError::FormulaRelocation)?;
                    Some(CellContent::Formula(
                        crate::binder::bind(&copied, book, destination)
                            .map_err(|_| SortError::FormulaRelocation)?,
                    ))
                }
                content => content.cloned(),
            };
            content_writes.push(GuardedContentWrite {
                address: destination,
                content,
                previous: book.cell_content(destination)?.cloned(),
            });
        }
    }
    Ok(SortPlan {
        order,
        content_writes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Value, binder};
    use spreadsheet_domain::sort::*;
    use std::collections::BTreeMap;

    fn fixture() -> Result<(Workbook, SortRange), Box<dyn std::error::Error>> {
        let rows = (3..7)
            .map(|n| RowId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let columns = (7..10)
            .map(|n| ColumnId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let sheet = SheetId::from_bytes([2; 16])?;
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
                column: columns[1],
                descending: false,
            }],
        };
        let mut book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Sheet1".into(),
                    rows: rows.clone(),
                    columns: columns.clone(),
                },
            },
            DocumentVersion::new(1)?,
        )?;
        for (index, number) in [2.0, 3.0, 1.0, 2.0].into_iter().enumerate() {
            let address = CellAddress {
                sheet,
                row: rows[index],
                column: columns[0],
            };
            book = book.candidate(
                &Operation::SetCellValue {
                    address,
                    value: CellValue::Number(number),
                },
                DocumentVersion::new(book.version().get() + 1)?,
            )?;
        }
        for (index, row) in rows.iter().copied().enumerate().take(4).skip(1) {
            let address = CellAddress {
                sheet,
                row,
                column: columns[1],
            };
            let formula = binder::bind(&format!("=A{}*$A$1", index + 1), &book, address)?;
            book = book.candidate(
                &Operation::SetCellFormula { address, formula },
                DocumentVersion::new(book.version().get() + 1)?,
            )?;
        }
        Ok((book, range))
    }

    #[test]
    fn sorting_uses_confirmed_results_moves_mixed_formulas_and_excludes_headers()
    -> Result<(), Box<dyn std::error::Error>> {
        let (book, range) = fixture()?;
        let sheet = &book.sheets()[0];
        let calculated = (1..4)
            .zip([6.0, 2.0, 4.0])
            .map(|(index, number)| {
                (
                    CellAddress {
                        sheet: sheet.id(),
                        row: sheet.rows()[index],
                        column: sheet.columns()[1],
                    },
                    Value::Number(number),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let output = plan(&book, &range, &calculated)?;
        assert_eq!(
            output.order,
            vec![sheet.rows()[2], sheet.rows()[3], sheet.rows()[1]]
        );
        assert_eq!(output.content_writes.len(), 6);
        let destination = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[3],
            column: sheet.columns()[1],
        };
        let moved = output
            .content_writes
            .iter()
            .find(|w| w.address == destination)
            .ok_or("missing moved formula")?;
        let Some(CellContent::Formula(formula)) = &moved.content else {
            return Err("missing formula".into());
        };
        assert_eq!(crate::bound::render_current(formula, &book)?, "=(A4*$A$1)");
        assert!(output.content_writes.iter().all(|w| w.address.row != sheet.rows()[0] && w.address.column != sheet.columns()[2]));
        assert_eq!(book.version().get(), 8);
        assert!(plan(&book, &range, &BTreeMap::new()).is_err());
        let operation = Operation::CellBatchWrite {
            intent: CellBatchIntent::SortRange {
                range: range.clone(),
                order: output.order.clone(),
            },
            content_writes: output.content_writes.clone(),
            style_writes: spreadsheet_domain::sort::style_writes(&book, &range, &output.order)?,
            undoes: None,
            undoes_version: None,
        };
        let candidate = book.candidate(&operation, DocumentVersion::new(9)?)?;
        assert_eq!(
            candidate.cell_content(CellAddress {
                sheet: sheet.id(),
                row: sheet.rows()[1],
                column: sheet.columns()[0]
            })?,
            Some(&CellContent::Value(CellValue::Number(1.0)))
        );
        assert_eq!(
            candidate.cell_content(range.source)?,
            book.cell_content(range.source)?
        );
        let inverse = Operation::CellBatchWrite {
            intent: CellBatchIntent::SortRange {
                range: range.clone(),
                order: output.order,
            },
            content_writes: output
                .content_writes
                .into_iter()
                .map(|w| GuardedContentWrite {
                    address: w.address,
                    content: w.previous,
                    previous: w.content,
                })
                .collect(),
            style_writes: vec![],
            undoes: Some(OperationId::from_bytes([15; 16])?),
            undoes_version: Some(DocumentVersion::new(9)?),
        };
        let restored = candidate.candidate(&inverse, DocumentVersion::new(10)?)?;
        assert_eq!(restored.sheets()[0].cells(), book.sheets()[0].cells());
        Ok(())
    }

    #[test]
    fn empty_and_error_values_stay_last_and_equal_keys_keep_order_in_both_directions() {
        for descending in [false, true] {
            assert_eq!(
                compare(&Value::Blank, &Value::Number(1.0), descending),
                std::cmp::Ordering::Greater
            );
            assert_eq!(
                compare(
                    &Value::Text(String::new()),
                    &Value::Text("Я".into()),
                    descending
                ),
                std::cmp::Ordering::Greater
            );
            assert_eq!(
                compare(&Value::Number(-0.0), &Value::Number(0.0), descending),
                std::cmp::Ordering::Equal
            );
            let mut values = [
                (3, Value::Number(10.0)),
                (1, Value::Number(2.0)),
                (2, Value::Number(2.0)),
            ];
            values.sort_by(|a, b| compare(&a.1, &b.1, descending));
            assert_eq!(
                values.iter().map(|v| v.0).collect::<Vec<_>>(),
                if descending {
                    vec![3, 1, 2]
                } else {
                    vec![1, 2, 3]
                }
            );
        }
        assert_eq!(
            compare(&Value::Text("Ё".into()), &Value::Text("Я".into()), false),
            std::cmp::Ordering::Less
        );
    }
}
