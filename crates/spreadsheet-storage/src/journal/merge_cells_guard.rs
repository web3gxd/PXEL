//! Provenance and durable preimages for clearing merged cells.
use super::*;

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
        .ok_or(JournalError::Corrupt)?;
    axis_guard::verified_projection(db, tx, request.command.document_id, head, book).await?;
    let Operation::MergeCellsWrite {
        sheet,
        change,
        undoes,
        undoes_version,
        ..
    } = &request.event.operation
    else {
        return Err(JournalError::Corrupt);
    };
    let expected = match (target, undoes, undoes_version) {
        (None, None, None) => {
            if request.command.base_version != head {
                return Err(JournalError::Conflict);
            }
            if request.command.operation
                != (CommandPayload::MergeCells {
                    sheet: *sheet,
                    range: change.range,
                })
            {
                return Err(JournalError::Corrupt);
            }
            spreadsheet_domain::merge::prepare_merge_cells(book, *sheet, change.range)
                .map_err(|_| JournalError::Corrupt)?
        }
        (Some(target), Some(id), Some(version)) => {
            if target.principal != request.event.principal
                || target.operation_id != *id
                || target.version != *version
            {
                return Err(JournalError::UndoConflict);
            }
            merge_guard::ensure_latest_range_write(
                db,
                tx,
                request.command.document_id,
                target,
                head,
                *sheet,
                change.range,
            )
            .await?;
            guard_undo_tail(db, tx, request.command.document_id, target, head).await?;
            style_guard::validate_merge_history(db, tx, request.command.document_id, target, head)
                .await?;
            spreadsheet_domain::merge::inverse_merge_cells(&target.operation, *id, *version)
                .map_err(|_| JournalError::UndoConflict)?
        }
        _ => return Err(JournalError::Corrupt),
    };
    if expected != request.event.operation {
        return Err(JournalError::UndoConflict);
    }
    let candidate = book
        .candidate(&expected, request.event.version)
        .map_err(|_| JournalError::UndoConflict)?;
    if candidate.hash() != request.event.state_hash {
        return Err(JournalError::Corrupt);
    }
    codec::encode(&request.event).map_err(invalid)?;
    Ok(())
}
