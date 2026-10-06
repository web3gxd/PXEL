//! Independent durable validation for merged-range writes and their inverses.
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
    super::axis_guard::verified_projection(db, tx, request.command.document_id, head, book).await?;
    let (sheet, change, undoes, undoes_version) = match &request.event.operation {
        Operation::MergeWrite {
            sheet,
            change,
            undoes,
            undoes_version,
        } => (*sheet, *change, *undoes, *undoes_version),
        _ => return Err(JournalError::Corrupt),
    };
    match (target, undoes, undoes_version) {
        (None, None, None) => {
            let CommandPayload::SetMergeRange {
                sheet: command_sheet,
                range,
                merged,
            } = &request.command.operation
            else {
                return Err(JournalError::Corrupt);
            };
            if *command_sheet != sheet
                || *range != change.range
                || *merged != change.merged
                || change.previous == change.merged
            {
                return Err(JournalError::Corrupt);
            }
        }
        (Some(target), Some(id), Some(version)) => {
            let Operation::MergeWrite {
                sheet: target_sheet,
                change: target_change,
                ..
            } = &target.operation
            else {
                return Err(JournalError::UndoConflict);
            };
            if target.principal != request.event.principal
                || target.operation_id != id
                || target.version != version
                || sheet != *target_sheet
                || change.range != target_change.range
                || change.previous != target_change.merged
                || change.merged == target_change.merged
            {
                return Err(JournalError::UndoConflict);
            }
            ensure_latest_range_write(
                db,
                tx,
                request.command.document_id,
                target,
                head,
                sheet,
                change.range,
            )
            .await?;
        }
        _ => return Err(JournalError::Corrupt),
    }
    let expected = book
        .candidate(&request.event.operation, request.event.version)
        .map_err(|_| JournalError::UndoConflict)?;
    if expected.hash() != request.event.state_hash {
        return Err(JournalError::Corrupt);
    }
    codec::encode(&request.event).map_err(invalid)?;
    Ok(())
}

pub(super) async fn ensure_latest_range_write(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target: &CommittedEvent,
    head: DocumentVersion,
    sheet: SheetId,
    range: spreadsheet_domain::merge::MergeRange,
) -> Result<(), JournalError> {
    let mut rows = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
        uuid(document.as_bytes()), number(target.version.get())?, number(head.get())?
    )
    .fetch(&mut **tx);
    let mut count = 0usize;
    while let Some(row) = rows.try_next().await.map_err(error)? {
        count = count
            .checked_add(1)
            .filter(|n| *n <= UNDO_WINDOW as usize)
            .ok_or(JournalError::UndoWindowExceeded)?;
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
        if matches!(event.operation, Operation::MergeWrite { sheet: changed_sheet, change, .. } | Operation::MergeCellsWrite { sheet: changed_sheet, change, .. } if changed_sheet == sheet && change.range == range)
        {
            return Err(JournalError::UndoConflict);
        }
    }
    Ok(())
}
