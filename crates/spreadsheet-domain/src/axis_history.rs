//! Bounded structural-inverse guards over verified durable history.
use crate::formula::{BoundCellReference, Formula, Node};
use crate::workbook::*;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_EVENTS: usize = 10_000;
const MAX_WORK: usize = 1_000_000;
const MAX_MATCHING_TOUCHES: usize = 100_000;

#[derive(Debug, Error)]
pub enum AxisHistoryError {
    #[error("axis event is not an inverse target")]
    NotUndoable,
    #[error("axis inverse conflicts with later history")]
    Conflict,
    #[error("axis history is not a contiguous verified tail")]
    InvalidHistory,
    #[error("axis history inspection budget exceeded")]
    LimitExceeded,
    #[error("axis history inspection was cancelled")]
    Cancelled,
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// The caller must authenticate target ownership and verify durable event bytes.
/// `proposed` is the independently derived inverse candidate, not client state.
pub fn validate_axis_inverse(
    book: &Workbook,
    target: &CommittedEvent,
    tail: &[CommittedEvent],
    proposed: &Workbook,
) -> Result<(), AxisHistoryError> {
    validate_axis_inverse_cancellable(book, target, tail, proposed, || false)
}

pub fn validate_axis_inverse_cancellable(
    book: &Workbook,
    target: &CommittedEvent,
    tail: &[CommittedEvent],
    proposed: &Workbook,
    mut cancelled: impl FnMut() -> bool,
) -> Result<(), AxisHistoryError> {
    check_cancel(&mut cancelled)?;
    let change = axis_change(&target.operation).ok_or(AxisHistoryError::NotUndoable)?;
    if !matches!(target.principal, EventPrincipal::User(_)) {
        return Err(AxisHistoryError::NotUndoable);
    }
    if tail.len() > MAX_EVENTS {
        return Err(AxisHistoryError::LimitExceeded);
    }
    if target.document_id != book.document_id()
        || target.version > book.version()
        || proposed.document_id() != book.document_id()
        || book.version().get().checked_add(1) != Some(proposed.version().get())
    {
        return Err(AxisHistoryError::InvalidHistory);
    }
    let mut expected = target.version.get();
    for event in tail {
        check_cancel(&mut cancelled)?;
        expected = expected
            .checked_add(1)
            .ok_or(AxisHistoryError::InvalidHistory)?;
        if event.document_id != book.document_id() || event.version.get() != expected {
            return Err(AxisHistoryError::InvalidHistory);
        }
    }
    if expected != book.version().get() || tail.last().unwrap_or(target).state_hash != book.hash() {
        return Err(AxisHistoryError::InvalidHistory);
    }
    // Canonical axis inverses may add offset patches for independent formulas.
    // Cancel these verified structural pairs as events, rather than pretending
    // every patch is a literal historical content swap.
    let mut axis_stack: Vec<usize> = Vec::new();
    let mut neutralized = BTreeSet::new();
    let mut work = MAX_WORK;
    let addresses: BTreeSet<_> = change.writes.iter().map(|write| write.address).collect();
    let axis_ids = AxisIds::from_change(change);
    let mut touches = MAX_MATCHING_TOUCHES;
    for (index, event) in tail.iter().enumerate() {
        charge(&mut work, 1)?;
        check_cancel(&mut cancelled)?;
        if let Some(current) = axis_change(&event.operation) {
            if event.principal != target.principal {
                return Err(AxisHistoryError::Conflict);
            }
            for write in &current.writes {
                charge(&mut work, 1)?;
                check_cancel(&mut cancelled)?;
                if addresses.contains(&write.address) || axis_ids.contains(write.address) {
                    charge(&mut touches, 1)?;
                }
            }
            let previous = axis_stack.last().and_then(|index| tail.get(*index));
            let cancels = previous.is_some_and(|previous| {
                inverse_identity(&event.operation)
                    == Some((previous.operation_id, previous.version))
                    && axis_change(&previous.operation).is_some_and(|old| {
                        old.sheet == current.sheet
                            && same_delta(&old.delta, &current.inverse_delta)
                            && same_delta(&old.inverse_delta, &current.delta)
                            && old.before_order_hash == current.after_order_hash
                            && old.after_order_hash == current.before_order_hash
                    })
            });
            charge(
                &mut work,
                delta_len(&current.delta) + delta_len(&current.inverse_delta),
            )?;
            if cancels {
                let previous = axis_stack.pop().ok_or(AxisHistoryError::InvalidHistory)?;
                neutralized.insert(previous);
                neutralized.insert(index);
            } else {
                axis_stack.push(index);
            }
        } else if event.operation.changes_axes() {
            return Err(AxisHistoryError::Conflict);
        }
    }
    if !axis_stack.is_empty() {
        return Err(AxisHistoryError::Conflict);
    }
    let current_index = Footprint::new(book, &addresses, &axis_ids)?;
    let proposed_index = Footprint::new(proposed, &addresses, &axis_ids)?;
    let mut ledger: BTreeMap<CellAddress, Vec<Entry<'_>>> = BTreeMap::new();
    for (index, event) in tail.iter().enumerate() {
        check_cancel(&mut cancelled)?;
        if neutralized.contains(&index) {
            continue;
        }
        for entry in entries(event) {
            charge(&mut work, 1)?;
            check_cancel(&mut cancelled)?;
            if addresses.contains(&entry.address) || axis_ids.contains(entry.address) {
                charge(&mut touches, 1)?;
            }
            let stack = ledger.entry(entry.address).or_default();
            let previous = stack.last();
            let possible = event.principal == target.principal
                && event.operation.is_inverse_target()
                && previous.is_some_and(|old| {
                    old.event.principal == target.principal
                        && old.event.operation.is_inverse_target()
                        && inverse_identity(&event.operation)
                            == Some((old.event.operation_id, old.event.version))
                });
            let cancels = if possible {
                let previous = previous.ok_or(AxisHistoryError::InvalidHistory)?;
                for content in [
                    previous.content,
                    previous.previous,
                    entry.content,
                    entry.previous,
                ] {
                    charge(
                        &mut work,
                        formula(content).map_or(1, |formula| formula.ast().nodes().len()),
                    )?;
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
    for entry in ledger.values().flatten() {
        charge(&mut work, 1)?;
        check_cancel(&mut cancelled)?;
        if matches!(
            entry.event.operation,
            Operation::MoveRange { .. } | Operation::MoveCompensation { .. }
        ) || addresses.contains(&entry.address)
            || axis_ids.contains(entry.address)
        {
            return Err(AxisHistoryError::Conflict);
        }
        if let Some(formula) = entry.formula {
            for node in formula.ast().nodes() {
                charge(&mut work, 1)?;
                check_cancel(&mut cancelled)?;
                match node {
                    Node::Cell(reference)
                        if addresses.contains(&reference_address(*reference))
                            || axis_ids.contains(reference_address(*reference)) =>
                    {
                        return Err(AxisHistoryError::Conflict);
                    }
                    Node::Range(range) => {
                        for index in [&current_index, &proposed_index] {
                            if index.intersects(range, &mut work, &mut cancelled)? {
                                return Err(AxisHistoryError::Conflict);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    check_cancel(&mut cancelled)
}

fn axis_change(operation: &Operation) -> Option<&AxisChange> {
    match operation {
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => Some(change),
        _ => None,
    }
}
fn delta_len(delta: &AxisDelta) -> usize {
    match delta {
        AxisDelta::Rows { removed, inserted } => removed.len() + inserted.len(),
        AxisDelta::Columns { removed, inserted } => removed.len() + inserted.len(),
    }
}
fn same_delta(left: &AxisDelta, right: &AxisDelta) -> bool {
    // Removal membership is a set; only insertions encode ordered placements.
    // Verified durable deltas already reject duplicate identities.
    match (left, right) {
        (
            AxisDelta::Rows {
                removed: a,
                inserted: x,
            },
            AxisDelta::Rows {
                removed: b,
                inserted: y,
            },
        ) => {
            x == y
                && a.len() == b.len()
                && a.iter().copied().collect::<BTreeSet<_>>()
                    == b.iter().copied().collect::<BTreeSet<_>>()
        }
        (
            AxisDelta::Columns {
                removed: a,
                inserted: x,
            },
            AxisDelta::Columns {
                removed: b,
                inserted: y,
            },
        ) => {
            x == y
                && a.len() == b.len()
                && a.iter().copied().collect::<BTreeSet<_>>()
                    == b.iter().copied().collect::<BTreeSet<_>>()
        }
        _ => false,
    }
}
fn inverse_identity(operation: &Operation) -> Option<(OperationId, DocumentVersion)> {
    match operation {
        Operation::CellWrite {
            kind: None,
            undoes: Some(id),
            undoes_version: Some(version),
            ..
        }
        | Operation::RangeWrite {
            kind: None,
            undoes: Some(id),
            undoes_version: Some(version),
            ..
        }
        | Operation::MoveCompensation {
            undoes: id,
            undoes_version: version,
            ..
        }
        | Operation::AxisCompensation {
            undoes: id,
            undoes_version: version,
            ..
        } => Some((*id, *version)),
        _ => None,
    }
}
struct AxisIds {
    sheet: SheetId,
    rows: BTreeSet<RowId>,
    columns: BTreeSet<ColumnId>,
}
impl AxisIds {
    fn from_change(change: &AxisChange) -> Self {
        let mut result = Self {
            sheet: change.sheet,
            rows: BTreeSet::new(),
            columns: BTreeSet::new(),
        };
        for delta in [&change.delta, &change.inverse_delta] {
            match delta {
                AxisDelta::Rows { removed, inserted } => {
                    result.rows.extend(removed);
                    result.rows.extend(inserted.iter().map(|p| p.row));
                }
                AxisDelta::Columns { removed, inserted } => {
                    result.columns.extend(removed);
                    result.columns.extend(inserted.iter().map(|p| p.column));
                }
            }
        }
        result
    }
    fn contains(&self, address: CellAddress) -> bool {
        address.sheet == self.sheet
            && (self.rows.contains(&address.row) || self.columns.contains(&address.column))
    }
}
struct Footprint<'a> {
    book: &'a Workbook,
    points: BTreeMap<SheetId, BTreeMap<u32, BTreeSet<u32>>>,
    axis_sheet: SheetId,
    axis_rows: BTreeSet<u32>,
    axis_columns: BTreeSet<u32>,
}
impl<'a> Footprint<'a> {
    fn new(
        book: &'a Workbook,
        addresses: &BTreeSet<CellAddress>,
        axes: &AxisIds,
    ) -> Result<Self, AxisHistoryError> {
        let mut result = Self {
            book,
            points: BTreeMap::new(),
            axis_sheet: axes.sheet,
            axis_rows: BTreeSet::new(),
            axis_columns: BTreeSet::new(),
        };
        for address in addresses {
            let Some(sheet) = book
                .sheets()
                .iter()
                .find(|sheet| sheet.id() == address.sheet)
            else {
                continue;
            };
            if let (Some(row), Some(column)) = (
                sheet.row_position(address.row),
                sheet.column_position(address.column),
            ) {
                result
                    .points
                    .entry(address.sheet)
                    .or_default()
                    .entry(row)
                    .or_default()
                    .insert(column);
            }
        }
        let sheet = book
            .sheets()
            .iter()
            .find(|sheet| sheet.id() == axes.sheet)
            .ok_or(AxisHistoryError::Conflict)?;
        result
            .axis_rows
            .extend(axes.rows.iter().filter_map(|id| sheet.row_position(*id)));
        result.axis_columns.extend(
            axes.columns
                .iter()
                .filter_map(|id| sheet.column_position(*id)),
        );
        Ok(result)
    }
    fn intersects(
        &self,
        range: &crate::formula::BoundRangeReference,
        work: &mut usize,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<bool, AxisHistoryError> {
        let sheet_id = range.start.target_sheet;
        if sheet_id != self.axis_sheet && !self.points.contains_key(&sheet_id) {
            return Ok(false);
        }
        let sheet = self
            .book
            .sheets()
            .iter()
            .find(|sheet| sheet.id() == sheet_id)
            .ok_or(AxisHistoryError::Conflict)?;
        let (Some(r1), Some(r2), Some(c1), Some(c2)) = (
            sheet.row_position(range.start.target_row),
            sheet.row_position(range.end.target_row),
            sheet.column_position(range.start.target_column),
            sheet.column_position(range.end.target_column),
        ) else {
            return Ok(false);
        };
        if sheet_id == self.axis_sheet
            && (self
                .axis_rows
                .range(r1.min(r2)..=r1.max(r2))
                .next()
                .is_some()
                || self
                    .axis_columns
                    .range(c1.min(c2)..=c1.max(c2))
                    .next()
                    .is_some())
        {
            return Ok(true);
        }
        if let Some(rows) = self.points.get(&sheet_id) {
            for (_, columns) in rows.range(r1.min(r2)..=r1.max(r2)) {
                charge(work, 1)?;
                check_cancel(cancelled)?;
                if columns.range(c1.min(c2)..=c1.max(c2)).next().is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
struct Entry<'a> {
    event: &'a CommittedEvent,
    address: CellAddress,
    content: Option<&'a CellContent>,
    previous: Option<&'a CellContent>,
    formula: Option<&'a Formula>,
}
fn formula(content: Option<&CellContent>) -> Option<&Formula> {
    match content {
        Some(CellContent::Formula(formula)) => Some(formula),
        _ => None,
    }
}
fn entries(event: &CommittedEvent) -> Vec<Entry<'_>> {
    let mut entries = Vec::new();
    let mut add = |address, content, previous, formula| {
        entries.push(Entry {
            event,
            address,
            content,
            previous,
            formula,
        })
    };
    match &event.operation {
        Operation::CellWrite {
            address,
            content,
            previous,
            ..
        } => add(
            *address,
            content.as_ref(),
            previous.as_ref(),
            formula(content.as_ref()),
        ),
        Operation::RangeWrite { writes, .. }
        | Operation::MoveRange { writes, .. }
        | Operation::MoveCompensation { writes, .. } => {
            for write in writes {
                add(
                    write.address,
                    write.content.as_ref(),
                    write.previous.as_ref(),
                    formula(write.content.as_ref()),
                );
            }
        }
        Operation::SetCellValue { address, .. } | Operation::ClearCell { address } => {
            add(*address, None, None, None)
        }
        Operation::SetCellFormula { address, formula } => add(*address, None, None, Some(formula)),
        Operation::SetCellValues { writes } => {
            for write in writes {
                add(write.address, None, None, None);
            }
        }
        Operation::CopyRange { writes } => {
            for write in writes {
                add(write.address, None, None, formula(write.content.as_ref()));
            }
        }
        _ => {}
    }
    entries
}
fn reference_address(reference: BoundCellReference) -> CellAddress {
    CellAddress {
        sheet: reference.target_sheet,
        row: reference.target_row,
        column: reference.target_column,
    }
}
fn check_cancel(cancelled: &mut impl FnMut() -> bool) -> Result<(), AxisHistoryError> {
    if cancelled() {
        Err(AxisHistoryError::Cancelled)
    } else {
        Ok(())
    }
}
fn charge(work: &mut usize, amount: usize) -> Result<(), AxisHistoryError> {
    *work = work
        .checked_sub(amount)
        .ok_or(AxisHistoryError::LimitExceeded)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axis_edit::{prepare_axis_edit, prepare_axis_inverse};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
    fn apply(book: &mut Workbook, operation: Operation) -> TestResult<CommittedEvent> {
        let version = DocumentVersion::new(book.version().get() + 1)?;
        let candidate = book.candidate(&operation, version)?;
        let event = CommittedEvent {
            document_id: book.document_id(),
            operation_id: OperationId::from_bytes(u128::from(version.get()).to_be_bytes())?,
            principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
            version,
            operation,
            state_hash: candidate.hash(),
        };
        *book = candidate;
        Ok(event)
    }
    fn pair(
        book: &mut Workbook,
        sheet: SheetId,
        row: RowId,
        tail: &mut Vec<CommittedEvent>,
    ) -> TestResult {
        let columns = book
            .sheets()
            .iter()
            .find(|s| s.id() == sheet)
            .ok_or("sheet")?
            .columns()
            .to_vec();
        let writes = columns
            .into_iter()
            .map(|column| GuardedContentWrite {
                address: CellAddress { sheet, row, column },
                content: Some(CellContent::Value(CellValue::Number(1.))),
                previous: None,
            })
            .collect::<Vec<_>>();
        let written = apply(
            book,
            Operation::RangeWrite {
                writes: writes.clone(),
                kind: Some(RangeWriteKind::SetValues),
                undoes: None,
                undoes_version: None,
            },
        )?;
        let inverse = Operation::RangeWrite {
            writes: writes
                .into_iter()
                .map(|w| GuardedContentWrite {
                    address: w.address,
                    content: w.previous,
                    previous: w.content,
                })
                .collect(),
            kind: None,
            undoes: Some(written.operation_id),
            undoes_version: Some(written.version),
        };
        tail.push(written);
        tail.push(apply(book, inverse)?);
        Ok(())
    }
    #[test]
    fn axis_history_matching_touch_budget_counts_cancelled_updates_at_boundary() -> TestResult {
        let sheet = SheetId::from_bytes([2; 16])?;
        let before = RowId::from_bytes([3; 16])?;
        let inserted = RowId::from_bytes([4; 16])?;
        let seed = SheetSeed {
            id: sheet,
            name: "Budget".into(),
            rows: vec![before],
            columns: (0..1000u128)
                .map(|n| ColumnId::from_bytes((100 + n).to_be_bytes()))
                .collect::<Result<_, _>>()?,
        };
        let mut book = Workbook::hydrate_sheets(
            DocumentId::from_bytes([1; 16])?,
            DocumentVersion::new(1)?,
            [seed],
            [],
        )?;
        let operation = prepare_axis_edit(
            &book,
            &Operation::InsertRowsRebased {
                sheet,
                before,
                rows: vec![inserted],
            },
            DocumentVersion::new(2)?,
        )?;
        let target = apply(&mut book, operation)?;
        let mut tail = Vec::new();
        for _ in 0..50 {
            pair(&mut book, sheet, inserted, &mut tail)?;
        }
        let inverse = prepare_axis_inverse(
            &book,
            &target,
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let proposed = book.candidate(&inverse, DocumentVersion::new(book.version().get() + 1)?)?;
        validate_axis_inverse(&book, &target, &tail, &proposed)?;
        pair(&mut book, sheet, inserted, &mut tail)?;
        let inverse = prepare_axis_inverse(
            &book,
            &target,
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        let proposed = book.candidate(&inverse, DocumentVersion::new(book.version().get() + 1)?)?;
        assert!(matches!(
            validate_axis_inverse(&book, &target, &tail, &proposed),
            Err(AxisHistoryError::LimitExceeded)
        ));
        Ok(())
    }
}
