use crate::{CryptoError, PgDatabase};
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::{AuthenticatedActor, JournalError};
use spreadsheet_domain::workbook::DocumentId;
use sqlx::{FromRow, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
mod tests;

const MAX_TEXT_BYTES: usize = 4096;
const MAX_PAGE: u32 = 100;
const MAX_EVENTS: i64 = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommentAddress {
    pub sheet_id: Uuid,
    pub row_id: Uuid,
    pub column_id: Uuid,
}

pub struct CommentEntry {
    pub comment_id: Uuid,
    pub author_id: Uuid,
    pub address: CommentAddress,
    pub text: Zeroizing<String>,
    pub revision: u64,
    pub can_delete: bool,
}

pub struct CommentPage {
    pub revision: u64,
    pub comments: Vec<CommentEntry>,
    pub next_cursor: Option<u64>,
}

#[derive(Debug, Error)]
pub enum CommentError {
    #[error("comment input is invalid")]
    InvalidInput,
    #[error("comment access denied")]
    Unauthorized,
    #[error("comment or document not found")]
    NotFound,
    #[error("comment request identifier conflicts")]
    IdempotencyConflict,
    #[error("comment storage is unavailable")]
    Unavailable,
    #[error("comment storage is corrupt")]
    Corrupt,
}
impl From<sqlx::Error> for CommentError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
impl From<CryptoError> for CommentError {
    fn from(value: CryptoError) -> Self {
        match value {
            CryptoError::Database(_) | CryptoError::RestorePending => Self::Unavailable,
            _ => Self::Corrupt,
        }
    }
}
fn journal_error(value: JournalError) -> CommentError {
    match value {
        JournalError::Unauthorized => CommentError::Unauthorized,
        JournalError::NotFound => CommentError::NotFound,
        JournalError::Corrupt => CommentError::Corrupt,
        _ => CommentError::Unavailable,
    }
}
fn id(bytes: &[u8]) -> Result<Uuid, CommentError> {
    Uuid::from_slice(bytes).map_err(|_| CommentError::Corrupt)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    comment_id: Uuid,
    actor_id: Uuid,
    sheet_id: Uuid,
    row_id: Uuid,
    column_id: Uuid,
    text: String,
}

async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
    write: bool,
    target_author: Option<Uuid>,
) -> Result<bool, CommentError> {
    let kind = sqlx::query_scalar::<_, String>("SELECT identity_kind FROM users WHERE user_id=$1")
        .bind(id(identity.actor_id.as_bytes())?)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(CommentError::Unauthorized)?;
    if kind == "share_guest" {
        let access = crate::shares::guest_access_locked(tx, identity, document)
            .await
            .map_err(journal_error)?;
        if write && !access.can_edit {
            let allowed = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM share_guest_sessions s JOIN share_links l ON l.link_id=s.link_id WHERE s.user_id=$1 AND s.document_id=$2 AND l.role='comment')")
                .bind(id(identity.actor_id.as_bytes())?).bind(id(document.as_bytes())?).fetch_one(&mut **tx).await?;
            if !allowed {
                return Err(CommentError::Unauthorized);
            }
        }
        return Ok(target_author == Some(id(identity.actor_id.as_bytes())?));
    }
    if kind != "member" {
        return Err(CommentError::Unauthorized);
    }
    let role = sqlx::query_scalar::<_, String>("SELECT p.role FROM memberships m JOIN document_permissions p ON p.workspace_id=m.workspace_id AND p.user_id=m.user_id WHERE m.user_id=$1 AND m.workspace_id=$2 AND p.document_id=$3")
        .bind(id(identity.actor_id.as_bytes())?).bind(id(identity.workspace_id.as_bytes())?).bind(id(document.as_bytes())?).fetch_optional(&mut **tx).await?.ok_or(CommentError::Unauthorized)?;
    if write && role == "viewer" {
        return Err(CommentError::Unauthorized);
    }
    Ok(role == "owner" || target_author == Some(id(identity.actor_id.as_bytes())?))
}

#[derive(FromRow)]
struct EventRow {
    event_id: Uuid,
    revision: i64,
    comment_id: Uuid,
    sheet_id: Uuid,
    row_id: Uuid,
    column_id: Uuid,
    actor_id: Uuid,
    payload: Vec<u8>,
}

impl PgDatabase {
    pub async fn add_comment(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        request_id: Uuid,
        address: CommentAddress,
        text: Zeroizing<String>,
    ) -> Result<(), CommentError> {
        if request_id.is_nil()
            || address.sheet_id.is_nil()
            || address.row_id.is_nil()
            || address.column_id.is_nil()
            || text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || text.chars().count() > MAX_TEXT_BYTES
            || text.chars().any(|c| c == '\0')
        {
            return Err(CommentError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let doc = sqlx::query_scalar::<_, Uuid>("SELECT document_id FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR UPDATE")
            .bind(id(document.as_bytes())?).bind(id(identity.workspace_id.as_bytes())?).fetch_optional(&mut *tx).await?.ok_or(CommentError::NotFound)?;
        authorize(&mut tx, identity, document, true, None).await?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM document_comment_events WHERE document_id=$1",
        )
        .bind(doc)
        .fetch_one(&mut *tx)
        .await?;
        if count >= MAX_EVENTS {
            return Err(CommentError::InvalidInput);
        }
        let prior = sqlx::query_as::<_, EventRow>("SELECT event_id,revision,comment_id,event_kind,sheet_id,row_id,column_id,actor_id,payload FROM document_comment_events WHERE document_id=$1 AND actor_id=$2 AND request_id=$3")
            .bind(doc).bind(id(identity.actor_id.as_bytes())?).bind(request_id).fetch_optional(&mut *tx).await?;
        let comment_id = request_id;
        let mut payload = Payload {
            kind: "add".into(),
            comment_id,
            actor_id: id(identity.actor_id.as_bytes())?,
            sheet_id: address.sheet_id,
            row_id: address.row_id,
            column_id: address.column_id,
            text: text.to_string(),
        };
        let mut plaintext =
            Zeroizing::new(serde_json::to_vec(&payload).map_err(|_| CommentError::InvalidInput)?);
        payload.text.zeroize();
        if let Some(prior) = prior {
            let old = self
                .open_record(
                    document,
                    u64::try_from(prior.revision).map_err(|_| CommentError::Corrupt)?,
                    "comment",
                    prior.event_id,
                    1,
                    &prior.payload,
                    16 * 1024,
                )
                .await?;
            if old.as_slice() != plaintext.as_slice() {
                return Err(CommentError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(());
        }
        let revision = next_revision(&mut tx, doc).await?;
        let event_id = request_id;
        let sealed = self
            .seal_record_guarded(
                document,
                revision,
                "comment",
                event_id,
                1,
                &plaintext,
                16 * 1024,
            )
            .await?;
        plaintext.zeroize();
        sqlx::query("INSERT INTO document_comment_events(document_id,revision,event_id,request_id,comment_id,event_kind,sheet_id,row_id,column_id,actor_id,payload) VALUES($1,$2,$3,$3,$4,'add',$5,$6,$7,$8,$9)")
            .bind(doc).bind(i64::try_from(revision).map_err(|_| CommentError::Corrupt)?).bind(event_id).bind(comment_id).bind(address.sheet_id).bind(address.row_id).bind(address.column_id).bind(id(identity.actor_id.as_bytes())?).bind(sealed).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_comment(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        request_id: Uuid,
        comment_id: Uuid,
    ) -> Result<CommentAddress, CommentError> {
        if request_id.is_nil() || comment_id.is_nil() {
            return Err(CommentError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let doc = sqlx::query_scalar::<_, Uuid>("SELECT document_id FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR UPDATE")
            .bind(id(document.as_bytes())?).bind(id(identity.workspace_id.as_bytes())?).fetch_optional(&mut *tx).await?.ok_or(CommentError::NotFound)?;
        let duplicate = sqlx::query_as::<_, (Uuid, String, Uuid, Uuid, Uuid)>("SELECT comment_id,event_kind,sheet_id,row_id,column_id FROM document_comment_events WHERE document_id=$1 AND actor_id=$2 AND request_id=$3")
            .bind(doc).bind(id(identity.actor_id.as_bytes())?).bind(request_id).fetch_optional(&mut *tx).await?;
        if let Some((prior_comment, prior_kind, sheet, row, column)) = duplicate {
            if prior_comment != comment_id || prior_kind != "delete" {
                return Err(CommentError::IdempotencyConflict);
            }
            let address = CommentAddress {
                sheet_id: sheet,
                row_id: row,
                column_id: column,
            };
            tx.commit().await?;
            return Ok(address);
        }
        let current = sqlx::query_as::<_, (String, Uuid, Uuid, Uuid, Uuid)>("SELECT event_kind,actor_id,sheet_id,row_id,column_id FROM document_comment_events WHERE document_id=$1 AND comment_id=$2 ORDER BY revision DESC LIMIT 1")
            .bind(doc).bind(comment_id).fetch_optional(&mut *tx).await?.ok_or(CommentError::NotFound)?;
        if current.0 != "add" {
            return Err(CommentError::NotFound);
        }
        let owner = authorize(&mut tx, identity, document, true, Some(current.1)).await?;
        if !owner && current.1 != id(identity.actor_id.as_bytes())? {
            return Err(CommentError::Unauthorized);
        }
        let revision = next_revision(&mut tx, doc).await?;
        let event_id = request_id;
        let payload = Payload {
            kind: "delete".into(),
            comment_id,
            actor_id: id(identity.actor_id.as_bytes())?,
            sheet_id: current.2,
            row_id: current.3,
            column_id: current.4,
            text: String::new(),
        };
        let mut plaintext =
            Zeroizing::new(serde_json::to_vec(&payload).map_err(|_| CommentError::InvalidInput)?);
        let sealed = self
            .seal_record_guarded(
                document,
                revision,
                "comment",
                event_id,
                1,
                &plaintext,
                16 * 1024,
            )
            .await?;
        plaintext.zeroize();
        sqlx::query("INSERT INTO document_comment_events(document_id,revision,event_id,request_id,comment_id,event_kind,sheet_id,row_id,column_id,actor_id,payload) VALUES($1,$2,$3,$3,$4,'delete',$5,$6,$7,$8,$9)")
            .bind(doc).bind(i64::try_from(revision).map_err(|_| CommentError::Corrupt)?).bind(event_id).bind(comment_id).bind(current.2).bind(current.3).bind(current.4).bind(id(identity.actor_id.as_bytes())?).bind(sealed).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(CommentAddress {
            sheet_id: current.2,
            row_id: current.3,
            column_id: current.4,
        })
    }

    pub async fn list_comments(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        address: CommentAddress,
        cursor: Option<u64>,
        limit: u32,
    ) -> Result<CommentPage, CommentError> {
        if address.sheet_id.is_nil()
            || address.row_id.is_nil()
            || address.column_id.is_nil()
            || !(1..=MAX_PAGE).contains(&limit)
            || cursor.is_some_and(|value| value == 0 || value > i64::MAX as u64)
        {
            return Err(CommentError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        let doc = sqlx::query_scalar::<_, Uuid>("SELECT document_id FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR SHARE")
            .bind(id(document.as_bytes())?).bind(id(identity.workspace_id.as_bytes())?).fetch_optional(&mut *tx).await?.ok_or(CommentError::NotFound)?;
        let can_moderate = authorize(&mut tx, identity, document, false, None).await?;
        let revision = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT revision FROM document_comment_heads WHERE document_id=$1",
        )
        .bind(doc)
        .fetch_optional(&mut *tx)
        .await?
        .flatten()
        .unwrap_or(0);
        let query = "WITH latest AS (SELECT DISTINCT ON(comment_id) event_id,revision,comment_id,event_kind,sheet_id,row_id,column_id,actor_id,payload FROM document_comment_events WHERE document_id=$1 AND sheet_id=$2 AND row_id=$3 AND column_id=$4 ORDER BY comment_id,revision DESC) SELECT event_id,revision,comment_id,event_kind,sheet_id,row_id,column_id,actor_id,payload FROM latest WHERE event_kind='add' AND ($5::bigint IS NULL OR revision<$5) ORDER BY revision DESC LIMIT $6";
        let rows = sqlx::query_as::<_, EventRow>(query)
            .bind(doc)
            .bind(address.sheet_id)
            .bind(address.row_id)
            .bind(address.column_id)
            .bind(cursor.map(|n| n as i64))
            .bind(i64::from(limit) + 1)
            .fetch_all(&mut *tx)
            .await?;
        let has_more = rows.len() > limit as usize;
        let mut entries = Vec::with_capacity(rows.len().min(limit as usize));
        for row in rows.into_iter().take(limit as usize) {
            let revision = u64::try_from(row.revision).map_err(|_| CommentError::Corrupt)?;
            let plain = self
                .open_record(
                    document,
                    revision,
                    "comment",
                    row.event_id,
                    1,
                    &row.payload,
                    16 * 1024,
                )
                .await?;
            let mut payload: Payload =
                serde_json::from_slice(&plain).map_err(|_| CommentError::Corrupt)?;
            if payload.kind != "add"
                || payload.comment_id != row.comment_id
                || payload.actor_id != row.actor_id
                || payload.sheet_id != row.sheet_id
                || payload.row_id != row.row_id
                || payload.column_id != row.column_id
                || payload.text.is_empty()
                || payload.text.len() > MAX_TEXT_BYTES
            {
                return Err(CommentError::Corrupt);
            }
            let can_delete = can_moderate || row.actor_id == id(identity.actor_id.as_bytes())?;
            entries.push(CommentEntry {
                comment_id: row.comment_id,
                author_id: row.actor_id,
                address: CommentAddress {
                    sheet_id: row.sheet_id,
                    row_id: row.row_id,
                    column_id: row.column_id,
                },
                text: Zeroizing::new(std::mem::take(&mut payload.text)),
                revision,
                can_delete,
            });
        }
        let next_cursor = if has_more {
            entries.last().map(|entry| entry.revision)
        } else {
            None
        };
        tx.commit().await?;
        Ok(CommentPage {
            revision: u64::try_from(revision).map_err(|_| CommentError::Corrupt)?,
            comments: entries,
            next_cursor,
        })
    }
}

async fn next_revision(
    tx: &mut Transaction<'_, Postgres>,
    document: Uuid,
) -> Result<u64, CommentError> {
    let revision = sqlx::query_scalar::<_, i64>("INSERT INTO document_comment_heads(document_id,revision) VALUES($1,1) ON CONFLICT(document_id) DO UPDATE SET revision=document_comment_heads.revision+1 RETURNING revision")
        .bind(document).fetch_one(&mut **tx).await?;
    u64::try_from(revision).map_err(|_| CommentError::Corrupt)
}
