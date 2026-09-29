//! Formula-copy text transformation. The server still binds the result to
//! stable IDs; these A1 coordinates are only the user's copy intent.
use crate::{
    ast::{CellReference, Node, ParseError, ParseErrorCode},
    canonical, parse,
};

const MAX_ROW: i64 = 1_048_576;
const MAX_COLUMN: i64 = 16_384;

fn shift_axis(value: u32, from: u32, to: u32, absolute: bool, limit: i64) -> Option<u32> {
    if absolute {
        return Some(value);
    }
    let shifted = i64::from(value).checked_add(i64::from(to) - i64::from(from))?;
    if !(0..limit).contains(&shifted) {
        return None;
    }
    u32::try_from(shifted).ok()
}

fn shift_reference(reference: &mut CellReference, from: (u32, u32), to: (u32, u32)) -> bool {
    let Some(row) = shift_axis(reference.row, from.0, to.0, reference.row_absolute, MAX_ROW) else {
        return false;
    };
    let Some(column) = shift_axis(
        reference.column,
        from.1,
        to.1,
        reference.column_absolute,
        MAX_COLUMN,
    ) else {
        return false;
    };
    reference.row = row;
    reference.column = column;
    true
}

/// Return canonical English formula text copied from one zero-based A1
/// position to another. A relative reference outside Excel's address space
/// becomes `#REF!`; absolute axes retain their coordinate. Callers must use
/// the source formula's current displayed text after structural changes.
pub fn copy_formula_text(
    source: &str,
    from: (u32, u32),
    to: (u32, u32),
) -> Result<String, ParseError> {
    if [from.0, to.0].iter().any(|&row| i64::from(row) >= MAX_ROW)
        || [from.1, to.1]
            .iter()
            .any(|&column| i64::from(column) >= MAX_COLUMN)
    {
        return Err(ParseError::at(
            ParseErrorCode::InvalidReference,
            0,
            source.len(),
        ));
    }
    let mut diagnostic = parse(source)?;
    for node in &mut diagnostic.ast.nodes {
        match node {
            Node::Cell(reference) => {
                if !shift_reference(reference, from, to) {
                    *node = Node::InvalidReference;
                }
            }
            Node::Range(range) => {
                if !shift_reference(&mut range.start, from, to)
                    || !shift_reference(&mut range.end, from, to)
                {
                    *node = Node::InvalidReference;
                }
            }
            _ => {}
        }
    }
    canonical(&diagnostic.ast)
}
