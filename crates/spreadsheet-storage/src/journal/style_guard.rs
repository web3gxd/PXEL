//! Independently verify property-level style intent and durable inverse history.
use super::*;
use spreadsheet_domain::{
    style::{CellStyle, StyleChange, StyleProperty},
    style_write::prepare_style_write,
};
use std::collections::{BTreeMap, BTreeSet};

fn style_writes(operation: &Operation) -> Option<(StyleWriteKind, &[GuardedStyleWrite])> {
    match operation {
        Operation::StyleWrite { kind, writes }
        | Operation::StyleCompensation { kind, writes, .. } => Some((*kind, writes)),
        Operation::CellBatchWrite { style_writes, .. }
        | Operation::MergeCellsWrite { style_writes, .. }
            if !style_writes.is_empty() =>
        {
            Some((StyleWriteKind::Range, style_writes))
        }
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
        Operation::CellBatchWrite {
            undoes: Some(undoes),
            undoes_version: Some(version),
            ..
        }
        | Operation::MergeCellsWrite {
            undoes: Some(undoes),
            undoes_version: Some(version),
            ..
        } => Some((*undoes, *version)),
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

fn copy_style_writes(
    book: &Workbook,
    source: CellAddress,
    destination: CellAddress,
    rows: u32,
    columns: u32,
) -> Result<Vec<GuardedStyleWrite>, JournalError> {
    let count = rows
        .checked_mul(columns)
        .filter(|count| *count > 0 && *count <= 1_000)
        .ok_or(JournalError::Corrupt)?;
    if source.sheet != destination.sheet {
        return Err(JournalError::Corrupt);
    }
    let sheet = book
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == source.sheet)
        .ok_or(JournalError::UndoConflict)?;
    let source_row = sheet
        .row_position(source.row)
        .ok_or(JournalError::UndoConflict)?;
    let source_column = sheet
        .column_position(source.column)
        .ok_or(JournalError::UndoConflict)?;
    let destination_row = sheet
        .row_position(destination.row)
        .ok_or(JournalError::UndoConflict)?;
    let destination_column = sheet
        .column_position(destination.column)
        .ok_or(JournalError::UndoConflict)?;
    let properties = [
        StyleProperty::FontFamily,
        StyleProperty::FontSize,
        StyleProperty::FontBold,
        StyleProperty::FontItalic,
        StyleProperty::FontUnderline,
        StyleProperty::FontStrike,
        StyleProperty::FontColor,
        StyleProperty::Fill,
        StyleProperty::BorderTop,
        StyleProperty::BorderRight,
        StyleProperty::BorderBottom,
        StyleProperty::BorderLeft,
        StyleProperty::Horizontal,
        StyleProperty::Vertical,
        StyleProperty::TextOverflow,
        StyleProperty::NumberFormat,
    ];
    let mut writes = Vec::with_capacity(count as usize);
    for row in 0..rows {
        for column in 0..columns {
            let source_address = CellAddress {
                sheet: source.sheet,
                row: *sheet
                    .rows()
                    .get((source_row + row) as usize)
                    .ok_or(JournalError::UndoConflict)?,
                column: *sheet
                    .columns()
                    .get((source_column + column) as usize)
                    .ok_or(JournalError::UndoConflict)?,
            };
            let destination_address = CellAddress {
                sheet: destination.sheet,
                row: *sheet
                    .rows()
                    .get((destination_row + row) as usize)
                    .ok_or(JournalError::UndoConflict)?,
                column: *sheet
                    .columns()
                    .get((destination_column + column) as usize)
                    .ok_or(JournalError::UndoConflict)?,
            };
            let source_default = CellStyle::default();
            let destination_default = CellStyle::default();
            let source_style = sheet
                .styles()
                .get(&source_address)
                .unwrap_or(&source_default);
            let destination_style = sheet
                .styles()
                .get(&destination_address)
                .unwrap_or(&destination_default);
            writes.push(GuardedStyleWrite {
                address: destination_address,
                previous: properties
                    .iter()
                    .map(|property| destination_style.change(*property))
                    .collect(),
                changes: properties
                    .iter()
                    .map(|property| source_style.change(*property))
                    .collect(),
            });
        }
    }
    writes.sort_by_key(|write| write.address);
    Ok(writes)
}

fn move_style_writes(
    book: &Workbook,
    source: CellAddress,
    destination: CellAddress,
    rows: u32,
    columns: u32,
) -> Result<Vec<GuardedStyleWrite>, JournalError> {
    let mut writes = copy_style_writes(book, source, destination, rows, columns)?;
    let destinations: BTreeSet<_> = writes.iter().map(|write| write.address).collect();
    let count = rows
        .checked_mul(columns)
        .filter(|n| *n > 0 && *n <= 1_000)
        .ok_or(JournalError::Corrupt)?;
    let sheet = book
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == source.sheet)
        .ok_or(JournalError::UndoConflict)?;
    let source_row = sheet
        .row_position(source.row)
        .ok_or(JournalError::UndoConflict)?;
    let source_column = sheet
        .column_position(source.column)
        .ok_or(JournalError::UndoConflict)?;
    let properties = [
        StyleProperty::FontFamily,
        StyleProperty::FontSize,
        StyleProperty::FontBold,
        StyleProperty::FontItalic,
        StyleProperty::FontUnderline,
        StyleProperty::FontStrike,
        StyleProperty::FontColor,
        StyleProperty::Fill,
        StyleProperty::BorderTop,
        StyleProperty::BorderRight,
        StyleProperty::BorderBottom,
        StyleProperty::BorderLeft,
        StyleProperty::Horizontal,
        StyleProperty::Vertical,
        StyleProperty::TextOverflow,
        StyleProperty::NumberFormat,
    ];
    let default = CellStyle::default();
    for index in 0..count {
        let row = source_row + index / columns;
        let column = source_column + index % columns;
        let address = CellAddress {
            sheet: source.sheet,
            row: *sheet
                .rows()
                .get(row as usize)
                .ok_or(JournalError::UndoConflict)?,
            column: *sheet
                .columns()
                .get(column as usize)
                .ok_or(JournalError::UndoConflict)?,
        };
        if destinations.contains(&address) {
            continue;
        }
        let previous = sheet.styles().get(&address).cloned().unwrap_or_default();
        writes.push(GuardedStyleWrite {
            address,
            previous: properties
                .iter()
                .map(|property| previous.change(*property))
                .collect(),
            changes: properties
                .iter()
                .map(|property| default.change(*property))
                .collect(),
        });
    }
    writes.sort_by_key(|write| write.address);
    Ok(writes)
}

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
    db: &PgDatabase,
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
            .checked_add(encrypted_plaintext_length(row.payload.len())?)
            .filter(|n| *n <= MAX_UNDO_TAIL_BYTES)
            .ok_or(JournalError::UndoWindowExceeded)?;
        if events.len() >= UNDO_WINDOW as usize {
            return Err(JournalError::UndoWindowExceeded);
        }
        expected = expected.checked_add(1).ok_or(JournalError::Corrupt)?;
        let event = verified_event(
            db,
            StoredEventRow {
                document,
                committed_version: row.committed_version,
                operation_id: row.operation_id,
                actor_id: row.actor_id,
                principal_kind: &row.principal_kind,
                schema_version: row.schema_version,
                payload: &row.payload,
                payload_checksum: &row.payload_checksum,
                canonical_event_hash: &row.canonical_event_hash,
            },
        )
        .await?;
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

pub(super) async fn validate_merge_history(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target: &CommittedEvent,
    head: DocumentVersion,
) -> Result<(), JournalError> {
    let Operation::MergeCellsWrite { style_writes, .. } = &target.operation else {
        return Err(JournalError::Corrupt);
    };
    let leaves: BTreeSet<_> = style_writes
        .iter()
        .flat_map(|w| w.changes.iter().map(move |p| (w.address, p.property())))
        .collect();
    let events = tail(db, tx, document, target.version, head).await?;
    validate_history(target.principal, &events, |address, property| {
        leaves.contains(&(address, property))
    })
}

pub(super) async fn validate(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    request: &CommitRequest,
    head: DocumentVersion,
    target: Option<&CommittedEvent>,
) -> Result<(), JournalError> {
    let book = request
        .inverse_projection
        .as_deref()
        .ok_or(JournalError::UndoConflict)?;
    axis_guard::verified_projection(db, tx, request.command.document_id, head, book).await?;
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
            Operation::CellBatchWrite {
                intent,
                content_writes,
                ..
            },
            None,
        ) => match intent {
            CellBatchIntent::ShiftCells { .. } => {
                book.candidate(&request.event.operation, request.event.version)
                    .map_err(|_| JournalError::Corrupt)?;
                request.event.operation.clone()
            }
            CellBatchIntent::SortRange { range, order } => {
                let CommandPayload::SortRange { range: selected } = &request.command.operation
                else {
                    return Err(JournalError::Corrupt);
                };
                if range != selected || request.command.base_version != head {
                    return Err(JournalError::Corrupt);
                }
                let bounds = range
                    .validate_order(book, order)
                    .map_err(|_| JournalError::UndoConflict)?;
                book.candidate(&request.event.operation, request.event.version)
                    .map_err(|_| JournalError::UndoConflict)?;
                let sheet = book
                    .sheets()
                    .iter()
                    .find(|s| s.id() == range.source.sheet)
                    .ok_or(JournalError::Corrupt)?;
                let expected_content = content_writes
                    .iter()
                    .map(|write| {
                        let destination = sheet
                            .row_position(write.address.row)
                            .ok_or(JournalError::Corrupt)?;
                        let offset = destination
                            .checked_sub(bounds.row + range.header_rows)
                            .ok_or(JournalError::Corrupt)?;
                        let source = CellAddress {
                            row: *order.get(offset as usize).ok_or(JournalError::Corrupt)?,
                            ..write.address
                        };
                        let content = match book
                            .cell_content(source)
                            .map_err(|_| JournalError::Corrupt)?
                        {
                            Some(CellContent::Formula(_)) => match &write.content {
                                Some(CellContent::Formula(_)) => write.content.clone(),
                                _ => return Err(JournalError::Corrupt),
                            },
                            content => content.cloned(),
                        };
                        Ok(GuardedContentWrite {
                            address: write.address,
                            content,
                            previous: book
                                .cell_content(write.address)
                                .map_err(|_| JournalError::Corrupt)?
                                .cloned(),
                        })
                    })
                    .collect::<Result<Vec<_>, JournalError>>()?;
                Operation::CellBatchWrite {
                    intent: intent.clone(),
                    content_writes: expected_content,
                    style_writes: spreadsheet_domain::sort::style_writes(book, range, order)
                        .map_err(|_| JournalError::UndoConflict)?,
                    undoes: None,
                    undoes_version: None,
                }
            }
            CellBatchIntent::SetValues => {
                let CommandPayload::SetCellBatch { writes, styles } = &request.command.operation
                else {
                    return Err(JournalError::Corrupt);
                };
                let mut expected_content = Vec::with_capacity(writes.len());
                for write in writes {
                    let value = write
                        .value
                        .clone()
                        .normalized()
                        .map_err(|_| JournalError::UndoConflict)?;
                    expected_content.push(GuardedContentWrite {
                        address: write.address,
                        content: (value != CellValue::Blank).then_some(CellContent::Value(value)),
                        previous: book
                            .cell_content(write.address)
                            .map_err(|_| JournalError::UndoConflict)?
                            .cloned(),
                    });
                }
                let mut style_writes = Vec::with_capacity(styles.len());
                for (address, changes) in styles {
                    let Operation::StyleWrite { writes, .. } =
                        prepare_style_write(book, &[*address], changes, StyleWriteKind::Cell)
                            .map_err(|_| JournalError::UndoConflict)?
                    else {
                        return Err(JournalError::Corrupt);
                    };
                    style_writes.extend(writes);
                }
                style_writes.sort_by_key(|write| write.address);
                Operation::CellBatchWrite {
                    intent: intent.clone(),
                    content_writes: expected_content,
                    style_writes,
                    undoes: None,
                    undoes_version: None,
                }
            }
            CellBatchIntent::CopyRange {
                source,
                destination,
                rows,
                columns,
            } => {
                let CommandPayload::CopyRange {
                    source: command_source,
                    destination: command_destination,
                    rows: command_rows,
                    columns: command_columns,
                    repeat: None,
                } = &request.command.operation
                else {
                    return Err(JournalError::Corrupt);
                };
                if (source, destination, rows, columns)
                    != (
                        command_source,
                        command_destination,
                        command_rows,
                        command_columns,
                    )
                {
                    return Err(JournalError::Corrupt);
                }
                let expected_styles =
                    copy_style_writes(book, *source, *destination, *rows, *columns)?;
                let expected_content = content_writes
                    .iter()
                    .map(|write| {
                        Ok(GuardedContentWrite {
                            address: write.address,
                            content: write.content.clone(),
                            previous: book
                                .cell_content(write.address)
                                .map_err(|_| JournalError::UndoConflict)?
                                .cloned(),
                        })
                    })
                    .collect::<Result<Vec<_>, JournalError>>()?;
                Operation::CellBatchWrite {
                    intent: intent.clone(),
                    content_writes: expected_content,
                    style_writes: expected_styles,
                    undoes: None,
                    undoes_version: None,
                }
            }
            CellBatchIntent::MoveRange {
                source,
                destination,
                rows,
                columns,
            } => {
                let CommandPayload::MoveRange {
                    source: command_source,
                    destination: command_destination,
                    rows: command_rows,
                    columns: command_columns,
                } = &request.command.operation
                else {
                    return Err(JournalError::Corrupt);
                };
                if (source, destination, rows, columns)
                    != (
                        command_source,
                        command_destination,
                        command_rows,
                        command_columns,
                    )
                {
                    return Err(JournalError::Corrupt);
                }
                let Operation::CellBatchWrite { .. } = &request.event.operation else {
                    return Err(JournalError::Corrupt);
                };
                book.candidate(&request.event.operation, request.event.version)
                    .map_err(|_| JournalError::UndoConflict)?;
                let expected_content = content_writes
                    .iter()
                    .map(|write| {
                        Ok(GuardedContentWrite {
                            address: write.address,
                            content: write.content.clone(),
                            previous: book
                                .cell_content(write.address)
                                .map_err(|_| JournalError::UndoConflict)?
                                .cloned(),
                        })
                    })
                    .collect::<Result<Vec<_>, JournalError>>()?;
                Operation::CellBatchWrite {
                    intent: intent.clone(),
                    content_writes: expected_content,
                    style_writes: move_style_writes(book, *source, *destination, *rows, *columns)?,
                    undoes: None,
                    undoes_version: None,
                }
            }
        },
        (Operation::CellBatchWrite { .. }, Some(target)) => {
            if target.operation_id
                != match &request.event.operation {
                    Operation::CellBatchWrite {
                        undoes: Some(id), ..
                    } => *id,
                    _ => return Err(JournalError::Corrupt),
                }
                || target.principal != EventPrincipal::User(request.identity.actor_id)
            {
                return Err(JournalError::UndoConflict);
            }
            let Operation::CellBatchWrite {
                intent,
                content_writes,
                style_writes,
                ..
            } = &target.operation
            else {
                return Err(JournalError::NotUndoable);
            };
            let tail = tail(db, tx, request.command.document_id, target.version, head).await?;
            let leaves: BTreeSet<_> = style_writes
                .iter()
                .flat_map(|write| {
                    write
                        .changes
                        .iter()
                        .map(move |change| (write.address, change.property()))
                })
                .collect();
            validate_history(target.principal, &tail, |address, property| {
                leaves.contains(&(address, property))
            })?;
            Operation::CellBatchWrite {
                intent: intent.clone(),
                content_writes: content_writes
                    .iter()
                    .map(|write| GuardedContentWrite {
                        address: write.address,
                        content: write.previous.clone(),
                        previous: write.content.clone(),
                    })
                    .collect(),
                style_writes: style_writes
                    .iter()
                    .map(|write| GuardedStyleWrite {
                        address: write.address,
                        previous: write.changes.clone(),
                        changes: write.previous.clone(),
                    })
                    .collect(),
                undoes: Some(target.operation_id),
                undoes_version: Some(target.version),
            }
        }
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
            let tail = tail(db, tx, request.command.document_id, target.version, head).await?;
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
