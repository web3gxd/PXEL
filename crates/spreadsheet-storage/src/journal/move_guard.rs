//! Reference guards bind an internal projection to the locked durable head.
use super::*;
use spreadsheet_domain::move_history::{MoveInverseError, plan_move_inverse};

pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target: &CommittedEvent,
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

    let mut rows = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
        uuid(document.as_bytes()), number(target.version.get())?, number(head.get())?
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
            document,
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
    plan_move_inverse(projection, target, &tail).map_err(|error| match error {
        MoveInverseError::Conflict | MoveInverseError::Domain(_) => JournalError::UndoConflict,
        MoveInverseError::NotUndoable => JournalError::NotUndoable,
        MoveInverseError::LimitExceeded => JournalError::UndoWindowExceeded,
        MoveInverseError::InvalidHistory => JournalError::Corrupt,
        MoveInverseError::Cancelled => JournalError::Unavailable,
    })?;
    Ok(())
}
