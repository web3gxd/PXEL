//! Read-only syntax and series diagnostics; no clocks, IO or calculation mutation.
use crate::{CellReference, Node, ParseError, Span, parse};
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::CellAddress;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenClass {
    Reference,
    Number,
    Text,
    Boolean,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxToken {
    pub span: Span,
    pub class: TokenClass,
}
pub struct Syntax {
    pub tokens: Vec<SyntaxToken>,
    pub pattern: [u8; 32],
    pub constants: [u8; 32],
    pub literal_count: usize,
    pub nodes: usize,
}
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DiagnosticsError {
    #[error("diagnostics budget exceeded")]
    LimitExceeded,
    #[error("invalid diagnostic formula")]
    Invalid,
}
pub struct SeriesFormula<'a> {
    pub address: CellAddress,
    pub row: u32,
    pub column: u32,
    pub source: &'a str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardcodeWarning {
    pub address: CellAddress,
    pub fingerprint: [u8; 32],
    pub by_row: bool,
    pub formulas: usize,
    pub dominant: usize,
}

pub fn syntax(source: &str, origin: (u32, u32)) -> Result<Syntax, ParseError> {
    let parsed = parse(source)?;
    let mut pattern = Sha256::new();
    let mut constants = Sha256::new();
    let mut tokens = Vec::new();
    let mut literal_count = 0;
    for (index, node) in parsed.ast.nodes.iter().enumerate() {
        let class = match node {
            Node::Number(value) => {
                pattern.update([0]);
                pattern.update(value.to_bits().to_le_bytes());
                constants.update([0]);
                constants.update(value.to_bits().to_le_bytes());
                literal_count += 1;
                Some(TokenClass::Number)
            }
            Node::Text(value) => {
                pattern.update([1]);
                hashed_text(&mut pattern, value);
                constants.update([1]);
                hashed_text(&mut constants, value);
                literal_count += 1;
                Some(TokenClass::Text)
            }
            Node::Boolean(value) => {
                pattern.update([2, u8::from(*value)]);
                constants.update([2, u8::from(*value)]);
                literal_count += 1;
                Some(TokenClass::Boolean)
            }
            Node::Cell(reference) => {
                pattern.update([3]);
                hashed_reference(&mut pattern, reference, origin);
                Some(TokenClass::Reference)
            }
            Node::Range(range) => {
                pattern.update([4]);
                hashed_reference(&mut pattern, &range.start, origin);
                hashed_reference(&mut pattern, &range.end, origin);
                Some(TokenClass::Reference)
            }
            Node::Unary { op, operand } => {
                pattern.update([5]);
                hashed_text(&mut pattern, &format!("{op:?}"));
                pattern.update((operand.0 as u64).to_le_bytes());
                None
            }
            Node::Binary { op, left, right } => {
                pattern.update([6]);
                hashed_text(&mut pattern, &format!("{op:?}"));
                pattern.update((left.0 as u64).to_le_bytes());
                pattern.update((right.0 as u64).to_le_bytes());
                None
            }
            Node::Call { name, arguments } => {
                pattern.update([7]);
                hashed_text(&mut pattern, name);
                pattern.update((arguments.len() as u64).to_le_bytes());
                for argument in arguments {
                    pattern.update((argument.0 as u64).to_le_bytes());
                }
                None
            }
            Node::Missing => {
                pattern.update([8]);
                None
            }
            Node::Error(error) => {
                pattern.update([9]);
                hashed_text(&mut pattern, &format!("{error:?}"));
                None
            }
            Node::InvalidReference => {
                pattern.update([10]);
                None
            }
        };
        if let Some(class) = class
            && let Some(span) = parsed.spans.get(index)
        {
            tokens.push(SyntaxToken { span: *span, class });
        }
    }
    tokens.sort_by_key(|token| token.span.start);
    Ok(Syntax {
        tokens,
        pattern: pattern.finalize().into(),
        constants: constants.finalize().into(),
        literal_count,
        nodes: parsed.ast.nodes.len(),
    })
}
fn hashed_text(hash: &mut Sha256, text: &str) {
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
}
fn hashed_reference(hash: &mut Sha256, reference: &CellReference, origin: (u32, u32)) {
    hash.update([u8::from(reference.sheet.is_some())]);
    if let Some(sheet) = &reference.sheet {
        hashed_text(hash, sheet);
    }
    for (position, absolute, base) in [
        (reference.row, reference.row_absolute, origin.0),
        (reference.column, reference.column_absolute, origin.1),
    ] {
        hash.update([u8::from(absolute)]);
        hash.update(
            (i64::from(position) - if absolute { 0 } else { i64::from(base) }).to_le_bytes(),
        );
    }
}

pub fn analyze(formulas: &[SeriesFormula<'_>]) -> Result<Vec<HardcodeWarning>, DiagnosticsError> {
    if formulas.len() > 100_000 {
        return Err(DiagnosticsError::LimitExceeded);
    }
    let mut remaining_nodes = 2_000_000usize;
    let mut remaining_bytes = 16 * 1024 * 1024usize;
    let mut parsed = Vec::with_capacity(formulas.len());
    let mut groups = BTreeMap::new();
    for (index, formula) in formulas.iter().enumerate() {
        remaining_bytes = remaining_bytes
            .checked_sub(formula.source.len())
            .ok_or(DiagnosticsError::LimitExceeded)?;
        let value = syntax(formula.source, (formula.row, formula.column))
            .map_err(|_| DiagnosticsError::Invalid)?;
        remaining_nodes = remaining_nodes
            .checked_sub(value.nodes)
            .ok_or(DiagnosticsError::LimitExceeded)?;
        parsed.push(value);
        for (by_row, position) in [(true, formula.row), (false, formula.column)] {
            groups
                .entry((formula.address.sheet, by_row, position))
                .or_insert_with(Vec::new)
                .push(index);
        }
    }
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for ((_, by_row, _), indices) in groups {
        if indices.len() < 100 {
            continue;
        }
        let mut patterns = BTreeMap::new();
        for index in &indices {
            patterns
                .entry(parsed[*index].pattern)
                .or_insert_with(Vec::new)
                .push(*index);
        }
        let Some((dominant_pattern, dominant_indices)) =
            patterns.iter().max_by_key(|(_, indices)| indices.len())
        else {
            continue;
        };
        if dominant_indices.len() * 100 < indices.len() * 90 {
            continue;
        }
        let dominant_constants = parsed[dominant_indices[0]].constants;
        for index in &indices {
            let value = &parsed[*index];
            if &value.pattern == dominant_pattern
                || value.literal_count == 0
                || value.constants == dominant_constants
            {
                continue;
            }
            let address = formulas[*index].address;
            if !seen.insert(address) {
                continue;
            }
            let mut hash = Sha256::new();
            hash.update(b"PXEL-HARDCODE-1");
            hash.update(address.sheet.as_bytes());
            hash.update(address.row.as_bytes());
            hash.update(address.column.as_bytes());
            hash.update(value.pattern);
            warnings.push(HardcodeWarning {
                address,
                fingerprint: hash.finalize().into(),
                by_row,
                formulas: indices.len(),
                dominant: dominant_indices.len(),
            });
            if warnings.len() > 1000 {
                return Err(DiagnosticsError::LimitExceeded);
            }
        }
    }
    warnings.sort_by_key(|warning| warning.address);
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::{ColumnId, RowId, SheetId};
    #[test]
    fn unicode_spans_and_relative_mixed_references_share_patterns()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "=IF(TRUE,'Лист😀'!A1+2,\"Привет😀\")";
        let first = syntax(source, (0, 1))?;
        let second = syntax("=IF(TRUE,'Лист😀'!A2+2,\"Привет😀\")", (1, 1))?;
        assert_eq!(first.pattern, second.pattern);
        let tokens: Vec<_> = first
            .tokens
            .iter()
            .map(|token| source.get(token.span.start..token.span.end))
            .collect();
        assert_eq!(
            tokens,
            [
                Some("TRUE"),
                Some("'Лист😀'!A1"),
                Some("2"),
                Some("\"Привет😀\"")
            ]
        );
        assert_eq!(
            syntax("=$A1+A$1", (0, 1))?.pattern,
            syntax("=$A2+B$1", (1, 2))?.pattern
        );
        assert_ne!(
            syntax("=$A$1+2", (0, 1))?.pattern,
            syntax("=$A$2+2", (1, 1))?.pattern
        );
        assert!(syntax("=1+", (0, 0)).is_err());
        Ok(())
    }
    #[test]
    fn series_thresholds_and_uniform_constants_are_exact() -> Result<(), Box<dyn std::error::Error>>
    {
        let sheet = SheetId::from_bytes([1; 16])?;
        let column = ColumnId::from_bytes([2; 16])?;
        let sources: Vec<_> = (0..100)
            .map(|row| format!("=A{}*{}", row + 1, if row == 99 { 3 } else { 2 }))
            .collect();
        let formulas: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(row, source)| {
                let mut id = [3; 16];
                id[..4].copy_from_slice(&(row as u32).to_le_bytes());
                Ok(SeriesFormula {
                    address: CellAddress {
                        sheet,
                        row: RowId::from_bytes(id)?,
                        column,
                    },
                    row: row as u32,
                    column: 1,
                    source,
                })
            })
            .collect::<Result<_, spreadsheet_domain::workbook::DomainError>>()?;
        assert!(analyze(&formulas[..99])?.is_empty());
        let warnings = analyze(&formulas)?;
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].dominant, 99);
        let uniform: Vec<_> = formulas
            .iter()
            .map(|formula| SeriesFormula {
                source: "=2",
                ..*formula
            })
            .collect();
        assert!(analyze(&uniform)?.is_empty());
        let boundary: Vec<_> = formulas
            .iter()
            .enumerate()
            .map(|(index, formula)| SeriesFormula {
                source: if index < 90 { "=2" } else { "=3" },
                ..*formula
            })
            .collect();
        assert_eq!(analyze(&boundary)?.len(), 10);
        let below: Vec<_> = formulas
            .iter()
            .enumerate()
            .map(|(index, formula)| SeriesFormula {
                source: if index < 89 { "=2" } else { "=3" },
                ..*formula
            })
            .collect();
        assert!(analyze(&below)?.is_empty());
        Ok(())
    }
}
