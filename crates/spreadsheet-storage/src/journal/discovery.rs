use super::*;
use spreadsheet_application::history::{self, HistoryTruncation, HistoryWindow, UndoHistory};

const PAGE_SIZE: i64 = 128;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

async fn reauthorize(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    document: DocumentId,
    initial: &DocumentAccess,
) -> Result<(), JournalError> {
    let current = db.access(identity, document).await?;
    if !current.can_edit
        || current.document_access_epoch != initial.document_access_epoch
        || current.membership_access_epoch != initial.membership_access_epoch
    {
        return Err(JournalError::Unauthorized);
    }
    if current.head < initial.head {
        return Err(JournalError::Corrupt);
    }
    Ok(())
}

pub(super) async fn discover(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<UndoHistory, JournalError> {
    let initial = db.access(identity, document).await?;
    let window = scan(db, identity, document, initial).await?;
    let history = history::reconstruct(window)?;
    reauthorize(db, identity, document, &initial).await?;
    Ok(history)
}

pub(super) async fn scan(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    document: DocumentId,
    initial: DocumentAccess,
) -> Result<HistoryWindow, JournalError> {
    if !initial.can_edit {
        return Err(JournalError::Unauthorized);
    }
    let head = initial.head.get();
    let floor = head.saturating_sub(UNDO_WINDOW).max(1);
    let mut expected = head;
    let mut bytes = 0usize;
    let mut entries = Vec::new();
    let mut truncation = (floor > 1).then_some(HistoryTruncation::VersionWindow);
    let mut byte_limit = false;
    while expected >= floor && head != 0 {
        reauthorize(db, identity, document, &initial).await?;
        let mut rows = sqlx::query!(
            "SELECT committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version<=$2 AND committed_version>=$3 ORDER BY committed_version DESC LIMIT $4",
            uuid(document.as_bytes()), number(expected)?, number(floor)?, PAGE_SIZE,
        ).fetch(&db.pool);
        let mut page_rows = 0usize;
        while let Some(row) = rows.try_next().await.map_err(error)? {
            if version(row.committed_version)?.get() != expected {
                return Err(JournalError::Corrupt);
            }
            if row.payload.len() > MAX_EVENT_BYTES {
                return Err(JournalError::Corrupt);
            }
            let next_bytes = bytes
                .checked_add(row.payload.len())
                .ok_or(JournalError::Corrupt)?;
            if next_bytes > MAX_UNDO_TAIL_BYTES {
                truncation = Some(HistoryTruncation::ByteBudget);
                byte_limit = true;
                break;
            }
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
            entries.push(history::describe(&event, identity.actor_id)?);
            bytes = next_bytes;
            expected = expected.checked_sub(1).ok_or(JournalError::Corrupt)?;
            page_rows += 1;
        }
        drop(rows);
        reauthorize(db, identity, document, &initial).await?;
        if byte_limit {
            break;
        }
        if page_rows == 0 {
            return Err(JournalError::Corrupt);
        }
    }
    entries.reverse();
    reauthorize(db, identity, document, &initial).await?;
    Ok(HistoryWindow {
        document_id: document,
        actor_id: identity.actor_id,
        through_version: initial.head,
        earliest_scanned_version: if head == 0 {
            DocumentVersion::ZERO
        } else {
            DocumentVersion::new(expected.checked_add(1).ok_or(JournalError::Corrupt)?)
                .map_err(|_| JournalError::Corrupt)?
        },
        entries,
        truncation,
    })
}
