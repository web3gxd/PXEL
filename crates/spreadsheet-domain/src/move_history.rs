//! Frozen inverse planning; durable eligibility and publication remain application work.
use crate::formula::{BoundCellReference, Formula, Node};
use crate::workbook::*;
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
/// not authorization or commit admission. Exact own inverse pairs cancel per
/// address; foreign and legacy touches cannot be cancelled by this reduction.
pub fn plan_move_inverse_cancellable(
    book: &Workbook,
    target: &CommittedEvent,
    tail: &[CommittedEvent],
    mut cancelled: impl FnMut() -> bool,
) -> Result<MoveInversePlan, MoveInverseError> {
    check_cancel(&mut cancelled)?;
    let writes = match &target.operation {
        Operation::MoveRange { writes, .. } | Operation::MoveCompensation { writes, .. } => writes,
        _ => return Err(MoveInverseError::NotUndoable),
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
        if event.operation.changes_axes() {
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
    let mut ledger: BTreeMap<CellAddress, Vec<HistoryWrite<'_>>> = BTreeMap::new();
    for event in tail {
        check_cancel(&mut cancelled)?;
        let own = event.principal == target.principal && event.operation.is_inverse_target();
        let inverse = inverse_identity(&event.operation);
        for entry in history_writes(event)? {
            charge(&mut work)?;
            check_cancel(&mut cancelled)?;
            let stack = ledger.entry(entry.address).or_default();
            let possible = own
                && inverse.is_some_and(|(id, version)| {
                    stack.last().is_some_and(|previous| {
                        previous.event.principal == target.principal
                            && previous.event.operation.is_inverse_target()
                            && previous.event.operation_id == id
                            && previous.event.version == version
                    })
                });
            let cancels = if possible {
                let previous = stack.last().ok_or(MoveInverseError::InvalidHistory)?;
                for content in [
                    previous.content,
                    previous.previous,
                    entry.content,
                    entry.previous,
                ] {
                    let nodes =
                        content_formula(content).map_or(1, |formula| formula.ast().nodes().len());
                    work = work
                        .checked_sub(nodes)
                        .ok_or(MoveInverseError::LimitExceeded)?;
                }
                previous.content == entry.previous && previous.previous == entry.content
            } else {
                false
            };
            if cancels {
                stack.pop();
            } else {
                stack.push(entry);
            }
        }
    }
    for stack in ledger.values() {
        for entry in stack {
            charge(&mut work)?;
            check_cancel(&mut cancelled)?;
            let address = entry.address;
            let formula = entry.formula;
            // A later effective move remains a global structural-history barrier.
            if matches!(
                entry.event.operation,
                Operation::MoveRange { .. } | Operation::MoveCompensation { .. }
            ) {
                return Err(MoveInverseError::Conflict);
            }
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
        }
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
struct HistoryWrite<'a> {
    event: &'a CommittedEvent,
    address: CellAddress,
    content: Option<&'a CellContent>,
    previous: Option<&'a CellContent>,
    formula: Option<&'a Formula>,
}
fn inverse_identity(operation: &Operation) -> Option<(OperationId, DocumentVersion)> {
    match operation {
        Operation::CellWrite {
            undoes: Some(id),
            undoes_version: Some(version),
            kind: None,
            ..
        }
        | Operation::RangeWrite {
            undoes: Some(id),
            undoes_version: Some(version),
            kind: None,
            ..
        } => Some((*id, *version)),
        Operation::MoveCompensation {
            undoes,
            undoes_version,
            ..
        } => Some((*undoes, *undoes_version)),
        _ => None,
    }
}
fn history_writes(event: &CommittedEvent) -> Result<Vec<HistoryWrite<'_>>, MoveInverseError> {
    let mut entries = Vec::new();
    match &event.operation {
        Operation::CellWrite {
            address,
            content,
            previous,
            ..
        } => entries.push(HistoryWrite {
            event,
            address: *address,
            content: content.as_ref(),
            previous: previous.as_ref(),
            formula: content_formula(content.as_ref()),
        }),
        Operation::RangeWrite { writes, .. }
        | Operation::MoveRange { writes, .. }
        | Operation::MoveCompensation { writes, .. } => {
            for write in writes {
                entries.push(HistoryWrite {
                    event,
                    address: write.address,
                    content: write.content.as_ref(),
                    previous: write.previous.as_ref(),
                    formula: content_formula(write.content.as_ref()),
                });
            }
        }
        _ => visit_writes(&event.operation, |address, formula| {
            entries.push(HistoryWrite {
                event,
                address,
                content: None,
                previous: None,
                formula,
            });
            Ok(())
        })?,
    }
    Ok(entries)
}
fn visit_writes<'a>(
    operation: &'a Operation,
    mut visit: impl FnMut(CellAddress, Option<&'a Formula>) -> Result<(), MoveInverseError>,
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
        Operation::RangeWrite { writes, .. }
        | Operation::MoveRange { writes, .. }
        | Operation::MoveCompensation { writes, .. } => {
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
