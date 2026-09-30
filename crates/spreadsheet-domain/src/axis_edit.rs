//! Frozen authored axis edits. Journal admission independently proves inverse ownership.
use crate::workbook::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowPlacement {
    pub row: RowId,
    pub before: Option<RowId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnPlacement {
    pub column: ColumnId,
    pub before: Option<ColumnId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AxisDelta {
    Rows {
        removed: Vec<RowId>,
        inserted: Vec<RowPlacement>,
    },
    Columns {
        removed: Vec<ColumnId>,
        inserted: Vec<ColumnPlacement>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct AxisChange {
    pub sheet: SheetId,
    pub delta: AxisDelta,
    pub inverse_delta: AxisDelta,
    pub writes: Vec<GuardedContentWrite>,
    pub before_order_hash: [u8; 32],
    pub after_order_hash: [u8; 32],
}
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum AxisEditError {
    #[error("axis edit cannot be inverted")]
    NotUndoable,
    #[error(transparent)]
    Domain(#[from] DomainError),
}

const MAX_WORK: usize = 1_000_000;
fn charge(work: &mut usize, amount: usize) -> Result<(), DomainError> {
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= MAX_WORK)
        .ok_or(DomainError::LimitExceeded)?;
    Ok(())
}
fn sheet(book: &Workbook, id: SheetId) -> Result<&Sheet, DomainError> {
    book.sheets
        .iter()
        .find(|s| s.id == id)
        .ok_or(DomainError::MissingSheet)
}
pub fn axis_order_hash(
    book: &Workbook,
    id: SheetId,
    delta: &AxisDelta,
) -> Result<[u8; 32], DomainError> {
    let s = sheet(book, id)?;
    let mut hash = Sha256::new();
    hash.update(b"PXEL-AXIS-ORDER-1\0");
    hash.update(id.as_bytes());
    match delta {
        AxisDelta::Rows { .. } => {
            hash.update([0]);
            hash.update((s.rows.len() as u64).to_be_bytes());
            for id in &s.rows {
                hash.update(id.as_bytes());
            }
        }
        AxisDelta::Columns { .. } => {
            hash.update([1]);
            hash.update((s.columns.len() as u64).to_be_bytes());
            for id in &s.columns {
                hash.update(id.as_bytes());
            }
        }
    }
    Ok(hash.finalize().into())
}
fn mutate_axis<T: Copy + Ord>(
    ordered: &mut Vec<T>,
    removed: &[T],
    inserted: &[(T, Option<T>)],
    limit: usize,
    max_ids: usize,
) -> Result<Vec<(T, Option<T>)>, DomainError> {
    if (removed.is_empty() == inserted.is_empty()) || removed.len().max(inserted.len()) > max_ids {
        return Err(DomainError::InvalidAxis);
    }
    let mut unique = BTreeSet::new();
    for id in removed {
        if !unique.insert(*id) || !ordered.contains(id) {
            return Err(DomainError::InvalidAxis);
        }
    }
    let placements = ordered
        .iter()
        .enumerate()
        .filter(|(_, id)| unique.contains(id))
        .map(|(i, id)| {
            (
                *id,
                ordered[i + 1..]
                    .iter()
                    .find(|id| !unique.contains(id))
                    .copied(),
            )
        })
        .collect();
    if !removed.is_empty() {
        if removed.len() >= ordered.len() {
            return Err(DomainError::InvalidAxis);
        }
        ordered.retain(|id| !unique.contains(id));
    } else {
        if ordered.len().saturating_add(inserted.len()) > limit {
            return Err(DomainError::LimitExceeded);
        }
        // Anchors must be live in the original axis, never another inserted ID.
        for (id, before) in inserted {
            if !unique.insert(*id)
                || ordered.contains(id)
                || before.is_some_and(|anchor| !ordered.contains(&anchor))
            {
                return Err(DomainError::InvalidAxis);
            }
        }
        for (id, before) in inserted {
            let position = match before {
                Some(anchor) => ordered
                    .iter()
                    .position(|id| id == anchor)
                    .ok_or(DomainError::InvalidAxis)?,
                None => ordered.len(),
            };
            ordered.insert(position, *id);
        }
    }
    Ok(placements)
}

/// Derive axes and deterministic reference changes without trusting frozen writes.
fn transformed(
    book: &Workbook,
    id: SheetId,
    delta: &AxisDelta,
    next: DocumentVersion,
    restore: bool,
) -> Result<(Workbook, AxisDelta), DomainError> {
    if next.get()
        != book
            .version
            .get()
            .checked_add(1)
            .ok_or(DomainError::VersionConflict)?
    {
        return Err(DomainError::VersionConflict);
    }
    let original = sheet(book, id)?;
    let mut candidate = book.clone();
    let target = candidate
        .sheets
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(DomainError::MissingSheet)?;
    let inverse = match delta {
        AxisDelta::Rows { removed, inserted } => {
            if original.styles.keys().any(|a| removed.contains(&a.row)) {
                return Err(DomainError::UnsupportedStructuralDependency);
            }
            if removed
                .iter()
                .any(|row| !original.row_positions.contains_key(row))
                || inserted.iter().any(|placement| {
                    placement
                        .before
                        .is_some_and(|row| !original.row_positions.contains_key(&row))
                })
            {
                return Err(DomainError::MissingRow);
            }
            for placement in inserted {
                if book
                    .sheets
                    .iter()
                    .any(|s| s.row_positions.contains_key(&placement.row))
                    || (book.retired_row_ids.contains(&placement.row) != restore)
                {
                    return Err(DomainError::InvalidAxis);
                }
            }
            let old = mutate_axis(
                &mut target.rows,
                removed,
                &inserted
                    .iter()
                    .map(|p| (p.row, p.before))
                    .collect::<Vec<_>>(),
                MAX_ROWS,
                1000,
            )?;
            target.row_positions = target
                .rows
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i as u32))
                .collect();
            target.cells.retain(|a, _| !removed.contains(&a.row));
            candidate.retired_row_ids.extend(removed.iter().copied());
            if candidate.retired_row_ids.len() > MAX_RETIRED_ROWS {
                return Err(DomainError::LimitExceeded);
            }
            if restore {
                for p in inserted {
                    candidate.row_activation_floors.insert(p.row, next);
                }
            }
            AxisDelta::Rows {
                removed: inserted.iter().map(|p| p.row).collect(),
                inserted: old
                    .into_iter()
                    .map(|(row, before)| RowPlacement { row, before })
                    .collect(),
            }
        }
        AxisDelta::Columns { removed, inserted } => {
            if original.styles.keys().any(|a| removed.contains(&a.column)) {
                return Err(DomainError::UnsupportedStructuralDependency);
            }
            if removed
                .iter()
                .any(|column| !original.column_positions.contains_key(column))
                || inserted.iter().any(|placement| {
                    placement
                        .before
                        .is_some_and(|column| !original.column_positions.contains_key(&column))
                })
            {
                return Err(DomainError::MissingColumn);
            }
            for placement in inserted {
                if book
                    .sheets
                    .iter()
                    .any(|s| s.column_positions.contains_key(&placement.column))
                    || (book.retired_column_ids.contains(&placement.column) != restore)
                {
                    return Err(DomainError::InvalidAxis);
                }
            }
            let old = mutate_axis(
                &mut target.columns,
                removed,
                &inserted
                    .iter()
                    .map(|p| (p.column, p.before))
                    .collect::<Vec<_>>(),
                MAX_COLUMNS,
                100,
            )?;
            target.column_positions = target
                .columns
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i as u32))
                .collect();
            target.cells.retain(|a, _| !removed.contains(&a.column));
            candidate.retired_column_ids.extend(removed.iter().copied());
            if candidate.retired_column_ids.len() > MAX_RETIRED_COLUMNS {
                return Err(DomainError::LimitExceeded);
            }
            if restore {
                for p in inserted {
                    candidate.column_activation_floors.insert(p.column, next);
                }
            }
            AxisDelta::Columns {
                removed: inserted.iter().map(|p| p.column).collect(),
                inserted: old
                    .into_iter()
                    .map(|(column, before)| ColumnPlacement { column, before })
                    .collect(),
            }
        }
    };
    let mut updates = Vec::new();
    let mut work = 0;
    for current in &candidate.sheets {
        for (address, content) in &current.cells {
            charge(&mut work, 1)?;
            let CellContent::Formula(formula) = content else {
                continue;
            };
            let removals = match delta {
                AxisDelta::Rows { removed, .. } => removed.len(),
                AxisDelta::Columns { removed, .. } => removed.len(),
            };
            charge(
                &mut work,
                formula.ast().nodes().len().saturating_mul(removals + 1),
            )?;
            let rebased = match delta {
                AxisDelta::Rows { removed, .. } => {
                    let deleted: BTreeSet<_> = removed.iter().copied().collect();
                    formula.rebase_deleted_rows(
                        next,
                        current
                            .row_position(address.row)
                            .ok_or(DomainError::MissingRow)?,
                        id,
                        &deleted,
                        |s, r| {
                            candidate
                                .sheets
                                .iter()
                                .find(|sht| sht.id == s)
                                .and_then(|sht| sht.row_position(r))
                        },
                        |start, end| {
                            survivors(
                                &original.rows,
                                &original.row_positions,
                                &deleted,
                                start,
                                end,
                            )
                        },
                    )
                }
                AxisDelta::Columns { removed, .. } => {
                    let deleted: BTreeSet<_> = removed.iter().copied().collect();
                    formula.rebase_deleted_columns(
                        next,
                        current
                            .column_position(address.column)
                            .ok_or(DomainError::MissingColumn)?,
                        id,
                        &deleted,
                        |s, c| {
                            candidate
                                .sheets
                                .iter()
                                .find(|sht| sht.id == s)
                                .and_then(|sht| sht.column_position(c))
                        },
                        |start, end| {
                            survivors(
                                &original.columns,
                                &original.column_positions,
                                &deleted,
                                start,
                                end,
                            )
                        },
                    )
                }
            }
            .map_err(|_| DomainError::InvalidFormula)?;
            if rebased.ast() != formula.ast() {
                updates.push((*address, rebased));
            }
        }
    }
    for (address, formula) in updates {
        candidate
            .sheets
            .iter_mut()
            .find(|s| s.id == address.sheet)
            .ok_or(DomainError::MissingSheet)?
            .cells
            .insert(address, CellContent::Formula(formula));
    }
    candidate.version = next;
    candidate.last_axis_change_version = next;
    Ok((candidate, inverse))
}
fn survivors<T: Copy + Ord + std::hash::Hash>(
    axis: &[T],
    positions: &HashMap<T, u32>,
    removed: &BTreeSet<T>,
    start: T,
    end: T,
) -> Option<(T, T)> {
    let start_pos = *positions.get(&start)? as usize;
    let end_pos = *positions.get(&end)? as usize;
    let span = &axis[start_pos.min(end_pos)..=start_pos.max(end_pos)];
    let first = span.iter().find(|id| !removed.contains(id)).copied()?;
    let last = span
        .iter()
        .rev()
        .find(|id| !removed.contains(id))
        .copied()?;
    Some(if start_pos <= end_pos {
        (first, last)
    } else {
        (last, first)
    })
}
fn diff(before: &Workbook, after: &Workbook) -> Result<Vec<GuardedContentWrite>, DomainError> {
    let mut changes = BTreeMap::new();
    let mut work = 0;
    for sheet in &before.sheets {
        for (address, previous) in &sheet.cells {
            charge(&mut work, 1 + previous.ast_memory_bytes() / 256)?;
            let content = after
                .sheets
                .iter()
                .find(|s| s.id == address.sheet)
                .and_then(|s| s.cells.get(address));
            if content != Some(previous) {
                changes.insert(
                    *address,
                    GuardedContentWrite {
                        address: *address,
                        previous: Some(previous.clone()),
                        content: content.cloned(),
                    },
                );
            }
        }
    }
    for sheet in &after.sheets {
        for (address, content) in &sheet.cells {
            charge(&mut work, 1)?;
            let previous = before
                .sheets
                .iter()
                .find(|s| s.id == address.sheet)
                .and_then(|s| s.cells.get(address));
            if previous.is_none() {
                changes.insert(
                    *address,
                    GuardedContentWrite {
                        address: *address,
                        previous: None,
                        content: Some(content.clone()),
                    },
                );
            }
        }
    }
    let writes: Vec<_> = changes.into_values().collect();
    validate_writes_budget(&writes)?;
    Ok(writes)
}
fn validate_writes_budget(writes: &[GuardedContentWrite]) -> Result<(), DomainError> {
    if writes.len() > 10000 {
        return Err(DomainError::LimitExceeded);
    }
    let mut incoming = 0usize;
    let mut previous = 0usize;
    let mut work = 0;
    let mut ids = BTreeSet::new();
    for write in writes {
        if !ids.insert(write.address) {
            return Err(DomainError::InvalidAxis);
        }
        for content in [&write.previous, &write.content].into_iter().flatten() {
            charge(&mut work, 1 + content.ast_memory_bytes() / 256)?;
        }
        incoming = incoming
            .checked_add(write.content.as_ref().map_or(0, CellContent::text_bytes))
            .ok_or(DomainError::LimitExceeded)?;
        previous = previous
            .checked_add(write.previous.as_ref().map_or(0, CellContent::text_bytes))
            .ok_or(DomainError::LimitExceeded)?;
    }
    if incoming > MAX_CELL_VALUE_BATCH_TEXT_BYTES || previous > MAX_CELL_VALUE_BATCH_TEXT_BYTES {
        return Err(DomainError::LimitExceeded);
    }
    Ok(())
}
fn finish(candidate: &mut Workbook) -> Result<(), DomainError> {
    crate::style_write::validate_capacity(candidate)?;
    let mut text = 0usize;
    let mut ast = 0usize;
    let mut cells = 0usize;
    for sheet in &candidate.sheets {
        for (address, content) in &sheet.cells {
            sheet.validate_address(*address)?;
            match content {
                CellContent::Formula(f) => candidate.validate_formula(*address, f)?,
                CellContent::Value(v) => {
                    v.validate()?;
                    if matches!(v, CellValue::Blank) {
                        return Err(DomainError::InvalidValue);
                    }
                }
            }
            text = text
                .checked_add(content.text_bytes())
                .ok_or(DomainError::LimitExceeded)?;
            ast = ast
                .checked_add(content.ast_memory_bytes())
                .ok_or(DomainError::LimitExceeded)?;
            cells += 1;
        }
    }
    if text > MAX_WORKBOOK_TEXT_BYTES || ast > MAX_WORKBOOK_AST_BYTES || cells > MAX_CELLS {
        return Err(DomainError::LimitExceeded);
    }
    candidate.text_bytes = text;
    candidate.ast_memory_bytes = ast;
    Ok(())
}
fn change_from(
    book: &Workbook,
    delta: AxisDelta,
    id: SheetId,
    next: DocumentVersion,
) -> Result<AxisChange, DomainError> {
    let (mut candidate, inverse_delta) = transformed(book, id, &delta, next, false)?;
    finish(&mut candidate)?;
    Ok(AxisChange {
        sheet: id,
        before_order_hash: axis_order_hash(book, id, &delta)?,
        after_order_hash: axis_order_hash(&candidate, id, &delta)?,
        writes: diff(book, &candidate)?,
        delta,
        inverse_delta,
    })
}
pub fn prepare_axis_edit(
    book: &Workbook,
    legacy: &Operation,
    next: DocumentVersion,
) -> Result<Operation, AxisEditError> {
    let (sheet, delta) = match legacy {
        Operation::InsertRows {
            sheet,
            before,
            rows,
        }
        | Operation::InsertRowsRebased {
            sheet,
            before,
            rows,
        } => (
            *sheet,
            AxisDelta::Rows {
                removed: vec![],
                inserted: rows
                    .iter()
                    .map(|row| RowPlacement {
                        row: *row,
                        before: Some(*before),
                    })
                    .collect(),
            },
        ),
        Operation::DeleteRows { sheet, rows } => (
            *sheet,
            AxisDelta::Rows {
                removed: rows.clone(),
                inserted: vec![],
            },
        ),
        Operation::InsertColumnsRebased {
            sheet,
            before,
            columns,
        } => (
            *sheet,
            AxisDelta::Columns {
                removed: vec![],
                inserted: columns
                    .iter()
                    .map(|column| ColumnPlacement {
                        column: *column,
                        before: Some(*before),
                    })
                    .collect(),
            },
        ),
        Operation::DeleteColumns { sheet, columns } => (
            *sheet,
            AxisDelta::Columns {
                removed: columns.clone(),
                inserted: vec![],
            },
        ),
        _ => return Err(AxisEditError::NotUndoable),
    };
    Ok(Operation::AxisEdit {
        change: change_from(book, delta, sheet, next)?,
    })
}
pub fn prepare_axis_inverse(
    book: &Workbook,
    target: &CommittedEvent,
    next: DocumentVersion,
) -> Result<Operation, AxisEditError> {
    if target.document_id != book.document_id || target.version > book.version {
        return Err(DomainError::VersionConflict.into());
    }
    let change = match &target.operation {
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => change,
        _ => return Err(AxisEditError::NotUndoable),
    };
    if axis_order_hash(book, change.sheet, &change.delta)? != change.after_order_hash {
        return Err(DomainError::PreimageConflict.into());
    }
    let (mut candidate, inverse_delta) =
        transformed(book, change.sheet, &change.inverse_delta, next, true)?;
    for write in &change.writes {
        let current = book
            .sheets
            .iter()
            .find(|s| s.id == write.address.sheet)
            .and_then(|s| s.cells.get(&write.address));
        if current != write.content.as_ref() {
            return Err(DomainError::PreimageConflict.into());
        }
        let sheet = candidate
            .sheets
            .iter_mut()
            .find(|s| s.id == write.address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        match &write.previous {
            Some(content) => {
                sheet.validate_address(write.address)?;
                sheet.cells.insert(write.address, content.clone());
            }
            None => {
                sheet.cells.remove(&write.address);
            }
        }
    }
    finish(&mut candidate)?;
    let result = AxisChange {
        sheet: change.sheet,
        delta: change.inverse_delta.clone(),
        inverse_delta,
        before_order_hash: change.after_order_hash,
        after_order_hash: change.before_order_hash,
        writes: diff(book, &candidate)?,
    };
    if axis_order_hash(&candidate, result.sheet, &result.delta)? != result.after_order_hash {
        return Err(DomainError::InvalidAxis.into());
    }
    Ok(Operation::AxisCompensation {
        change: result,
        undoes: target.operation_id,
        undoes_version: target.version,
    })
}

pub(crate) fn candidate(
    book: &Workbook,
    change: &AxisChange,
    next: DocumentVersion,
    compensation: bool,
) -> Result<Workbook, DomainError> {
    validate_writes_budget(&change.writes)?;
    if axis_order_hash(book, change.sheet, &change.delta)? != change.before_order_hash {
        return Err(DomainError::PreimageConflict);
    }
    if !compensation && change_from(book, change.delta.clone(), change.sheet, next)? != *change {
        return Err(DomainError::InvalidAxis);
    }
    let (mut candidate, inverse) =
        transformed(book, change.sheet, &change.delta, next, compensation)?;
    if inverse != change.inverse_delta
        || axis_order_hash(&candidate, change.sheet, &change.delta)? != change.after_order_hash
    {
        return Err(DomainError::InvalidAxis);
    }
    for write in &change.writes {
        let previous = book
            .sheets
            .iter()
            .find(|s| s.id == write.address.sheet)
            .and_then(|s| s.cells.get(&write.address));
        if previous != write.previous.as_ref() {
            return Err(DomainError::PreimageConflict);
        }
        let restored_address = write.address.sheet == change.sheet
            && match &change.delta {
                AxisDelta::Rows { inserted, .. } => {
                    inserted.iter().any(|p| p.row == write.address.row)
                }
                AxisDelta::Columns { inserted, .. } => {
                    inserted.iter().any(|p| p.column == write.address.column)
                }
            };
        let derived = candidate
            .sheets
            .iter()
            .find(|s| s.id == write.address.sheet)
            .and_then(|s| s.cells.get(&write.address));
        if compensation && !restored_address && derived != write.content.as_ref() {
            match (&write.previous, &write.content) {
                (Some(CellContent::Formula(old)), Some(CellContent::Formula(new)))
                    if old.source() == new.source()
                        && old.canonical() == new.canonical()
                        && old.origin() == new.origin()
                        && old.profile() == new.profile() => {}
                _ => return Err(DomainError::InvalidAxis),
            }
        }
        let target = candidate
            .sheets
            .iter_mut()
            .find(|s| s.id == write.address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        match &write.content {
            Some(content) => {
                target.validate_address(write.address)?;
                target.cells.insert(write.address, content.clone());
            }
            None => {
                target.cells.remove(&write.address);
            }
        }
    }
    // Derived deletion/offset changes cannot be omitted. Restored frozen ASTs
    // deliberately override lossy transformations; journal proves exact inverse.
    if diff(book, &candidate)? != change.writes {
        return Err(DomainError::InvalidAxis);
    }
    finish(&mut candidate)?;
    Ok(candidate)
}
