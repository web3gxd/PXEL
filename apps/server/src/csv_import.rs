//! Bounded CSV decoding and explicit typed conversion, without implicit formulas.

use crate::pxel_file::{Cell, Content};
use serde::{Deserialize, Serialize};
use spreadsheet_domain::workbook::{MAX_COLUMNS, MAX_ROWS, MAX_TEXT_BYTES};

pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FIELDS: usize = 100_000;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum CsvError {
    #[error("invalid CSV syntax")]
    Syntax,
    #[error("CSV limit exceeded")]
    Limit,
    #[error("CSV column type mismatch")]
    Type,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    Text,
    Number,
    Boolean,
}

fn finish_field(row: &mut Vec<String>, field: &mut String) -> Result<(), CsvError> {
    if row.len() >= MAX_COLUMNS {
        return Err(CsvError::Limit);
    }
    row.push(std::mem::take(field));
    Ok(())
}

fn finish_row(rows: &mut Vec<Vec<String>>, row: &mut Vec<String>) -> Result<(), CsvError> {
    if rows.first().is_some_and(|first| first.len() != row.len()) {
        return Err(CsvError::Syntax);
    }
    if rows.len() >= MAX_ROWS || (rows.len() + 1).saturating_mul(row.len()) > MAX_FIELDS {
        return Err(CsvError::Limit);
    }
    rows.push(std::mem::take(row));
    Ok(())
}

pub fn parse(source: &str, delimiter: u8) -> Result<Vec<Vec<String>>, CsvError> {
    if !matches!(delimiter, b',' | b';' | b'\t') || source.is_empty() {
        return Err(CsvError::Syntax);
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(CsvError::Limit);
    }
    let mut chars = source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .chars()
        .peekable();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut closed = false;
    let mut touched = false;
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            return Err(CsvError::Syntax);
        }
        touched = true;
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    closed = true;
                }
            } else {
                field.push(ch);
            }
        } else if ch == char::from(delimiter) {
            finish_field(&mut row, &mut field)?;
            closed = false;
        } else if ch == '\r' || ch == '\n' {
            if ch == '\r' && chars.next() != Some('\n') {
                return Err(CsvError::Syntax);
            }
            finish_field(&mut row, &mut field)?;
            finish_row(&mut rows, &mut row)?;
            closed = false;
            touched = false;
        } else if ch == '"' && field.is_empty() && !closed {
            quoted = true;
        } else if ch == '"' || closed {
            return Err(CsvError::Syntax);
        } else {
            field.push(ch);
        }
        if field.len() > MAX_TEXT_BYTES {
            return Err(CsvError::Limit);
        }
    }
    if quoted {
        return Err(CsvError::Syntax);
    }
    if touched || !row.is_empty() {
        finish_field(&mut row, &mut field)?;
        finish_row(&mut rows, &mut row)?;
    }
    if rows.is_empty() {
        return Err(CsvError::Syntax);
    }
    Ok(rows)
}

pub fn convert(
    rows: &[Vec<String>],
    types: &[ColumnType],
    header: bool,
) -> Result<Vec<Cell>, CsvError> {
    let width = rows.first().ok_or(CsvError::Syntax)?.len();
    if types.len() != width || rows.iter().any(|row| row.len() != width) {
        return Err(CsvError::Type);
    }
    let mut cells = Vec::new();
    for (row, values) in rows.iter().enumerate() {
        for (column, (value, kind)) in values.iter().zip(types).enumerate() {
            if value.is_empty() {
                continue;
            }
            let content = if row == 0 && header || matches!(kind, ColumnType::Text) {
                Content::Text(value.clone())
            } else {
                match kind {
                    ColumnType::Text => Content::Text(value.clone()),
                    ColumnType::Number => {
                        let value = value.trim().parse::<f64>().map_err(|_| CsvError::Type)?;
                        if !value.is_finite() {
                            return Err(CsvError::Type);
                        }
                        Content::Number(value)
                    }
                    ColumnType::Boolean if value.eq_ignore_ascii_case("true") => {
                        Content::Boolean(true)
                    }
                    ColumnType::Boolean if value.eq_ignore_ascii_case("false") => {
                        Content::Boolean(false)
                    }
                    ColumnType::Boolean => return Err(CsvError::Type),
                }
            };
            cells.push(Cell {
                row: row as u32,
                column: column as u32,
                content,
            });
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_unicode_newlines_escapes_bom_and_trailing_fields() -> Result<(), CsvError> {
        let rows = parse(
            "\u{feff}Имя;Текст;Пусто\r\n001;\"a;\"\"b\"\"\r\nc\";\r\n",
            b';',
        )?;
        assert_eq!(
            rows,
            vec![
                vec!["Имя", "Текст", "Пусто"],
                vec!["001", "a;\"b\"\r\nc", ""]
            ]
        );
        assert_eq!(
            parse("a,b\n1,2", b',')?,
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
        Ok(())
    }

    #[test]
    fn malformed_quote_ragged_null_empty_and_limits_reject_atomically() {
        for source in ["", "a,\"b", "a,b\"c", "\"a\"x,b", "a,b\n1", "a\0,b"] {
            assert!(parse(source, b',').is_err());
        }
        assert!(parse("x", b'|').is_err());
        assert!(parse(&"x".repeat(32 * 1024 + 1), b',').is_err());
        assert!(parse(&vec!["x"; 1001].join(","), b',').is_err());
    }

    #[test]
    fn explicit_types_preserve_header_identifiers_and_formula_like_text() -> Result<(), CsvError> {
        let rows = parse("Код,Сумма,Флаг\n001,12.50,true\n=SUM(A1),-3,false", b',')?;
        let cells = convert(
            &rows,
            &[ColumnType::Text, ColumnType::Number, ColumnType::Boolean],
            true,
        )?;
        assert!(matches!(&cells[0].content, crate::pxel_file::Content::Text(v) if v == "Код"));
        assert!(matches!(&cells[3].content, crate::pxel_file::Content::Text(v) if v == "001"));
        assert!(matches!(cells[4].content, crate::pxel_file::Content::Number(v) if v == 12.5));
        assert!(matches!(
            cells[5].content,
            crate::pxel_file::Content::Boolean(true)
        ));
        assert!(matches!(&cells[6].content, crate::pxel_file::Content::Text(v) if v == "=SUM(A1)"));
        assert!(convert(&rows, &[ColumnType::Number; 3], true).is_err());
        Ok(())
    }

    #[test]
    fn invalid_numeric_boolean_or_type_count_refuses_entire_conversion() -> Result<(), CsvError> {
        for source in ["NaN", "inf", "1e999", "12x"] {
            assert!(convert(&parse(source, b',')?, &[ColumnType::Number], false).is_err());
        }
        assert!(convert(&parse("yes", b',')?, &[ColumnType::Boolean], false).is_err());
        assert!(convert(&parse("1,2", b',')?, &[ColumnType::Number], false).is_err());
        Ok(())
    }

    #[test]
    fn field_and_source_budgets_include_empty_fields() -> Result<(), CsvError> {
        let row = format!("{}\n", ",".repeat(999));
        let rows = parse(&row.repeat(100), b',')?;
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[0].len(), 1000);
        assert_eq!(parse(&row.repeat(101), b','), Err(CsvError::Limit));
        assert_eq!(
            parse(&"x".repeat(MAX_SOURCE_BYTES + 1), b','),
            Err(CsvError::Limit)
        );
        Ok(())
    }
}
