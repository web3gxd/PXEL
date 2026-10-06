use crate::PgDatabase;
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::DocumentId;
use sqlx::{FromRow, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureRules {
    pub allow_copy: bool,
    pub export_formats: Vec<DisclosureFormat>,
    pub allow_participant_export: bool,
    pub require_anonymization: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureFormat {
    Xlsx,
    Pxel,
    Csv,
}
impl DisclosureRules {
    pub(crate) fn defaults(link: bool) -> Self {
        Self {
            allow_copy: true,
            export_formats: if link {
                vec![]
            } else {
                vec![
                    DisclosureFormat::Xlsx,
                    DisclosureFormat::Pxel,
                    DisclosureFormat::Csv,
                ]
            },
            allow_participant_export: false,
            require_anonymization: false,
        }
    }
    fn valid(&self) -> bool {
        self.export_formats.len() <= 3
            && self
                .export_formats
                .iter()
                .filter(|f| **f == DisclosureFormat::Csv)
                .count()
                <= 1
            && self
                .export_formats
                .iter()
                .filter(|f| **f == DisclosureFormat::Xlsx)
                .count()
                <= 1
            && self
                .export_formats
                .iter()
                .filter(|f| **f == DisclosureFormat::Pxel)
                .count()
                <= 1
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct DisclosurePolicy {
    pub schema_version: u32,
    pub revision: u64,
    pub rules: DisclosureRules,
    pub access_epoch: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureAction {
    Copy,
    CopyAnonymized,
    Cut,
    ExportXlsx,
    ExportXlsxAnonymized,
    ExportPxel,
    ExportPxelAnonymized,
    ExportCsv,
    ExportCsvAnonymized,
}
impl DisclosureAction {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Copy | Self::CopyAnonymized => ("copy", "clipboard"),
            Self::Cut => ("cut", "clipboard"),
            Self::ExportXlsx | Self::ExportXlsxAnonymized => ("export", "xlsx"),
            Self::ExportPxel | Self::ExportPxelAnonymized => ("export", "pxel"),
            Self::ExportCsv | Self::ExportCsvAnonymized => ("export", "csv"),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureReason {
    Allowed,
    RoleDenied,
    CopyDenied,
    FormatDenied,
    AnonymizationRequired,
    PolicyChanged,
    VersionChanged,
    PrivacyAnalysisFailed,
    PreparationFailed,
}
impl DisclosureReason {
    fn name(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::RoleDenied => "role_denied",
            Self::CopyDenied => "copy_denied",
            Self::FormatDenied => "format_denied",
            Self::AnonymizationRequired => "anonymization_required",
            Self::PolicyChanged => "policy_changed",
            Self::VersionChanged => "version_changed",
            Self::PrivacyAnalysisFailed => "privacy_analysis_failed",
            Self::PreparationFailed => "preparation_failed",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureStamp {
    pub document_version: u64,
    pub book_revision: u64,
    pub link_revision: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct DisclosureReceipt {
    pub schema_version: u32,
    pub allowed: bool,
    pub reason: DisclosureReason,
    pub stamp: DisclosureStamp,
    pub anonymized: bool,
}
#[derive(Clone, Debug, Serialize, FromRow)]
pub struct DisclosureAuditEvent {
    pub event_id: Uuid,
    pub actor_id: Uuid,
    pub link_id: Option<Uuid>,
    pub created_at: i64,
    pub document_version: i64,
    pub book_revision: i64,
    pub link_revision: i64,
    pub action: String,
    pub format: String,
    pub allowed: bool,
    pub reason: String,
}
#[derive(Serialize)]
pub struct DisclosureAuditPage {
    pub schema_version: u32,
    pub events: Vec<DisclosureAuditEvent>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Debug, Error)]
pub enum DisclosureError {
    #[error("disclosure unauthorized")]
    Unauthorized,
    #[error("disclosure policy changed")]
    Conflict,
    #[error("invalid disclosure policy")]
    InvalidInput,
    #[error("disclosure store unavailable")]
    Unavailable,
    #[error("invalid disclosure storage")]
    Corrupt,
}
impl From<sqlx::Error> for DisclosureError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
#[derive(FromRow)]
pub(crate) struct DocumentRow {
    pub(crate) head_version: i64,
    pub(crate) access_epoch: i64,
}
#[derive(FromRow)]
struct PolicyRow {
    revision: i64,
    payload: Vec<u8>,
    codec_version: i32,
}
fn id(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}
fn number(value: u64) -> Result<i64, DisclosureError> {
    i64::try_from(value).map_err(|_| DisclosureError::InvalidInput)
}
fn unsigned(value: i64) -> Result<u64, DisclosureError> {
    u64::try_from(value).map_err(|_| DisclosureError::Corrupt)
}

impl PgDatabase {
    pub(crate) async fn disclosure_document(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
        write: bool,
    ) -> Result<DocumentRow, DisclosureError> {
        let query = if write {
            "SELECT head_version,access_epoch FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR UPDATE"
        } else {
            "SELECT head_version,access_epoch FROM documents WHERE document_id=$1 AND workspace_id=$2 AND state='active' FOR SHARE"
        };
        sqlx::query_as(query)
            .bind(id(doc.as_bytes()))
            .bind(id(identity.workspace_id.as_bytes()))
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(DisclosureError::Unauthorized)
    }
    pub(crate) async fn disclosure_role(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<Option<String>, DisclosureError> {
        if let Some(link) = link {
            Ok(sqlx::query_scalar("SELECT l.role FROM share_guest_sessions s JOIN share_links l ON l.link_id=s.link_id JOIN users u ON u.user_id=s.user_id AND u.identity_kind='share_guest' WHERE s.user_id=$1 AND s.document_id=$2 AND l.link_id=$3 AND l.workspace_id=$4 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND l.status='active' AND (l.expires_at IS NULL OR l.expires_at>clock_timestamp())").bind(id(identity.actor_id.as_bytes())).bind(id(doc.as_bytes())).bind(link).bind(id(identity.workspace_id.as_bytes())).fetch_optional(&mut **tx).await?)
        } else {
            Ok(sqlx::query_scalar("SELECT p.role FROM document_permissions p JOIN memberships m ON m.workspace_id=p.workspace_id AND m.user_id=p.user_id JOIN users u ON u.user_id=p.user_id AND u.identity_kind='member' WHERE p.document_id=$1 AND p.workspace_id=$2 AND p.user_id=$3").bind(id(doc.as_bytes())).bind(id(identity.workspace_id.as_bytes())).bind(id(identity.actor_id.as_bytes())).fetch_optional(&mut **tx).await?)
        }
    }
    pub(crate) async fn read_disclosure_policy(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        doc: DocumentId,
        link: Option<Uuid>,
        epoch: u64,
    ) -> Result<DisclosurePolicy, DisclosureError> {
        let scope = link.unwrap_or(id(doc.as_bytes()));
        let row: Option<PolicyRow> = sqlx::query_as(
            "SELECT revision,payload,codec_version FROM disclosure_policies WHERE scope_id=$1 AND document_id=$2",
        )
        .bind(scope)
        .bind(id(doc.as_bytes()))
        .fetch_optional(&mut **tx)
        .await?;
        let (revision, rules) = if let Some(row) = row {
            let codec = match row.codec_version {
                1 => 1,
                2 => 2,
                _ => return Err(DisclosureError::Corrupt),
            };
            let revision = unsigned(row.revision)?;
            let bytes = self
                .open_record(
                    doc,
                    revision,
                    "disclosure-policy",
                    scope,
                    codec,
                    &row.payload,
                    4096,
                )
                .await
                .map_err(|_| DisclosureError::Corrupt)?;
            let rules: DisclosureRules =
                serde_json::from_slice(&bytes).map_err(|_| DisclosureError::Corrupt)?;
            if !rules.valid()
                || (codec == 1 && rules.export_formats.contains(&DisclosureFormat::Csv))
            {
                return Err(DisclosureError::Corrupt);
            }
            (revision, rules)
        } else {
            (0, DisclosureRules::defaults(link.is_some()))
        };
        Ok(DisclosurePolicy {
            schema_version: 2,
            revision,
            rules,
            access_epoch: epoch,
        })
    }
    pub(crate) async fn require_disclosure_owner(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<(), DisclosureError> {
        if self
            .disclosure_role(tx, identity, doc, None)
            .await?
            .as_deref()
            != Some("owner")
        {
            return Err(DisclosureError::Unauthorized);
        }
        if let Some(link) = link {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM share_links WHERE link_id=$1 AND document_id=$2)",
            )
            .bind(link)
            .bind(id(doc.as_bytes()))
            .fetch_one(&mut **tx)
            .await?;
            if !exists {
                return Err(DisclosureError::Unauthorized);
            }
        }
        Ok(())
    }
    pub async fn disclosure_policy(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<DisclosurePolicy, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        let row = self
            .disclosure_document(&mut tx, identity, doc, false)
            .await?;
        self.require_disclosure_owner(&mut tx, identity, doc, link)
            .await?;
        self.read_disclosure_policy(&mut tx, doc, link, unsigned(row.access_epoch)?)
            .await
    }
    pub async fn set_disclosure_policy(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        expected_revision: u64,
        rules: DisclosureRules,
    ) -> Result<DisclosurePolicy, DisclosureError> {
        if !rules.valid() {
            return Err(DisclosureError::InvalidInput);
        }
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let row = self
            .disclosure_document(&mut tx, identity, doc, true)
            .await?;
        self.require_disclosure_owner(&mut tx, identity, doc, link)
            .await?;
        let previous = self
            .read_disclosure_policy(&mut tx, doc, link, unsigned(row.access_epoch)?)
            .await?;
        if previous.revision != expected_revision {
            return Err(DisclosureError::Conflict);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(DisclosureError::InvalidInput)?;
        let scope = link.unwrap_or(id(doc.as_bytes()));
        let bytes =
            Zeroizing::new(serde_json::to_vec(&rules).map_err(|_| DisclosureError::InvalidInput)?);
        let payload = self
            .seal_record_guarded(doc, revision, "disclosure-policy", scope, 2, &bytes, 4096)
            .await
            .map_err(|_| DisclosureError::Unavailable)?;
        sqlx::query("INSERT INTO disclosure_policies(scope_id,document_id,link_id,revision,payload,codec_version) VALUES($1,$2,$3,$4,$5,2) ON CONFLICT(scope_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload,codec_version=EXCLUDED.codec_version").bind(scope).bind(id(doc.as_bytes())).bind(link).bind(number(revision)?).bind(payload).execute(&mut *tx).await?;
        let epoch = row
            .access_epoch
            .checked_add(1)
            .ok_or(DisclosureError::Corrupt)?;
        sqlx::query("UPDATE documents SET access_epoch=$1 WHERE document_id=$2")
            .bind(epoch)
            .bind(id(doc.as_bytes()))
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE share_guest_sessions SET revoked_at=clock_timestamp() WHERE document_id=$1 AND ($2::uuid IS NULL OR link_id=$2) AND revoked_at IS NULL").bind(id(doc.as_bytes())).bind(link).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(DisclosurePolicy {
            schema_version: 2,
            revision,
            rules,
            access_epoch: unsigned(epoch)?,
        })
    }
    pub(crate) async fn disclosure_decision(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        action: DisclosureAction,
        expected: Option<DisclosureStamp>,
    ) -> Result<DisclosureReceipt, DisclosureError> {
        let row = self.disclosure_document(tx, identity, doc, false).await?;
        let book = self
            .read_disclosure_policy(tx, doc, None, unsigned(row.access_epoch)?)
            .await?;
        let shared = if link.is_some() {
            Some(
                self.read_disclosure_policy(tx, doc, link, unsigned(row.access_epoch)?)
                    .await?,
            )
        } else {
            None
        };
        let role = self.disclosure_role(tx, identity, doc, link).await?;
        let stamp = DisclosureStamp {
            document_version: unsigned(row.head_version)?,
            book_revision: book.revision,
            link_revision: shared.as_ref().map_or(0, |s| s.revision),
        };
        let format = match action {
            DisclosureAction::ExportXlsx | DisclosureAction::ExportXlsxAnonymized => {
                Some(DisclosureFormat::Xlsx)
            }
            DisclosureAction::ExportPxel | DisclosureAction::ExportPxelAnonymized => {
                Some(DisclosureFormat::Pxel)
            }
            DisclosureAction::ExportCsv | DisclosureAction::ExportCsvAnonymized => {
                Some(DisclosureFormat::Csv)
            }
            _ => None,
        };
        let export = format.is_some();
        let anonymized = matches!(
            action,
            DisclosureAction::CopyAnonymized
                | DisclosureAction::ExportXlsxAnonymized
                | DisclosureAction::ExportPxelAnonymized
                | DisclosureAction::ExportCsvAnonymized
        ) || book.rules.require_anonymization
            || shared
                .as_ref()
                .is_some_and(|s| s.rules.require_anonymization);
        let reason = if role.is_none()
            || (action == DisclosureAction::Cut
                && !matches!(role.as_deref(), Some("owner" | "editor" | "edit")))
        {
            DisclosureReason::RoleDenied
        } else if expected.is_some_and(|s| {
            s.book_revision != stamp.book_revision || s.link_revision != stamp.link_revision
        }) {
            DisclosureReason::PolicyChanged
        } else if expected.is_some_and(|s| s.document_version != stamp.document_version) {
            DisclosureReason::VersionChanged
        } else if !export
            && (!book.rules.allow_copy || shared.as_ref().is_some_and(|s| !s.rules.allow_copy))
        {
            DisclosureReason::CopyDenied
        } else if format.is_some_and(|format| {
            !book.rules.export_formats.contains(&format)
                || (role.as_deref() != Some("owner") && !book.rules.allow_participant_export)
                || shared.as_ref().is_some_and(|s| {
                    !s.rules.export_formats.contains(&format) || !s.rules.allow_participant_export
                })
        }) {
            DisclosureReason::FormatDenied
        } else if anonymized
            && (action == DisclosureAction::Cut || {
                let privacy = self
                    .read_privacy_policy(tx, doc, unsigned(row.access_epoch)?)
                    .await?;
                !privacy.rules.enabled || privacy.rules.kinds()?.is_empty()
            })
        {
            DisclosureReason::AnonymizationRequired
        } else {
            DisclosureReason::Allowed
        };
        Ok(DisclosureReceipt {
            schema_version: 1,
            allowed: reason == DisclosureReason::Allowed,
            reason,
            stamp,
            anonymized,
        })
    }
    pub async fn disclosure_capabilities(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<(bool, bool, bool, DisclosureStamp), DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        let copy = self
            .disclosure_decision(&mut tx, identity, doc, link, DisclosureAction::Copy, None)
            .await?;
        let export = self
            .disclosure_decision(
                &mut tx,
                identity,
                doc,
                link,
                DisclosureAction::ExportXlsx,
                None,
            )
            .await?;
        Ok((copy.allowed, export.allowed, copy.anonymized, copy.stamp))
    }
    pub async fn authorize_disclosure(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        action: DisclosureAction,
        expected: Option<DisclosureStamp>,
    ) -> Result<DisclosureReceipt, DisclosureError> {
        self.audit_disclosure(identity, doc, link, action, expected, None)
            .await
    }
    pub async fn can_export_pxel(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<bool, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        Ok(self
            .disclosure_decision(
                &mut tx,
                identity,
                doc,
                link,
                DisclosureAction::ExportPxel,
                None,
            )
            .await?
            .allowed)
    }
    pub async fn can_export_csv(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<bool, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        Ok(self
            .disclosure_decision(
                &mut tx,
                identity,
                doc,
                link,
                DisclosureAction::ExportCsv,
                None,
            )
            .await?
            .allowed)
    }
    pub async fn record_disclosure_failure(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        action: DisclosureAction,
        expected: DisclosureStamp,
    ) -> Result<(), DisclosureError> {
        self.audit_disclosure(
            identity,
            doc,
            link,
            action,
            Some(expected),
            Some(DisclosureReason::PrivacyAnalysisFailed),
        )
        .await?;
        Ok(())
    }
    pub async fn record_export_preparation_failure(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        action: DisclosureAction,
        expected: DisclosureStamp,
    ) -> Result<(), DisclosureError> {
        self.audit_disclosure(
            identity,
            doc,
            link,
            action,
            Some(expected),
            Some(DisclosureReason::PreparationFailed),
        )
        .await?;
        Ok(())
    }
    async fn audit_disclosure(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
        action: DisclosureAction,
        expected: Option<DisclosureStamp>,
        failure: Option<DisclosureReason>,
    ) -> Result<DisclosureReceipt, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let mut result = self
            .disclosure_decision(&mut tx, identity, doc, link, action, expected)
            .await?;
        if let Some(reason) = failure.filter(|_| result.allowed) {
            result.allowed = false;
            result.reason = reason;
        }
        let (action, format) = action.names();
        sqlx::query("INSERT INTO disclosure_audit_events(event_id,document_id,actor_id,link_id,document_version,book_revision,link_revision,action,format,allowed,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(Uuid::now_v7()).bind(id(doc.as_bytes())).bind(id(identity.actor_id.as_bytes())).bind(link).bind(number(result.stamp.document_version)?).bind(number(result.stamp.book_revision)?).bind(number(result.stamp.link_revision)?).bind(action).bind(format).bind(result.allowed).bind(result.reason.name()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn disclosure_audit(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        cursor: Option<Uuid>,
    ) -> Result<DisclosureAuditPage, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        self.disclosure_document(&mut tx, identity, doc, false)
            .await?;
        self.require_disclosure_owner(&mut tx, identity, doc, None)
            .await?;
        let mut events:Vec<DisclosureAuditEvent>=sqlx::query_as("SELECT event_id,actor_id,link_id,floor(extract(epoch FROM created_at))::bigint AS created_at,document_version,book_revision,link_revision,action,format,allowed,reason FROM disclosure_audit_events WHERE document_id=$1 AND ($2::uuid IS NULL OR event_id<$2) ORDER BY event_id DESC LIMIT 101").bind(id(doc.as_bytes())).bind(cursor).fetch_all(&mut *tx).await?;
        let next_cursor = if events.len() > 100 {
            events.truncate(100);
            events.last().map(|event| event.event_id)
        } else {
            None
        };
        Ok(DisclosureAuditPage {
            schema_version: 2,
            events,
            next_cursor,
        })
    }
}

#[cfg(test)]
mod tests;
