//! Frozen inverse planning; durable eligibility and publication remain application work.
use spreadsheet_domain::formula::{BoundCellReference, Formula, Node};
use spreadsheet_domain::workbook::*;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_TAIL_EVENTS: usize = 10_000;
const MAX_GUARD_WORK: usize = 1_000_000;

pub struct MoveInversePlan {
    pub input_version: DocumentVersion,
    pub input_hash: [u8; 32],
    pub target_operation_id: OperationId,
    pub target_version: DocumentVersion,
    pub writes: Vec<GuardedContentWrite>,
}

#[derive(Debug, Error)]
pub enum MoveInverseError {
    #[error("event is not an eligible move")]
    NotUndoable,
    #[error("move inverse conflicts with subsequent document changes")]
    Conflict,
    #[error("history is not a contiguous tail for this projection")]
    InvalidHistory,
    #[error("move inverse exceeds its history or inspection budget")]
    LimitExceeded,
    #[error("move inverse planning was cancelled")]
    Cancelled,
    #[error("invalid document state")]
    Domain(#[from] DomainError),
}

/// Plans an immutable inverse from a verified durable target and contiguous tail.
/// See [`plan_move_inverse_cancellable`] for admission responsibilities and limits.
pub fn plan_move_inverse(
    book: &Workbook,
    target: &CommittedEvent,
    tail: &[CommittedEvent],
) -> Result<MoveInversePlan, MoveInverseError> {
    plan_move_inverse_cancellable(book, target, tail, || false)
}

/// Requires a verified durable target and tail. This is conservative planning,
/// not authorization or commit admission. Any later content touch of the inverse
/// footprint is a barrier, including equal values and subsequently undone writes.
/// Reference writes are also barriers even if subsequently cleared. Future own
/// history reduction must be integrated with the transactional journal guard.
pub fn plan_move_inverse_cancellable(
    book: &Workbook,
    target: &CommittedEvent,
    tail: &[CommittedEvent],
    mut cancelled: impl FnMut() -> bool,
) -> Result<MoveInversePlan, MoveInverseError> {
    check_cancel(&mut cancelled)?;
    let Operation::MoveRange { writes, .. } = &target.operation else {
        return Err(MoveInverseError::NotUndoable);
    };
    if !matches!(target.principal, EventPrincipal::User(_)) {
        return Err(MoveInverseError::NotUndoable);
    }
    if tail.len() > MAX_TAIL_EVENTS || writes.is_empty() || writes.len() > 10_000 {
        return Err(MoveInverseError::LimitExceeded);
    }
    let mut expected = target.version.get();
    if target.document_id != book.document_id() || target.version > book.version() {
        return Err(MoveInverseError::InvalidHistory);
    }
    for event in tail {
        check_cancel(&mut cancelled)?;
        expected = expected
            .checked_add(1)
            .ok_or(MoveInverseError::InvalidHistory)?;
        if event.document_id != book.document_id() || event.version.get() != expected {
            return Err(MoveInverseError::InvalidHistory);
        }
        if event.operation.changes_axes() || matches!(event.operation, Operation::MoveRange { .. })
        {
            return Err(MoveInverseError::Conflict);
        }
    }
    let input_hash = book.hash();
    if expected != book.version().get() || tail.last().unwrap_or(target).state_hash != input_hash {
        return Err(MoveInverseError::InvalidHistory);
    }
    let mut footprint = BTreeSet::new();
    let mut positions: BTreeMap<SheetId, BTreeMap<u32, BTreeSet<u32>>> = BTreeMap::new();
    for write in writes {
        check_cancel(&mut cancelled)?;
        if !footprint.insert(write.address) {
            return Err(MoveInverseError::InvalidHistory);
        }
        if book.cell_content(write.address)? != write.content.as_ref() {
            return Err(MoveInverseError::Conflict);
        }
        let sheet = sheet(book, write.address.sheet)?;
        let row = sheet
            .row_position(write.address.row)
            .ok_or(MoveInverseError::Conflict)?;
        let column = sheet
            .column_position(write.address.column)
            .ok_or(MoveInverseError::Conflict)?;
        positions
            .entry(write.address.sheet)
            .or_default()
            .entry(row)
            .or_default()
            .insert(column);
    }
    let mut work = MAX_GUARD_WORK;
    for event in tail {
        check_cancel(&mut cancelled)?;
        visit_writes(&event.operation, |address, formula| {
            charge(&mut work)?;
            check_cancel(&mut cancelled)?;
            if footprint.contains(&address) {
                return Err(MoveInverseError::Conflict);
            }
            if let Some(formula) = formula {
                for node in formula.ast().nodes() {
                    charge(&mut work)?;
                    check_cancel(&mut cancelled)?;
                    match node {
                        Node::Cell(reference)
                            if footprint.contains(&reference_address(*reference)) =>
                        {
                            return Err(MoveInverseError::Conflict);
                        }
                        Node::Range(range) => {
                            let Some(rows) = positions.get(&range.start.target_sheet) else {
                                continue;
                            };
                            let sheet = sheet(book, range.start.target_sheet)?;
                            let r1 = sheet
                                .row_position(range.start.target_row)
                                .ok_or(MoveInverseError::Conflict)?;
                            let r2 = sheet
                                .row_position(range.end.target_row)
                                .ok_or(MoveInverseError::Conflict)?;
                            let c1 = sheet
                                .column_position(range.start.target_column)
                                .ok_or(MoveInverseError::Conflict)?;
                            let c2 = sheet
                                .column_position(range.end.target_column)
                                .ok_or(MoveInverseError::Conflict)?;
                            for (_, columns) in rows.range(r1.min(r2)..=r1.max(r2)) {
                                charge(&mut work)?;
                                check_cancel(&mut cancelled)?;
                                if columns.range(c1.min(c2)..=c1.max(c2)).next().is_some() {
                                    return Err(MoveInverseError::Conflict);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        })?;
    }
    check_cancel(&mut cancelled)?;
    Ok(MoveInversePlan {
        input_version: book.version(),
        input_hash,
        target_operation_id: target.operation_id,
        target_version: target.version,
        writes: writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
    })
}

fn check_cancel(cancelled: &mut impl FnMut() -> bool) -> Result<(), MoveInverseError> {
    if cancelled() {
        Err(MoveInverseError::Cancelled)
    } else {
        Ok(())
    }
}
fn charge(remaining: &mut usize) -> Result<(), MoveInverseError> {
    *remaining = remaining
        .checked_sub(1)
        .ok_or(MoveInverseError::LimitExceeded)?;
    Ok(())
}
fn sheet(book: &Workbook, id: SheetId) -> Result<&Sheet, MoveInverseError> {
    book.sheets()
        .iter()
        .find(|sheet| sheet.id() == id)
        .ok_or(MoveInverseError::Conflict)
}
fn reference_address(reference: BoundCellReference) -> CellAddress {
    CellAddress {
        sheet: reference.target_sheet,
        row: reference.target_row,
        column: reference.target_column,
    }
}
fn content_formula(content: Option<&CellContent>) -> Option<&Formula> {
    match content {
        Some(CellContent::Formula(formula)) => Some(formula),
        _ => None,
    }
}
fn visit_writes(
    operation: &Operation,
    mut visit: impl FnMut(CellAddress, Option<&Formula>) -> Result<(), MoveInverseError>,
) -> Result<(), MoveInverseError> {
    match operation {
        Operation::SetCellValue { address, .. } | Operation::ClearCell { address } => {
            visit(*address, None)?
        }
        Operation::SetCellFormula { address, formula } => visit(*address, Some(formula))?,
        Operation::CellWrite {
            address, content, ..
        } => visit(*address, content_formula(content.as_ref()))?,
        Operation::SetCellValues { writes } => {
            for write in writes {
                visit(write.address, None)?;
            }
        }
        Operation::CopyRange { writes } => {
            for write in writes {
                visit(write.address, content_formula(write.content.as_ref()))?;
            }
        }
        Operation::RangeWrite { writes, .. } | Operation::MoveRange { writes, .. } => {
            for write in writes {
                visit(write.address, content_formula(write.content.as_ref()))?;
            }
        }
        Operation::CreateWorkbook { .. }
        | Operation::AddSheet { .. }
        | Operation::ExtendRows { .. }
        | Operation::InsertRows { .. }
        | Operation::InsertRowsRebased { .. }
        | Operation::DeleteRows { .. }
        | Operation::InsertColumnsRebased { .. }
        | Operation::DeleteColumns { .. }
        | Operation::AdvanceCalculationEpoch { .. } => {}
    }
    Ok(())
}
