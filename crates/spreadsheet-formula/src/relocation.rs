//! Frozen cut/move planning. No mutation, persistence, authorization or client ASTs.
use spreadsheet_domain::{
    formula::{
        AxisReferenceMode, BoundAst, BoundCellReference, Formula, FormulaValidationError, Node,
    },
    workbook::{
        CellAddress, CellContent, DocumentVersion, DomainError, GuardedContentWrite, Sheet,
        Workbook,
    },
};
use std::collections::BTreeMap;
use thiserror::Error;

const MAX_SOURCE_CELLS: u64 = 1000;
const MAX_WRITES: usize = 10_000;
const MAX_VISITED_NODES: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct MoveRequest {
    pub source: CellAddress,
    pub destination: CellAddress,
    pub rows: u32,
    pub columns: u32,
}

/// A caller must revalidate this input version/hash before preparing a candidate.
/// The footprint includes inbound references outside the moved rectangle.
pub struct MovePlan {
    pub input_version: DocumentVersion,
    pub input_hash: [u8; 32],
    pub writes: Vec<GuardedContentWrite>,
}

#[derive(Debug, Error)]
pub enum MoveError {
    #[error("move rectangle is invalid or outside allocated axes")]
    InvalidRectangle,
    #[error("cross-sheet moves are not supported")]
    CrossSheet,
    #[error("source and destination are identical")]
    NoChange,
    #[error("move intersects only part of a referenced range")]
    PartialRange,
    #[error("move exceeds planning budget")]
    LimitExceeded,
    #[error("move planning was cancelled")]
    Cancelled,
    #[error("invalid document address")]
    Domain(#[from] DomainError),
    #[error("invalid relocated formula")]
    Formula(#[from] FormulaValidationError),
}

#[derive(Clone, Copy)]
struct Rectangle {
    row: u32,
    column: u32,
    rows: u32,
    columns: u32,
}
impl Rectangle {
    fn at(sheet: &Sheet, address: CellAddress, rows: u32, columns: u32) -> Result<Self, MoveError> {
        let row = sheet
            .row_position(address.row)
            .ok_or(MoveError::InvalidRectangle)?;
        let column = sheet
            .column_position(address.column)
            .ok_or(MoveError::InvalidRectangle)?;
        if rows == 0
            || columns == 0
            || u64::from(row) + u64::from(rows) > sheet.rows().len() as u64
            || u64::from(column) + u64::from(columns) > sheet.columns().len() as u64
        {
            return Err(MoveError::InvalidRectangle);
        }
        Ok(Self {
            row,
            column,
            rows,
            columns,
        })
    }
    fn contains(self, row: u32, column: u32) -> bool {
        row >= self.row
            && row < self.row + self.rows
            && column >= self.column
            && column < self.column + self.columns
    }
}

fn sheet_for(book: &Workbook, address: CellAddress) -> Result<&Sheet, MoveError> {
    book.sheets()
        .iter()
        .find(|s| s.id() == address.sheet)
        .ok_or(MoveError::InvalidRectangle)
}
fn address_at(sheet: &Sheet, row: u32, column: u32) -> Result<CellAddress, MoveError> {
    Ok(CellAddress {
        sheet: sheet.id(),
        row: *sheet
            .rows()
            .get(row as usize)
            .ok_or(MoveError::InvalidRectangle)?,
        column: *sheet
            .columns()
            .get(column as usize)
            .ok_or(MoveError::InvalidRectangle)?,
    })
}
fn target(reference: BoundCellReference) -> CellAddress {
    CellAddress {
        sheet: reference.target_sheet,
        row: reference.target_row,
        column: reference.target_column,
    }
}
fn relocate_reference(
    book: &Workbook,
    reference: &mut BoundCellReference,
    origin: CellAddress,
    mapping: &BTreeMap<CellAddress, CellAddress>,
) -> Result<(), MoveError> {
    if let Some(destination) = mapping.get(&target(*reference)) {
        reference.target_sheet = destination.sheet;
        reference.target_row = destination.row;
        reference.target_column = destination.column;
    }
    let origin_sheet = sheet_for(book, origin)?;
    let target_sheet = sheet_for(book, target(*reference))?;
    if matches!(reference.row_mode, AxisReferenceMode::Relative { .. }) {
        reference.row_mode = AxisReferenceMode::Relative {
            original_delta: i64::from(
                target_sheet
                    .row_position(reference.target_row)
                    .ok_or(MoveError::InvalidRectangle)?,
            ) - i64::from(
                origin_sheet
                    .row_position(origin.row)
                    .ok_or(MoveError::InvalidRectangle)?,
            ),
        };
    }
    if matches!(reference.column_mode, AxisReferenceMode::Relative { .. }) {
        reference.column_mode = AxisReferenceMode::Relative {
            original_delta: i64::from(
                target_sheet
                    .column_position(reference.target_column)
                    .ok_or(MoveError::InvalidRectangle)?,
            ) - i64::from(
                origin_sheet
                    .column_position(origin.column)
                    .ok_or(MoveError::InvalidRectangle)?,
            ),
        };
    }
    Ok(())
}

fn inspect_formula(
    formula: &Formula,
    source: Rectangle,
    source_sheet: &Sheet,
    mapping: &BTreeMap<CellAddress, CellAddress>,
    visited: &mut usize,
) -> Result<bool, MoveError> {
    *visited = visited
        .checked_add(formula.ast().nodes().len())
        .ok_or(MoveError::LimitExceeded)?;
    if *visited > MAX_VISITED_NODES {
        return Err(MoveError::LimitExceeded);
    }
    let mut affected = false;
    for node in formula.ast().nodes() {
        match node {
            Node::Cell(reference) => affected |= mapping.contains_key(&target(*reference)),
            Node::Range(range) if range.start.target_sheet == source_sheet.id() => {
                let r1 = source_sheet
                    .row_position(range.start.target_row)
                    .ok_or(MoveError::InvalidRectangle)?;
                let r2 = source_sheet
                    .row_position(range.end.target_row)
                    .ok_or(MoveError::InvalidRectangle)?;
                let c1 = source_sheet
                    .column_position(range.start.target_column)
                    .ok_or(MoveError::InvalidRectangle)?;
                let c2 = source_sheet
                    .column_position(range.end.target_column)
                    .ok_or(MoveError::InvalidRectangle)?;
                let intersects = r1.min(r2) < source.row + source.rows
                    && r1.max(r2) >= source.row
                    && c1.min(c2) < source.column + source.columns
                    && c1.max(c2) >= source.column;
                if intersects && !(source.contains(r1, c1) && source.contains(r2, c2)) {
                    return Err(MoveError::PartialRange);
                }
                affected |= intersects;
            }
            _ => {}
        }
    }
    Ok(affected)
}

fn relocate_formula(
    book: &Workbook,
    formula: &Formula,
    origin: CellAddress,
    mapping: &BTreeMap<CellAddress, CellAddress>,
) -> Result<Formula, MoveError> {
    let mut nodes = formula.ast().nodes().to_vec();
    for node in &mut nodes {
        match node {
            Node::Cell(reference) => relocate_reference(book, reference, origin, mapping)?,
            Node::Range(range) => {
                relocate_reference(book, &mut range.start, origin, mapping)?;
                relocate_reference(book, &mut range.end, origin, mapping)?;
            }
            _ => {}
        }
    }
    Ok(Formula::new(
        formula.source().to_owned(),
        formula.canonical().to_owned(),
        BoundAst::new(formula.ast().schema_version(), formula.ast().root(), nodes)?,
        origin,
        book.version(),
    )?)
}

/// Moves content simultaneously, preserving formula targets and following cut
/// targets in every sheet. Partial range intersections reject the complete plan.
pub fn plan_move(book: &Workbook, request: MoveRequest) -> Result<MovePlan, MoveError> {
    plan_move_cancellable(book, request, || false)
}

/// Cancellation is checked before work, per source row and per visited cell.
/// The callback must be inexpensive; no partial result escapes on cancellation.
pub fn plan_move_cancellable(
    book: &Workbook,
    request: MoveRequest,
    mut cancelled: impl FnMut() -> bool,
) -> Result<MovePlan, MoveError> {
    if cancelled() {
        return Err(MoveError::Cancelled);
    }
    if request.source.sheet != request.destination.sheet {
        return Err(MoveError::CrossSheet);
    }
    if request.source == request.destination {
        return Err(MoveError::NoChange);
    }
    if u64::from(request.rows) * u64::from(request.columns) > MAX_SOURCE_CELLS {
        return Err(MoveError::LimitExceeded);
    }
    let sheet = sheet_for(book, request.source)?;
    let source = Rectangle::at(sheet, request.source, request.rows, request.columns)?;
    let destination = Rectangle::at(sheet, request.destination, request.rows, request.columns)?;
    let mut mapping = BTreeMap::new();
    let mut output = BTreeMap::new();
    for row in 0..request.rows {
        if cancelled() {
            return Err(MoveError::Cancelled);
        }
        for column in 0..request.columns {
            let from = address_at(sheet, source.row + row, source.column + column)?;
            let to = address_at(sheet, destination.row + row, destination.column + column)?;
            mapping.insert(from, to);
            output.insert(from, None);
        }
    }
    let mut visited = 0;
    for (&from, &to) in &mapping {
        if cancelled() {
            return Err(MoveError::Cancelled);
        }
        let content = match book.cell_content(from)? {
            Some(CellContent::Formula(formula)) => {
                inspect_formula(formula, source, sheet, &mapping, &mut visited)?;
                Some(CellContent::Formula(relocate_formula(
                    book, formula, to, &mapping,
                )?))
            }
            other => other.cloned(),
        };
        output.insert(to, content);
    }
    for other_sheet in book.sheets() {
        for (&address, content) in other_sheet.cells() {
            if cancelled() {
                return Err(MoveError::Cancelled);
            }
            // Both source and overwritten destinations already have frozen writes.
            if output.contains_key(&address) {
                continue;
            }
            if let CellContent::Formula(formula) = content
                && inspect_formula(formula, source, sheet, &mapping, &mut visited)?
            {
                let relocated = relocate_formula(book, formula, address, &mapping)?;
                output.insert(address, Some(CellContent::Formula(relocated)));
                if output.len() > MAX_WRITES {
                    return Err(MoveError::LimitExceeded);
                }
            }
        }
    }
    let writes = output
        .into_iter()
        .map(|(address, content)| {
            Ok(GuardedContentWrite {
                address,
                content,
                previous: book.cell_content(address)?.cloned(),
            })
        })
        .collect::<Result<Vec<_>, MoveError>>()?;
    Ok(MovePlan {
        input_version: book.version(),
        input_hash: book.hash(),
        writes,
    })
}
