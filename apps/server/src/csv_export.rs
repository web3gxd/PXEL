//! Selected-sheet CSV values are separate from authored formulas.

use spreadsheet_domain::workbook::{CellAddress, CellContent, CellValue, SheetId, Workbook};
use spreadsheet_formula::{
    Value,
    calculation::{CalculationLimits, CalculationProjection},
};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

#[derive(Debug, thiserror::Error)]
pub enum CsvExportError {
    #[error("CSV export limit exceeded")]
    Limit,
    #[error("CSV export input is invalid")]
    Invalid,
    #[error("CSV export calculation failed")]
    Calculation,
}

fn field(output: &mut String, value: &CellValue, safe_text: bool) -> Result<(), CsvExportError> {
    let (text, literal) = match value {
        CellValue::Blank => (String::new(), false),
        CellValue::Number(n) if n.is_finite() => (n.to_string(), false),
        CellValue::Number(_) => return Err(CsvExportError::Invalid),
        CellValue::Boolean(b) => (if *b { "TRUE" } else { "FALSE" }.into(), false),
        CellValue::Text(t) => (t.clone(), true),
    };
    if text.contains('\0') {
        return Err(CsvExportError::Invalid);
    }
    output.push('"');
    let dangerous = text.starts_with(['\t', '\r', '\n'])
        || text
            .trim_start()
            .starts_with(['=', '+', '-', '@', '＝', '＋', '－', '＠']);
    if safe_text && literal && dangerous {
        output.push('\'');
    }
    for ch in text.chars() {
        if ch == '"' {
            output.push('"');
        }
        output.push(ch);
    }
    output.push('"');
    if output.len() > crate::csv_import::MAX_SOURCE_BYTES {
        return Err(CsvExportError::Limit);
    }
    Ok(())
}

pub fn encode(
    book: &Workbook,
    sheet_id: SheetId,
    delimiter: u8,
    safe_text: bool,
    safe_values: Option<&BTreeMap<CellAddress, CellValue>>,
) -> Result<Vec<u8>, CsvExportError> {
    if !matches!(delimiter, b',' | b';' | b'\t') {
        return Err(CsvExportError::Invalid);
    }
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == sheet_id)
        .ok_or(CsvExportError::Invalid)?;
    let mut rows = 1usize;
    let mut columns = 1usize;
    let mut positions = BTreeMap::new();
    for address in sheet.cells().keys() {
        let row = sheet
            .row_position(address.row)
            .ok_or(CsvExportError::Invalid)? as usize;
        let column = sheet
            .column_position(address.column)
            .ok_or(CsvExportError::Invalid)? as usize;
        rows = rows.max(row + 1);
        columns = columns.max(column + 1);
        positions.insert((row, column), address);
    }
    if rows.saturating_mul(columns) > 100_000 {
        return Err(CsvExportError::Limit);
    }
    let limits = CalculationLimits::default();
    let calculated = if safe_values.is_none() {
        let mut calc = CalculationProjection::from_workbook(book, limits)
            .map_err(|_| CsvExportError::Calculation)?;
        calc.calculate(book, limits, &AtomicBool::new(false))
            .map_err(|_| CsvExportError::Calculation)?;
        Some(calc)
    } else {
        None
    };
    let mut output = String::from("\u{feff}");
    for row in 0..rows {
        for column in 0..columns {
            if column > 0 {
                output.push(char::from(delimiter));
            }
            let value = if let Some(address) = positions.get(&(row, column)) {
                if let Some(safe) = safe_values {
                    safe.get(address).cloned().ok_or(CsvExportError::Invalid)?
                } else {
                    match sheet.cells().get(address).ok_or(CsvExportError::Invalid)? {
                        CellContent::Value(value) => value.clone(),
                        CellContent::Formula(_) => match calculated
                            .as_ref()
                            .and_then(|c| c.results().get(address))
                            .ok_or(CsvExportError::Calculation)?
                        {
                            Value::Blank => CellValue::Blank,
                            Value::Number(n) | Value::DateTime { serial: n, .. } => {
                                CellValue::Number(*n)
                            }
                            Value::Text(t) => CellValue::Text(t.clone()),
                            Value::Boolean(b) => CellValue::Boolean(*b),
                            Value::Error(e) => CellValue::Text(e.code().into()),
                            Value::Array { .. } => return Err(CsvExportError::Calculation),
                        },
                    }
                }
            } else {
                CellValue::Blank
            };
            field(&mut output, &value, safe_text)?;
        }
        output.push_str("\r\n");
        if output.len() > crate::csv_import::MAX_SOURCE_BYTES {
            return Err(CsvExportError::Limit);
        }
    }
    Ok(output.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pxel_file::{Book, Cell, Content, Settings, Sheet};
    use spreadsheet_domain::workbook::{ActorId, DocumentId, Workbook};
    use uuid::Uuid;

    fn workbook(cells: Vec<Cell>) -> Result<Workbook, Box<dyn std::error::Error>> {
        let columns = cells
            .iter()
            .map(|c| c.column + 1)
            .max()
            .unwrap_or(26)
            .max(26);
        let file = Book {
            schema_version: 1,
            engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
            compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
            title: "CSV".into(),
            anonymized: false,
            settings: Settings::default(),
            sheets: vec![Sheet {
                name: "Data".into(),
                rows: 1000,
                columns,
                frozen_rows: 0,
                view: None,
                cells,
                styles: vec![],
                row_heights: vec![],
                column_widths: vec![],
                merges: vec![],
            }],
        };
        let doc = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        Ok(crate::pxel_workbook::prepare(&file, doc, actor, Uuid::now_v7(), &[3; 32])?.book)
    }
    #[test]
    fn utf8_quoting_sparse_positions_and_safe_text_roundtrip()
    -> Result<(), Box<dyn std::error::Error>> {
        let book = workbook(vec![
            Cell {
                row: 0,
                column: 0,
                content: Content::Text("001".into()),
            },
            Cell {
                row: 0,
                column: 1,
                content: Content::Text("a,\"b\"\nЯ".into()),
            },
            Cell {
                row: 1,
                column: 2,
                content: Content::Text("=SUM(A1)".into()),
            },
            Cell {
                row: 1,
                column: 0,
                content: Content::Number(-3.5),
            },
        ])?;
        let id = book.sheets()[0].id();
        let bytes = encode(&book, id, b',', true, None)?;
        let rows = crate::csv_import::parse(std::str::from_utf8(&bytes)?, b',')?;
        assert_eq!(
            rows,
            vec![vec!["001", "a,\"b\"\nЯ", ""], vec!["-3.5", "", "'=SUM(A1)"]]
        );
        let exact = encode(&book, id, b';', false, None)?;
        assert_eq!(
            crate::csv_import::parse(std::str::from_utf8(&exact)?, b';')?[1][2],
            "=SUM(A1)"
        );
        assert!(encode(&book, id, b'|', true, None).is_err());
        Ok(())
    }
    #[test]
    fn volatile_formula_without_durable_context_refuses_download()
    -> Result<(), Box<dyn std::error::Error>> {
        let book = workbook(vec![Cell {
            row: 0,
            column: 0,
            content: Content::Formula("=RAND()".into()),
        }])?;
        assert!(book.calculation_context().is_none());
        assert!(encode(&book, book.sheets()[0].id(), b',', true, None).is_err());
        Ok(())
    }
    #[test]
    fn formula_values_errors_and_boolean_keep_confirmed_book_unchanged()
    -> Result<(), Box<dyn std::error::Error>> {
        let book = workbook(vec![
            Cell {
                row: 0,
                column: 0,
                content: Content::Number(3.),
            },
            Cell {
                row: 0,
                column: 1,
                content: Content::Formula("=A1*2".into()),
            },
            Cell {
                row: 1,
                column: 0,
                content: Content::Boolean(false),
            },
            Cell {
                row: 1,
                column: 1,
                content: Content::Formula("=1/0".into()),
            },
        ])?;
        let hash = book.hash();
        let bytes = encode(&book, book.sheets()[0].id(), b',', true, None)?;
        assert_eq!(
            crate::csv_import::parse(std::str::from_utf8(&bytes)?, b',')?,
            vec![vec!["3", "6"], vec!["FALSE", "#DIV/0!"]]
        );
        assert_eq!(book.hash(), hash);
        Ok(())
    }
    #[test]
    fn encoded_quotation_expansion_and_null_are_bounded() {
        let mut output = "x".repeat(crate::csv_import::MAX_SOURCE_BYTES - 2);
        assert!(field(&mut output, &CellValue::Text("\"".into()), true).is_err());
        assert!(field(&mut String::new(), &CellValue::Text("x\0".into()), false).is_err());
    }
    #[test]
    fn empty_sheet_has_one_field_and_sparse_rectangle_budget_is_enforced()
    -> Result<(), Box<dyn std::error::Error>> {
        let empty = workbook(vec![])?;
        assert_eq!(
            encode(&empty, empty.sheets()[0].id(), b'\t', true, None)?,
            b"\xef\xbb\xbf\"\"\r\n"
        );
        let wide = workbook(vec![Cell {
            row: 999,
            column: 25,
            content: Content::Text("x".into()),
        }])?;
        assert!(encode(&wide, wide.sheets()[0].id(), b',', true, None).is_ok());
        let outside = workbook(vec![Cell {
            row: 999,
            column: 200,
            content: Content::Text("x".into()),
        }])?;
        assert!(encode(&outside, outside.sheets()[0].id(), b',', true, None).is_err());
        Ok(())
    }
}
