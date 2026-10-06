//! Encrypted catalog metadata and user preferences; current ACL owns every read.
use crate::{PgDatabase, journal::access_for_delivery_locked};
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::{AuthenticatedActor, JournalError};
use spreadsheet_domain::workbook::DocumentId;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;
#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog access denied")]
    Unauthorized,
    #[error("catalog revision changed")]
    Conflict,
    #[error("invalid catalog input")]
    Invalid,
    #[error("catalog unavailable")]
    Unavailable,
    #[error("corrupt catalog record")]
    Corrupt,
}
impl From<sqlx::Error> for CatalogError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
fn access_error(error: JournalError) -> CatalogError {
    match error {
        JournalError::Unauthorized | JournalError::NotFound => CatalogError::Unauthorized,
        _ => CatalogError::Unavailable,
    }
}
fn id(doc: DocumentId) -> Uuid {
    Uuid::from_bytes(*doc.as_bytes())
}
fn actor(identity: AuthenticatedActor) -> Uuid {
    Uuid::from_bytes(*identity.actor_id.as_bytes())
}
fn version(value: i64) -> Result<u64, CatalogError> {
    u64::try_from(value).map_err(|_| CatalogError::Corrupt)
}
fn number(value: u64) -> Result<i64, CatalogError> {
    i64::try_from(value).map_err(|_| CatalogError::Invalid)
}
pub fn valid_title(title: &str) -> bool {
    !title.trim().is_empty() && title.chars().count() <= 256 && !title.contains('\0')
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    title: String,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Preferences {
    favorite: bool,
    opened_at: u64,
}
#[derive(Serialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub revision: u64,
    pub title: String,
}
#[derive(Serialize)]
pub struct LibraryPreference {
    pub schema_version: u32,
    pub revision: u64,
    pub favorite: bool,
    pub opened_at: u64,
}
#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum LibraryLayout {
    Cards,
    List,
}
impl LibraryLayout {
    fn name(self) -> &'static str {
        match self {
            Self::Cards => "cards",
            Self::List => "list",
        }
    }
}
#[derive(Serialize)]
pub struct LayoutPreference {
    pub schema_version: u32,
    pub revision: u64,
    pub layout: LibraryLayout,
}
#[derive(Serialize)]
pub struct CatalogEntry {
    pub document_id: Uuid,
    pub version: u64,
    pub role: String,
    pub created_at: u64,
    pub metadata: Metadata,
    pub preference: LibraryPreference,
    pub disclosure_stamp: crate::DisclosureStamp,
}
#[derive(Serialize)]
pub struct CatalogPage {
    pub schema_version: u32,
    pub documents: Vec<CatalogEntry>,
    pub next_cursor: Option<Uuid>,
    pub layout: LayoutPreference,
}
pub struct CatalogLease {
    _transaction: Transaction<'static, Postgres>,
}
#[derive(FromRow)]
struct Record {
    revision: i64,
    payload: Vec<u8>,
}
#[derive(FromRow)]
struct Row {
    document_id: Uuid,
    head_version: i64,
    role: String,
    created_at: i64,
}
impl PgDatabase {
    async fn catalog_write_lock(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
    ) -> Result<(), CatalogError> {
        let workspace = Uuid::from_bytes(*identity.workspace_id.as_bytes());
        let member: Option<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
        )
        .bind(workspace)
        .bind(actor(identity))
        .fetch_optional(&mut **tx)
        .await?;
        if member.is_none() {
            return Err(CatalogError::Unauthorized);
        }
        let book:Option<Uuid>=sqlx::query_scalar("SELECT document_id FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR UPDATE").bind(id(doc)).bind(workspace).fetch_optional(&mut **tx).await?;
        if book.is_none() {
            return Err(CatalogError::Unauthorized);
        }
        access_for_delivery_locked(tx, identity, doc)
            .await
            .map_err(access_error)?;
        Ok(())
    }
    async fn metadata_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        doc: DocumentId,
    ) -> Result<Metadata, CatalogError> {
        let row: Option<Record> =
            sqlx::query_as("SELECT revision,payload FROM document_metadata WHERE document_id=$1")
                .bind(id(doc))
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(row) = row {
            let revision = version(row.revision)?;
            let bytes = self
                .open_record(
                    doc,
                    revision,
                    "document-metadata",
                    id(doc),
                    1,
                    &row.payload,
                    8192,
                )
                .await
                .map_err(|_| CatalogError::Corrupt)?;
            let title: Title = serde_json::from_slice(&bytes).map_err(|_| CatalogError::Corrupt)?;
            if !valid_title(&title.title) {
                return Err(CatalogError::Corrupt);
            };
            Ok(Metadata {
                schema_version: 1,
                revision,
                title: title.title,
            })
        } else {
            Ok(Metadata {
                schema_version: 1,
                revision: 0,
                title: "Новая таблица".into(),
            })
        }
    }
    async fn preference_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
    ) -> Result<LibraryPreference, CatalogError> {
        let row:Option<Record>=sqlx::query_as("SELECT revision,payload FROM document_library_preferences WHERE document_id=$1 AND user_id=$2").bind(id(doc)).bind(actor(identity)).fetch_optional(&mut **tx).await?;
        let (revision, value) = if let Some(row) = row {
            let revision = version(row.revision)?;
            let bytes = self
                .open_record(
                    doc,
                    revision,
                    "library-preference",
                    actor(identity),
                    1,
                    &row.payload,
                    4096,
                )
                .await
                .map_err(|_| CatalogError::Corrupt)?;
            let value =
                serde_json::from_slice::<Preferences>(&bytes).map_err(|_| CatalogError::Corrupt)?;
            (revision, value)
        } else {
            (0, Preferences::default())
        };
        Ok(LibraryPreference {
            schema_version: 1,
            revision,
            favorite: value.favorite,
            opened_at: value.opened_at,
        })
    }
    async fn layout_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
    ) -> Result<LayoutPreference, CatalogError> {
        let row:Option<(i64,String)>=sqlx::query_as("SELECT revision,layout FROM library_layout_preferences WHERE workspace_id=$1 AND user_id=$2").bind(Uuid::from_bytes(*identity.workspace_id.as_bytes())).bind(actor(identity)).fetch_optional(&mut **tx).await?;
        let (revision, layout) = match row {
            None => (0, LibraryLayout::Cards),
            Some((revision, layout)) => (
                version(revision)?,
                match layout.as_str() {
                    "cards" => LibraryLayout::Cards,
                    "list" => LibraryLayout::List,
                    _ => return Err(CatalogError::Corrupt),
                },
            ),
        };
        Ok(LayoutPreference {
            schema_version: 1,
            revision,
            layout,
        })
    }
    pub async fn catalog_page(
        &self,
        identity: AuthenticatedActor,
        before: Option<Uuid>,
    ) -> Result<(CatalogPage, CatalogLease), CatalogError> {
        let mut tx = self.pool.begin().await?;
        self.guard_transaction(&mut tx).await?;
        let member:Option<String>=sqlx::query_scalar("SELECT m.role FROM memberships m JOIN users u ON u.user_id=m.user_id WHERE m.workspace_id=$1 AND m.user_id=$2 AND u.identity_kind='member' FOR SHARE OF m,u").bind(Uuid::from_bytes(*identity.workspace_id.as_bytes())).bind(actor(identity)).fetch_optional(&mut *tx).await?;
        if member.is_none() {
            return Err(CatalogError::Unauthorized);
        }
        let mut rows:Vec<Row>=sqlx::query_as("SELECT d.document_id,d.head_version,p.role,floor(extract(epoch FROM d.created_at)*1000)::bigint AS created_at FROM documents d JOIN document_permissions p ON p.document_id=d.document_id AND p.workspace_id=d.workspace_id WHERE d.workspace_id=$1 AND p.user_id=$2 AND d.state='active' AND ($3::uuid IS NULL OR d.document_id<$3) ORDER BY d.document_id DESC LIMIT 101 FOR SHARE OF d,p").bind(Uuid::from_bytes(*identity.workspace_id.as_bytes())).bind(actor(identity)).bind(before).fetch_all(&mut *tx).await?;
        let next_cursor = if rows.len() > 100 {
            rows.truncate(100);
            rows.last().map(|r| r.document_id)
        } else {
            None
        };
        let mut documents = Vec::with_capacity(rows.len());
        for row in rows {
            let doc = DocumentId::from_bytes(*row.document_id.as_bytes())
                .map_err(|_| CatalogError::Corrupt)?;
            let copy = self
                .disclosure_decision(
                    &mut tx,
                    identity,
                    doc,
                    None,
                    crate::DisclosureAction::Copy,
                    None,
                )
                .await
                .map_err(|_| CatalogError::Unavailable)?;
            documents.push(CatalogEntry {
                document_id: row.document_id,
                version: version(row.head_version)?,
                role: row.role,
                created_at: version(row.created_at)?,
                metadata: self.metadata_in(&mut tx, doc).await?,
                preference: self.preference_in(&mut tx, identity, doc).await?,
                disclosure_stamp: copy.stamp,
            });
        }
        let layout = self.layout_in(&mut tx, identity).await?;
        Ok((
            CatalogPage {
                schema_version: 1,
                documents,
                next_cursor,
                layout,
            },
            CatalogLease { _transaction: tx },
        ))
    }
    pub async fn document_metadata(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
    ) -> Result<Metadata, CatalogError> {
        let mut tx = self.begin_guarded().await?;
        access_for_delivery_locked(&mut tx, identity, doc)
            .await
            .map_err(access_error)?;
        self.metadata_in(&mut tx, doc).await
    }
    pub(crate) async fn initial_metadata(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        doc: DocumentId,
        title: &str,
    ) -> Result<(), CatalogError> {
        if !valid_title(title) {
            return Err(CatalogError::Invalid);
        }
        let plain = Zeroizing::new(
            serde_json::to_vec(&Title {
                title: title.into(),
            })
            .map_err(|_| CatalogError::Invalid)?,
        );
        let payload = self
            .seal_record_guarded(doc, 1, "document-metadata", id(doc), 1, &plain, 8192)
            .await
            .map_err(|_| CatalogError::Unavailable)?;
        sqlx::query("INSERT INTO document_metadata(document_id,revision,payload) VALUES($1,1,$2)")
            .bind(id(doc))
            .bind(payload)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn set_document_metadata(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        expected: u64,
        title: String,
    ) -> Result<Metadata, CatalogError> {
        if !valid_title(&title) {
            return Err(CatalogError::Invalid);
        }
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        self.catalog_write_lock(&mut tx, identity, doc).await?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2",
        )
        .bind(id(doc))
        .bind(actor(identity))
        .fetch_optional(&mut *tx)
        .await?;
        if role.as_deref() != Some("owner") {
            return Err(CatalogError::Unauthorized);
        }
        if self.metadata_in(&mut tx, doc).await?.revision != expected {
            return Err(CatalogError::Conflict);
        }
        let revision = expected.checked_add(1).ok_or(CatalogError::Invalid)?;
        let plain = Zeroizing::new(
            serde_json::to_vec(&Title {
                title: title.clone(),
            })
            .map_err(|_| CatalogError::Invalid)?,
        );
        let payload = self
            .seal_record_guarded(doc, revision, "document-metadata", id(doc), 1, &plain, 8192)
            .await
            .map_err(|_| CatalogError::Unavailable)?;
        sqlx::query("INSERT INTO document_metadata(document_id,revision,payload) VALUES($1,$2,$3) ON CONFLICT(document_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload").bind(id(doc)).bind(number(revision)?).bind(payload).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Metadata {
            schema_version: 1,
            revision,
            title,
        })
    }
    pub async fn set_library_preference(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        expected: Option<u64>,
        favorite: Option<bool>,
    ) -> Result<LibraryPreference, CatalogError> {
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        self.catalog_write_lock(&mut tx, identity, doc).await?;
        let member: bool =
            sqlx::query_scalar("SELECT identity_kind='member' FROM users WHERE user_id=$1")
                .bind(actor(identity))
                .fetch_one(&mut *tx)
                .await?;
        if !member {
            return Err(CatalogError::Unauthorized);
        }
        let previous = self.preference_in(&mut tx, identity, doc).await?;
        if expected.is_some_and(|v| v != previous.revision) {
            return Err(CatalogError::Conflict);
        }
        let revision = previous
            .revision
            .checked_add(1)
            .ok_or(CatalogError::Invalid)?;
        let opened_at = if favorite.is_some() {
            previous.opened_at
        } else {
            version(
                sqlx::query_scalar::<_, i64>(
                    "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint",
                )
                .fetch_one(&mut *tx)
                .await?,
            )?
        };
        let value = Preferences {
            favorite: favorite.unwrap_or(previous.favorite),
            opened_at,
        };
        let plain = Zeroizing::new(serde_json::to_vec(&value).map_err(|_| CatalogError::Invalid)?);
        let payload = self
            .seal_record_guarded(
                doc,
                revision,
                "library-preference",
                actor(identity),
                1,
                &plain,
                4096,
            )
            .await
            .map_err(|_| CatalogError::Unavailable)?;
        sqlx::query("INSERT INTO document_library_preferences(document_id,user_id,revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(document_id,user_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload").bind(id(doc)).bind(actor(identity)).bind(number(revision)?).bind(payload).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(LibraryPreference {
            schema_version: 1,
            revision,
            favorite: value.favorite,
            opened_at,
        })
    }
    pub async fn set_library_layout(
        &self,
        identity: AuthenticatedActor,
        expected: u64,
        layout: LibraryLayout,
    ) -> Result<LayoutPreference, CatalogError> {
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let user:Option<Uuid>=sqlx::query_scalar("SELECT m.user_id FROM memberships m JOIN users u ON u.user_id=m.user_id WHERE m.workspace_id=$1 AND m.user_id=$2 AND u.identity_kind='member' FOR UPDATE OF m").bind(Uuid::from_bytes(*identity.workspace_id.as_bytes())).bind(actor(identity)).fetch_optional(&mut *tx).await?;
        if user.is_none() {
            return Err(CatalogError::Unauthorized);
        }
        if self.layout_in(&mut tx, identity).await?.revision != expected {
            return Err(CatalogError::Conflict);
        }
        let revision = expected.checked_add(1).ok_or(CatalogError::Invalid)?;
        sqlx::query("INSERT INTO library_layout_preferences(workspace_id,user_id,revision,layout) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id,user_id) DO UPDATE SET revision=EXCLUDED.revision,layout=EXCLUDED.layout").bind(Uuid::from_bytes(*identity.workspace_id.as_bytes())).bind(actor(identity)).bind(number(revision)?).bind(layout.name()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(LayoutPreference {
            schema_version: 1,
            revision,
            layout,
        })
    }
}
