//! Independently verify property-level style intent and durable inverse history.
use super::*;
use spreadsheet_domain::{
    style::{StyleChange, StyleProperty},
    style_write::prepare_style_write,
};
use std::collections::{BTreeMap, BTreeSet};

fn style_writes(operation: &Operation) -> Option<(StyleWriteKind, &[GuardedStyleWrite])> {
    match operation {
        Operation::StyleWrite { kind, writes }
        | Operation::StyleCompensation { kind, writes, .. } => Some((*kind, writes)),
        _ => None,
    }
}
fn inverse(operation: &Operation) -> Option<(OperationId, DocumentVersion)> {
    match operation {
        Operation::StyleCompensation {
            undoes,
            undoes_version,
            ..
        } => Some((*undoes, *undoes_version)),
        _ => None,
    }
}

struct Touch<'a> {
    id: OperationId,
    version: DocumentVersion,
    previous: &'a StyleChange,
    change: &'a StyleChange,
}
type Leaf = (CellAddress, StyleProperty);

/// Filtered leaf stacks never cancel foreign writes, including equal-value resets.
fn validate_history<'a>(
    actor: EventPrincipal,
    tail: &'a [CommittedEvent],
    mut affected: impl FnMut(CellAddress, StyleProperty) -> bool,
) -> Result<(), JournalError> {
    let mut stacks: BTreeMap<Leaf, Vec<Touch<'a>>> = BTreeMap::new();
    let mut work = 0usize;
    for event in tail {
        let Some((_, writes)) = style_writes(&event.operation) else {
            continue;
        };
        for write in writes {
            if write.previous.len() != write.changes.len() {
                return Err(JournalError::Corrupt);
            }
            for (previous, change) in write.previous.iter().zip(&write.changes) {
                work = work
                    .checked_add(1)
                    .filter(|n| *n <= 1_000_000)
                    .ok_or(JournalError::UndoWindowExceeded)?;
                if previous.property() != change.property() {
                    return Err(JournalError::Corrupt);
                }
                if !affected(write.address, change.property()) {
                    continue;
                }
                if event.principal != actor {
                    return Err(JournalError::UndoConflict);
                }
                let stack = stacks
                    .entry((write.address, change.property()))
                    .or_default();
                let cancels = stack.last().is_some_and(|last| {
                    inverse(&event.operation) == Some((last.id, last.version))
                        && previous == last.change
                        && change == last.previous
                });
                if cancels {
                    stack.pop();
                } else {
                    stack.push(Touch {
                        id: event.operation_id,
                        version: event.version,
                        previous,
                        change,
                    });
                }
            }
        }
    }
    if stacks.values().any(|s| !s.is_empty()) {
        Err(JournalError::UndoConflict)
    } else {
        Ok(())
    }
}

/// Removing an axis cannot erase a later foreign style write after its reset.
pub(super) fn validate_axis_styles(
    inverse: &Operation,
    principal: EventPrincipal,
    tail: &[CommittedEvent],
) -> Result<(), JournalError> {
    let Operation::AxisCompensation { change, .. } = inverse else {
        return Err(JournalError::Corrupt);
    };
    let (rows, columns): (BTreeSet<RowId>, BTreeSet<ColumnId>) = match &change.delta {
        AxisDelta::Rows { removed, .. } => (removed.iter().copied().collect(), BTreeSet::new()),
        AxisDelta::Columns { removed, .. } => (BTreeSet::new(), removed.iter().copied().collect()),
    };
    validate_history(principal, tail, |address, _| {
        address.sheet == change.sheet
            && (rows.contains(&address.row) || columns.contains(&address.column))
    })
}

async fn tail(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    from: DocumentVersion,
    head: DocumentVersion,
) -> Result<Vec<CommittedEvent>, JournalError> {
    let mut rows=sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
        uuid(document.as_bytes()),number(from.get())?,number(head.get())?
    ).fetch(&mut **tx);
    let mut events = Vec::new();
    let mut bytes = 0usize;
    let mut expected = from.get();
    while let Some(row) = rows.try_next().await.map_err(error)? {
        bytes = bytes
            .checked_add(row.payload.len())
            .filter(|n| *n <= MAX_UNDO_TAIL_BYTES)
            .ok_or(JournalError::UndoWindowExceeded)?;
        if events.len() >= UNDO_WINDOW as usize {
            return Err(JournalError::UndoWindowExceeded);
        }
        expected = expected.checked_add(1).ok_or(JournalError::Corrupt)?;
        let event = verified_event(StoredEventRow {
            document,
            committed_version: row.committed_version,
            operation_id: row.operation_id,
            actor_id: row.actor_id,
            principal_kind: &row.principal_kind,
            schema_version: row.schema_version,
            payload: &row.payload,
            payload_checksum: &row.payload_checksum,
            canonical_event_hash: &row.canonical_event_hash,
        })?;
        if event.version.get() != expected {
            return Err(JournalError::Corrupt);
        }
        events.push(event);
    }
    if expected != head.get() {
        return Err(JournalError::Corrupt);
    }
    Ok(events)
}

pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    request: &CommitRequest,
    head: DocumentVersion,
    target: Option<&CommittedEvent>,
) -> Result<(), JournalError> {
    let book = request
        .inverse_projection
        .as_deref()
        .ok_or(JournalError::UndoConflict)?;
    axis_guard::verified_projection(tx, request.command.document_id, head, book).await?;
    axis_guard::check_floors(book, &request.command)?;
    let expected = match (&request.event.operation, target) {
        (Operation::StyleWrite { .. }, None) => match &request.command.operation {
            CommandPayload::SetCellStyle { address, changes } => {
                prepare_style_write(book, &[*address], changes, StyleWriteKind::Cell)
            }
            CommandPayload::SetRangeStyle { addresses, changes } => {
                prepare_style_write(book, addresses, changes, StyleWriteKind::Range)
            }
            _ => return Err(JournalError::Corrupt),
        }
        .map_err(|_| JournalError::UndoConflict)?,
        (
            Operation::StyleCompensation {
                undoes,
                undoes_version,
                ..
            },
            Some(target),
        ) => {
            if target.operation_id != *undoes
                || target.version != *undoes_version
                || target.principal != EventPrincipal::User(request.identity.actor_id)
            {
                return Err(JournalError::UndoConflict);
            }
            let (kind, writes) =
                style_writes(&target.operation).ok_or(JournalError::NotUndoable)?;
            let tail = tail(tx, request.command.document_id, target.version, head).await?;
            let leaves: BTreeMap<_, _> = writes
                .iter()
                .flat_map(|w| {
                    w.changes
                        .iter()
                        .map(move |c| ((w.address, c.property()), ()))
                })
                .collect();
            validate_history(target.principal, &tail, |a, p| leaves.contains_key(&(a, p)))?;
            // Style identity cannot cross a deletion/revival unnoticed. Legacy and
            // frozen structural touches are conservative barriers in this slice.
            let mut structural_work = 0usize;
            for event in &tail {
                let mut removed = |sheet: SheetId,
                                   rows: &[RowId],
                                   columns: &[ColumnId]|
                 -> Result<bool, JournalError> {
                    structural_work = structural_work
                        .checked_add(leaves.len().saturating_mul(
                            rows.len().saturating_add(columns.len()).saturating_add(1),
                        ))
                        .filter(|n| *n <= 1_000_000)
                        .ok_or(JournalError::UndoWindowExceeded)?;
                    Ok(leaves.keys().any(|(a, _)| {
                        a.sheet == sheet && (rows.contains(&a.row) || columns.contains(&a.column))
                    }))
                };
                let intersects = match &event.operation {
                    Operation::DeleteRows { sheet, rows } => removed(*sheet, rows, &[])?,
                    Operation::DeleteColumns { sheet, columns } => removed(*sheet, &[], columns)?,
                    Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => {
                        match &change.delta {
                            AxisDelta::Rows { removed: rows, .. } => {
                                removed(change.sheet, rows, &[])?
                            }
                            AxisDelta::Columns {
                                removed: columns, ..
                            } => removed(change.sheet, &[], columns)?,
                        }
                    }
                    _ => false,
                };
                if intersects {
                    return Err(JournalError::UndoConflict);
                }
            }
            Operation::StyleCompensation {
                kind,
                writes: writes
                    .iter()
                    .map(|w| GuardedStyleWrite {
                        address: w.address,
                        previous: w.changes.clone(),
                        changes: w.previous.clone(),
                    })
                    .collect(),
                undoes: *undoes,
                undoes_version: *undoes_version,
            }
        }
        _ => return Err(JournalError::Corrupt),
    };
    if expected != request.event.operation {
        return Err(JournalError::UndoConflict);
    }
    let candidate = book
        .candidate(&expected, request.event.version)
        .map_err(|e| match e {
            DomainError::LimitExceeded => JournalError::PayloadTooLarge,
            _ => JournalError::UndoConflict,
        })?;
    if candidate.hash() != request.event.state_hash {
        return Err(JournalError::Corrupt);
    }
    Ok(())
}
