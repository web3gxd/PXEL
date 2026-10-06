//! Owner-only, verified journal pages. This is not the author's Undo stack.
#[cfg(test)]
mod tests;
use crate::{PgDatabase, disclosure::DisclosureError, journal};
use futures_util::TryStreamExt;
use spreadsheet_application::journal::{AuthenticatedActor, JournalError};
use spreadsheet_domain::workbook::{CommittedEvent, DocumentId};
use sqlx::FromRow;
use uuid::Uuid;

pub struct ActivityEntry {
    pub created_at: i64,
    pub event: CommittedEvent,
}
pub struct ActivityPage {
    pub through_version: u64,
    pub entries: Vec<ActivityEntry>,
    pub next_before: Option<u64>,
}
#[derive(FromRow)]
struct Row {
    committed_version: i64,
    operation_id: Uuid,
    actor_id: Option<Uuid>,
    principal_kind: String,
    schema_version: i32,
    payload: Vec<u8>,
    payload_checksum: Vec<u8>,
    canonical_event_hash: Vec<u8>,
    created_at: i64,
}
fn error(e: DisclosureError) -> JournalError {
    match e {
        DisclosureError::Unauthorized => JournalError::Unauthorized,
        DisclosureError::Corrupt => JournalError::Corrupt,
        _ => JournalError::Unavailable,
    }
}
impl PgDatabase {
    pub async fn activity_page(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        through: u64,
        before: Option<u64>,
    ) -> Result<ActivityPage, JournalError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| JournalError::Unavailable)?;
        let document = self
            .disclosure_document(&mut tx, identity, doc, false)
            .await
            .map_err(error)?;
        self.require_disclosure_owner(&mut tx, identity, doc, None)
            .await
            .map_err(error)?;
        if through == 0
            || through > u64::try_from(document.head_version).map_err(|_| JournalError::Corrupt)?
            || before.is_some_and(|v| v < 2 || v > through + 1)
        {
            return Err(JournalError::Conflict);
        }
        tx.commit().await.map_err(|_| JournalError::Unavailable)?;
        let limit =
            i64::try_from(before.unwrap_or(through + 1)).map_err(|_| JournalError::Conflict)?;
        let mut rows = sqlx::query_as::<_,Row>("SELECT committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,canonical_event_hash,floor(extract(epoch FROM created_at)*1000)::bigint AS created_at FROM document_operations WHERE document_id=$1 AND committed_version<$2 ORDER BY committed_version DESC LIMIT 100")
            .bind(Uuid::from_bytes(*doc.as_bytes())).bind(limit).fetch(&self.pool);
        let mut expected = u64::try_from(limit - 1).map_err(|_| JournalError::Corrupt)?;
        let mut bytes = 0usize;
        let mut keys = crate::crypto::ReadKeyCache::default();
        let mut entries = Vec::new();
        while let Some(row) = rows
            .try_next()
            .await
            .map_err(|_| JournalError::Unavailable)?
        {
            if u64::try_from(row.committed_version).map_err(|_| JournalError::Corrupt)? != expected
            {
                return Err(JournalError::Corrupt);
            }
            bytes = bytes
                .checked_add(row.payload.len())
                .filter(|n| *n <= 32 * 1024 * 1024)
                .ok_or(JournalError::PayloadTooLarge)?;
            let event = journal::verified_event_cached(
                self,
                &mut keys,
                journal::StoredEventRow {
                    document: doc,
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
            entries.push(ActivityEntry {
                created_at: row.created_at,
                event,
            });
            expected -= 1;
        }
        drop(rows);
        if expected > 0 && entries.len() < 100 {
            return Err(JournalError::Corrupt);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| JournalError::Unavailable)?;
        self.disclosure_document(&mut tx, identity, doc, false)
            .await
            .map_err(error)?;
        self.require_disclosure_owner(&mut tx, identity, doc, None)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(|_| JournalError::Unavailable)?;
        Ok(ActivityPage {
            through_version: through,
            next_before: (expected > 0).then_some(expected + 1),
            entries,
        })
    }
}
