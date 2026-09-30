//! Axis admission derives frozen edits from the independently verified locked head.
use super::*;
use spreadsheet_domain::axis_edit::{AxisEditError, prepare_axis_edit, prepare_axis_inverse};
use spreadsheet_domain::axis_history::{AxisHistoryError, validate_axis_inverse};

pub(super) fn legacy(command: &CommandPayload) -> Result<Operation, JournalError> {
    Ok(match command {
        CommandPayload::InsertRows {
            sheet,
            before,
            rows,
        } => Operation::InsertRowsRebased {
            sheet: *sheet,
            before: *before,
            rows: rows.clone(),
        },
        CommandPayload::DeleteRows { sheet, rows } => Operation::DeleteRows {
            sheet: *sheet,
            rows: rows.clone(),
        },
        CommandPayload::InsertColumns {
            sheet,
            before,
            columns,
        } => Operation::InsertColumnsRebased {
            sheet: *sheet,
            before: *before,
            columns: columns.clone(),
        },
        CommandPayload::DeleteColumns { sheet, columns } => Operation::DeleteColumns {
            sheet: *sheet,
            columns: columns.clone(),
        },
        _ => return Err(JournalError::Corrupt),
    })
}
pub(super) async fn verified_projection(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    head: DocumentVersion,
    projection: &Workbook,
) -> Result<(), JournalError> {
    if projection.document_id() != document || projection.version() != head {
        return Err(JournalError::UndoConflict);
    }
    let row = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version=$2",
        uuid(document.as_bytes()), number(head.get())?
    ).fetch_optional(&mut **tx).await.map_err(error)?.ok_or(JournalError::Corrupt)?;
    let terminal = verified_event(StoredEventRow {
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
    if projection.hash() != terminal.state_hash {
        return Err(JournalError::UndoConflict);
    }
    Ok(())
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
        .ok_or(JournalError::Corrupt)?;
    verified_projection(tx, request.command.document_id, head, book).await?;
    let expected = match (&request.event.operation, target) {
        (Operation::AxisEdit { .. }, None) => prepare_axis_edit(
            book,
            &legacy(&request.command.operation)?,
            request.event.version,
        ),
        (Operation::AxisCompensation { .. }, Some(target)) => {
            prepare_axis_inverse(book, target, request.event.version)
        }
        _ => return Err(JournalError::Corrupt),
    }
    .map_err(axis_error)?;
    if expected != request.event.operation {
        return Err(JournalError::UndoConflict);
    }
    let candidate = book
        .candidate(&expected, request.event.version)
        .map_err(domain_error)?;
    if candidate.hash() != request.event.state_hash {
        return Err(JournalError::Corrupt);
    }
    codec::encode(&request.event).map_err(invalid)?;
    if let Some(target) = target {
        let mut rows = sqlx::query!(
            "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
            uuid(request.command.document_id.as_bytes()), number(target.version.get())?, number(head.get())?
        ).fetch(&mut **tx);
        let mut tail = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.try_next().await.map_err(error)? {
            bytes = bytes
                .checked_add(row.payload.len())
                .filter(|bytes| *bytes <= MAX_UNDO_TAIL_BYTES)
                .ok_or(JournalError::UndoWindowExceeded)?;
            if tail.len() >= UNDO_WINDOW as usize {
                return Err(JournalError::UndoWindowExceeded);
            }
            tail.push(verified_event(StoredEventRow {
                document: request.command.document_id,
                committed_version: row.committed_version,
                operation_id: row.operation_id,
                actor_id: row.actor_id,
                principal_kind: &row.principal_kind,
                schema_version: row.schema_version,
                payload: &row.payload,
                payload_checksum: &row.payload_checksum,
                canonical_event_hash: &row.canonical_event_hash,
            })?);
        }
        drop(rows);
        validate_axis_inverse(book, target, &tail, &candidate).map_err(history_error)?;
    } else {
        // Reject an authored edit before its durable commit if its immediate
        // genuine inverse cannot fit the immutable persisted envelope limits.
        let next = DocumentVersion::new(
            request
                .event
                .version
                .get()
                .checked_add(1)
                .ok_or(JournalError::Corrupt)?,
        )
        .map_err(domain_error)?;
        number(next.get())?;
        let inverse = prepare_axis_inverse(&candidate, &request.event, next).map_err(axis_error)?;
        let inverse_candidate = candidate.candidate(&inverse, next).map_err(domain_error)?;
        let inverse_event = CommittedEvent {
            operation_id: OperationId::from_bytes(
                if request.event.operation_id.as_bytes() == &[255; 16] {
                    [254; 16]
                } else {
                    [255; 16]
                },
            )
            .map_err(|_| JournalError::Corrupt)?,
            version: next,
            operation: inverse,
            state_hash: inverse_candidate.hash(),
            ..request.event.clone()
        };
        let inverse_bytes = codec::encode(&inverse_event).map_err(invalid)?;
        // A later committed version can require a longer varint. Reserve its
        // bounded envelope metadata growth without weakening the 4MiB cap.
        if inverse_bytes.len().saturating_add(32) > 4 * 1024 * 1024 {
            return Err(JournalError::PayloadTooLarge);
        }
    }
    Ok(())
}

pub(super) fn check_floors(book: &Workbook, command: &Command) -> Result<(), JournalError> {
    spreadsheet_domain::lifecycle::check_command_activation(book, command)
        .map_err(|_| JournalError::Conflict)
}
pub(super) fn check_formulas(
    book: &Workbook,
    command: &Command,
    operation: &Operation,
) -> Result<(), JournalError> {
    let formula = match operation {
        Operation::SetCellFormula { formula, .. } => Some(formula),
        Operation::CellWrite {
            kind: Some(CellWriteKind::SetFormula),
            content: Some(CellContent::Formula(formula)),
            ..
        } => Some(formula),
        _ => None,
    };
    if let Some(formula) = formula {
        spreadsheet_domain::lifecycle::check_formula_activation(
            book,
            book,
            formula,
            command.base_version,
        )
        .map_err(|_| JournalError::Conflict)?;
    }
    Ok(())
}
fn axis_error(error: AxisEditError) -> JournalError {
    match error {
        AxisEditError::NotUndoable => JournalError::NotUndoable,
        AxisEditError::Domain(error) => domain_error(error),
    }
}
fn domain_error(error: DomainError) -> JournalError {
    match error {
        DomainError::LimitExceeded => JournalError::PayloadTooLarge,
        _ => JournalError::UndoConflict,
    }
}
fn history_error(error: AxisHistoryError) -> JournalError {
    match error {
        AxisHistoryError::NotUndoable => JournalError::NotUndoable,
        AxisHistoryError::Conflict | AxisHistoryError::Domain(_) => JournalError::UndoConflict,
        AxisHistoryError::InvalidHistory => JournalError::Corrupt,
        AxisHistoryError::LimitExceeded => JournalError::UndoWindowExceeded,
        AxisHistoryError::Cancelled => JournalError::Unavailable,
    }
}
