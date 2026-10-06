//! Independent encrypted personal filters. No workbook, calculation, or history writes.
use crate::{PgDatabase, journal::access_for_delivery_locked};
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::{AuthenticatedActor, JournalError};
use spreadsheet_domain::workbook::{ColumnId, DocumentId, RowId, SheetId, Workbook};
use sqlx::{FromRow, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_VIEW_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterValue {
    Blank {},
    Number { value: f64 },
    Text { value: String },
    Boolean { value: bool },
    Error { value: String },
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFilterOperator {
    Equals,
    Contains,
    StartsWith,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberFilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterPredicate {
    Blank {},
    Nonblank {},
    Values {
        values: Vec<FilterValue>,
    },
    Text {
        operator: TextFilterOperator,
        value: String,
    },
    Number {
        operator: NumberFilterOperator,
        value: f64,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnFilter {
    pub column_id: Uuid,
    pub predicate: FilterPredicate,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalFilter {
    pub id: Uuid,
    pub sheet_id: Uuid,
    pub first_row_id: Uuid,
    pub last_row_id: Uuid,
    pub first_column_id: Uuid,
    pub last_column_id: Uuid,
    pub header_row_id: Option<Uuid>,
    pub conditions: Vec<ColumnFilter>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalFilters {
    pub filters: Vec<PersonalFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hardcode_dismissals: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct PersonalView {
    pub schema_version: u32,
    pub revision: u64,
    pub settings: PersonalFilters,
}
#[derive(Debug, Error)]
pub enum PersonalViewError {
    #[error("personal view unauthorized")]
    Unauthorized,
    #[error("personal view changed")]
    Conflict,
    #[error("invalid personal view")]
    Invalid,
    #[error("personal view unavailable")]
    Unavailable,
}
impl From<sqlx::Error> for PersonalViewError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
fn access_error(error: JournalError) -> PersonalViewError {
    match error {
        JournalError::Unauthorized | JournalError::NotFound => PersonalViewError::Unauthorized,
        _ => PersonalViewError::Unavailable,
    }
}
fn text_valid(value: &str) -> bool {
    value.len() <= 1024 && !value.contains('\0')
}
impl PersonalFilters {
    pub fn validate(&self) -> Result<(), PersonalViewError> {
        if self.filters.len() > 5
            || self.hardcode_dismissals.len() > 1000
            || self.hardcode_dismissals.iter().any(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || self
                .hardcode_dismissals
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.hardcode_dismissals.len()
        {
            return Err(PersonalViewError::Invalid);
        }
        let mut identifiers = std::collections::BTreeSet::new();
        for filter in &self.filters {
            if !identifiers.insert(filter.id)
                || filter.conditions.len() > 50
                || [
                    filter.id,
                    filter.sheet_id,
                    filter.first_row_id,
                    filter.last_row_id,
                    filter.first_column_id,
                    filter.last_column_id,
                ]
                .iter()
                .any(Uuid::is_nil)
                || filter.header_row_id.is_some_and(|id| id.is_nil())
            {
                return Err(PersonalViewError::Invalid);
            }
            let mut columns = std::collections::BTreeSet::new();
            for condition in &filter.conditions {
                if condition.column_id.is_nil() || !columns.insert(condition.column_id) {
                    return Err(PersonalViewError::Invalid);
                }
                let valid = match &condition.predicate {
                    FilterPredicate::Blank {} | FilterPredicate::Nonblank {} => true,
                    FilterPredicate::Number { value, .. } => value.is_finite(),
                    FilterPredicate::Text { value, .. } => text_valid(value),
                    FilterPredicate::Values { values } => {
                        values.len() <= 500
                            && values.iter().all(|v| match v {
                                FilterValue::Text { value: t }
                                | FilterValue::Error { value: t } => text_valid(t),
                                FilterValue::Number { value: n } => n.is_finite(),
                                _ => true,
                            })
                    }
                };
                if !valid {
                    return Err(PersonalViewError::Invalid);
                }
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| PersonalViewError::Invalid)?
            .len()
            > MAX_VIEW_BYTES
        {
            return Err(PersonalViewError::Invalid);
        }
        Ok(())
    }
    pub fn validate_geometry(&self, book: &Workbook) -> Result<(), PersonalViewError> {
        self.validate()?;
        for filter in &self.filters {
            let sheet_id = SheetId::from_bytes(*filter.sheet_id.as_bytes())
                .map_err(|_| PersonalViewError::Invalid)?;
            let sheet = book
                .sheets()
                .iter()
                .find(|s| s.id() == sheet_id)
                .ok_or(PersonalViewError::Invalid)?;
            let row = |id: Uuid| {
                RowId::from_bytes(*id.as_bytes())
                    .ok()
                    .and_then(|id| sheet.row_position(id))
                    .ok_or(PersonalViewError::Invalid)
            };
            let col = |id: Uuid| {
                ColumnId::from_bytes(*id.as_bytes())
                    .ok()
                    .and_then(|id| sheet.column_position(id))
                    .ok_or(PersonalViewError::Invalid)
            };
            let first = row(filter.first_row_id)?;
            let last = row(filter.last_row_id)?;
            let left = col(filter.first_column_id)?;
            let right = col(filter.last_column_id)?;
            if last < first
                || right < left
                || u64::from(last - first + 1) * u64::from(right - left + 1) > 100_000
                || filter
                    .header_row_id
                    .map(row)
                    .transpose()?
                    .is_some_and(|r| r < first || r > last)
            {
                return Err(PersonalViewError::Invalid);
            }
            for condition in &filter.conditions {
                let column = col(condition.column_id)?;
                if column < left || column > right {
                    return Err(PersonalViewError::Invalid);
                }
            }
        }
        Ok(())
    }
}
#[derive(FromRow)]
struct ViewRow {
    revision: i64,
    payload: Vec<u8>,
}
impl PgDatabase {
    async fn view_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        doc: DocumentId,
        user: Uuid,
    ) -> Result<PersonalView, PersonalViewError> {
        let row: Option<ViewRow> = sqlx::query_as("SELECT revision,payload FROM personal_document_views WHERE document_id=$1 AND user_id=$2 FOR UPDATE")
            .bind(Uuid::from_bytes(*doc.as_bytes())).bind(user).fetch_optional(&mut **tx).await?;
        let Some(row) = row else {
            return Ok(PersonalView {
                schema_version: 1,
                revision: 0,
                settings: PersonalFilters::default(),
            });
        };
        let revision = u64::try_from(row.revision).map_err(|_| PersonalViewError::Unavailable)?;
        let plain = self
            .open_record(
                doc,
                revision,
                "personal-view",
                user,
                1,
                &row.payload,
                MAX_VIEW_BYTES,
            )
            .await
            .map_err(|_| PersonalViewError::Unavailable)?;
        let settings: PersonalFilters =
            serde_json::from_slice(&plain).map_err(|_| PersonalViewError::Unavailable)?;
        settings
            .validate()
            .map_err(|_| PersonalViewError::Unavailable)?;
        Ok(PersonalView {
            schema_version: 1,
            revision,
            settings,
        })
    }
    pub async fn personal_view(
        &self,
        actor: AuthenticatedActor,
        doc: DocumentId,
    ) -> Result<PersonalView, PersonalViewError> {
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| PersonalViewError::Unavailable)?;
        access_for_delivery_locked(&mut tx, actor, doc)
            .await
            .map_err(access_error)?;
        let view = self
            .view_in_transaction(&mut tx, doc, Uuid::from_bytes(*actor.actor_id.as_bytes()))
            .await?;
        tx.commit().await?;
        Ok(view)
    }
    pub async fn set_personal_view(
        &self,
        actor: AuthenticatedActor,
        doc: DocumentId,
        document_version: u64,
        expected_revision: u64,
        settings: PersonalFilters,
    ) -> Result<PersonalView, PersonalViewError> {
        settings.validate()?;
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| PersonalViewError::Unavailable)?;
        let access = access_for_delivery_locked(&mut tx, actor, doc)
            .await
            .map_err(access_error)?;
        if access.head.get() != document_version {
            return Err(PersonalViewError::Conflict);
        }
        let user = Uuid::from_bytes(*actor.actor_id.as_bytes());
        if self.view_in_transaction(&mut tx, doc, user).await?.revision != expected_revision {
            return Err(PersonalViewError::Conflict);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(PersonalViewError::Invalid)?;
        let revision_sql = i64::try_from(revision).map_err(|_| PersonalViewError::Invalid)?;
        let expected_sql =
            i64::try_from(expected_revision).map_err(|_| PersonalViewError::Invalid)?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let plain =
            Zeroizing::new(serde_json::to_vec(&settings).map_err(|_| PersonalViewError::Invalid)?);
        let payload = self
            .seal_record_guarded(
                doc,
                revision,
                "personal-view",
                user,
                1,
                &plain,
                MAX_VIEW_BYTES,
            )
            .await
            .map_err(|_| PersonalViewError::Unavailable)?;
        let written = sqlx::query("INSERT INTO personal_document_views(document_id,user_id,revision,payload) VALUES($1,$2,$3,$4) ON CONFLICT(document_id,user_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload WHERE personal_document_views.revision=$5")
            .bind(Uuid::from_bytes(*doc.as_bytes())).bind(user).bind(revision_sql).bind(payload).bind(expected_sql).execute(&mut *tx).await?;
        if written.rows_affected() != 1 {
            return Err(PersonalViewError::Conflict);
        }
        tx.commit().await?;
        Ok(PersonalView {
            schema_version: 1,
            revision,
            settings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_application::journal::OperationJournal;
    use spreadsheet_domain::workbook::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[sqlx::test(migrations = false)]
    async fn filters_are_private_encrypted_cas_state_without_a_workbook_commit(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let owner = AuthenticatedActor {
            actor_id: ActorId::from_bytes([1; 16])?,
            workspace_id: WorkspaceId::from_bytes([2; 16])?,
        };
        db.issue_development_session([9; 32], owner.actor_id, owner.workspace_id)
            .await?;
        let doc = DocumentId::from_bytes([3; 16])?;
        let seed = SheetSeed {
            id: SheetId::from_bytes([4; 16])?,
            name: "Лист".into(),
            rows: vec![RowId::from_bytes([5; 16])?, RowId::from_bytes([6; 16])?],
            columns: vec![ColumnId::from_bytes([7; 16])?],
        };
        let operation = Operation::CreateWorkbook {
            sheet: seed.clone(),
        };
        let book = Workbook::empty(doc).candidate(&operation, DocumentVersion::new(1)?)?;
        db.create(
            owner,
            CommittedEvent {
                document_id: doc,
                operation_id: OperationId::from_bytes([8; 16])?,
                principal: EventPrincipal::User(owner.actor_id),
                version: book.version(),
                operation,
                state_hash: book.hash(),
            },
        )
        .await?;
        let viewer = AuthenticatedActor {
            actor_id: ActorId::from_bytes([10; 16])?,
            ..owner
        };
        sqlx::query("INSERT INTO users(user_id) VALUES($1)")
            .bind(Uuid::from_bytes(*viewer.actor_id.as_bytes()))
            .execute(&db.pool)
            .await?;
        db.change_permission(
            owner,
            doc,
            viewer.actor_id,
            AuthorizationChangeId::from_bytes([11; 16])?,
            Some(crate::PermissionRole::Viewer),
        )
        .await?;
        let settings = PersonalFilters {
            filters: vec![PersonalFilter {
                id: Uuid::from_bytes([12; 16]),
                sheet_id: Uuid::from_bytes(*seed.id.as_bytes()),
                first_row_id: Uuid::from_bytes(*seed.rows[0].as_bytes()),
                last_row_id: Uuid::from_bytes(*seed.rows[1].as_bytes()),
                first_column_id: Uuid::from_bytes(*seed.columns[0].as_bytes()),
                last_column_id: Uuid::from_bytes(*seed.columns[0].as_bytes()),
                header_row_id: Some(Uuid::from_bytes(*seed.rows[0].as_bytes())),
                conditions: vec![ColumnFilter {
                    column_id: Uuid::from_bytes(*seed.columns[0].as_bytes()),
                    predicate: FilterPredicate::Text {
                        operator: TextFilterOperator::Contains,
                        value: "Private filter marker".into(),
                    },
                }],
            }],
            hardcode_dismissals: vec!["ab".repeat(32)],
        };
        settings.validate_geometry(&book)?;
        assert_eq!(db.personal_view(viewer, doc).await?.revision, 0);
        let saved = db
            .set_personal_view(viewer, doc, 1, 0, settings.clone())
            .await?;
        assert_eq!(saved.revision, 1);
        assert_eq!(db.personal_view(viewer, doc).await?.settings, settings);
        assert_eq!(
            db.personal_view(owner, doc).await?.settings,
            PersonalFilters::default()
        );
        assert!(matches!(
            db.set_personal_view(viewer, doc, 1, 0, settings.clone())
                .await,
            Err(PersonalViewError::Conflict)
        ));
        assert!(matches!(
            db.set_personal_view(viewer, doc, 2, 1, settings.clone())
                .await,
            Err(PersonalViewError::Conflict)
        ));
        let payload: Vec<u8> = sqlx::query_scalar(
            "SELECT payload FROM personal_document_views WHERE document_id=$1 AND user_id=$2",
        )
        .bind(Uuid::from_bytes(*doc.as_bytes()))
        .bind(Uuid::from_bytes(*viewer.actor_id.as_bytes()))
        .fetch_one(&db.pool)
        .await?;
        assert!(
            !payload
                .windows(b"Private filter marker".len())
                .any(|w| w == b"Private filter marker")
        );
        let event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_operations WHERE document_id=$1")
                .bind(Uuid::from_bytes(*doc.as_bytes()))
                .fetch_one(&db.pool)
                .await?;
        assert_eq!(event_count, 1);
        assert_eq!(db.access(owner, doc).await?.head, book.version());
        db.change_permission(
            owner,
            doc,
            viewer.actor_id,
            AuthorizationChangeId::from_bytes([13; 16])?,
            None,
        )
        .await?;
        assert!(matches!(
            db.personal_view(viewer, doc).await,
            Err(PersonalViewError::Unauthorized)
        ));
        Ok(())
    }

    #[test]
    fn invalid_and_unknown_filter_dtos_fail_before_storage() -> TestResult {
        assert!(
            serde_json::from_str::<FilterPredicate>(
                r#"{"kind":"number","operator":"approximately","value":1}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<FilterPredicate>(r#"{"kind":"blank","unexpected":true}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<PersonalFilters>(r#"{"filters":[],"schema_version":2}"#)
                .is_err()
        );
        let mut invalid = PersonalFilters::default();
        invalid.filters.push(PersonalFilter {
            id: Uuid::nil(),
            sheet_id: Uuid::nil(),
            first_row_id: Uuid::nil(),
            last_row_id: Uuid::nil(),
            first_column_id: Uuid::nil(),
            last_column_id: Uuid::nil(),
            header_row_id: None,
            conditions: vec![],
        });
        assert!(invalid.validate().is_err());
        Ok(())
    }
}
