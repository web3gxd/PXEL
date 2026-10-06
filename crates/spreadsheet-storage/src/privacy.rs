//! Encrypted owner recognition policy. Its changes also revise disclosure policy.
use crate::{DisclosureError, PgDatabase};
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::{privacy::SensitiveKind, workbook::DocumentId};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyRules {
    pub enabled: bool,
    pub types: Vec<String>,
    pub exceptions: Vec<String>,
}
impl PrivacyRules {
    pub fn kinds(&self) -> Result<Vec<SensitiveKind>, DisclosureError> {
        if self.types.len() > 6
            || self.exceptions.len() > 1000
            || self.exceptions.iter().any(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(DisclosureError::InvalidInput);
        }
        let kinds = self
            .types
            .iter()
            .map(|kind| match kind.as_str() {
                "phone" => Ok(SensitiveKind::Phone),
                "passport" => Ok(SensitiveKind::Passport),
                "snils" => Ok(SensitiveKind::Snils),
                "inn" => Ok(SensitiveKind::Inn),
                "email" => Ok(SensitiveKind::Email),
                "card" => Ok(SensitiveKind::Card),
                _ => Err(DisclosureError::InvalidInput),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if kinds
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != kinds.len()
            || self
                .exceptions
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.exceptions.len()
        {
            return Err(DisclosureError::InvalidInput);
        }
        Ok(if self.enabled { kinds } else { vec![] })
    }
}
#[derive(Clone, Serialize)]
pub struct PrivacyPolicy {
    pub schema_version: u32,
    pub revision: u64,
    pub rules: PrivacyRules,
    pub access_epoch: u64,
}
#[derive(FromRow)]
struct PolicyRow {
    revision: i64,
    payload: Vec<u8>,
}
fn id(doc: DocumentId) -> Uuid {
    Uuid::from_bytes(*doc.as_bytes())
}
impl PgDatabase {
    pub async fn share_guest_role(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Uuid,
    ) -> Result<bool, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        self.disclosure_document(&mut tx, identity, doc, false)
            .await?;
        let role = self
            .disclosure_role(&mut tx, identity, doc, Some(link))
            .await?
            .ok_or(DisclosureError::Unauthorized)?;
        Ok(role != "view")
    }
    pub(crate) async fn deny_anonymized_guest_write(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        identity: AuthenticatedActor,
        doc: DocumentId,
    ) -> Result<(), spreadsheet_application::journal::JournalError> {
        use spreadsheet_application::journal::JournalError;
        let link:Option<Uuid>=sqlx::query_scalar("SELECT s.link_id FROM share_guest_sessions s JOIN users u ON u.user_id=s.user_id AND u.identity_kind='share_guest' WHERE s.user_id=$1 AND s.document_id=$2 AND s.revoked_at IS NULL").bind(Uuid::from_bytes(*identity.actor_id.as_bytes())).bind(id(doc)).fetch_optional(&mut **tx).await.map_err(|_|JournalError::Unavailable)?;
        if let Some(link) = link {
            let book = self
                .read_disclosure_policy(tx, doc, None, 0)
                .await
                .map_err(|_| JournalError::Unavailable)?;
            let shared = self
                .read_disclosure_policy(tx, doc, Some(link), 0)
                .await
                .map_err(|_| JournalError::Unavailable)?;
            if book.rules.require_anonymization || shared.rules.require_anonymization {
                return Err(JournalError::Unauthorized);
            }
        }
        Ok(())
    }
    pub(crate) async fn read_privacy_policy(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        doc: DocumentId,
        epoch: u64,
    ) -> Result<PrivacyPolicy, DisclosureError> {
        let row: Option<PolicyRow> = sqlx::query_as(
            "SELECT revision,payload FROM document_privacy_policies WHERE document_id=$1",
        )
        .bind(id(doc))
        .fetch_optional(&mut **tx)
        .await?;
        let (revision, rules) = if let Some(row) = row {
            let revision = u64::try_from(row.revision).map_err(|_| DisclosureError::Corrupt)?;
            let bytes = self
                .open_record(
                    doc,
                    revision,
                    "privacy-policy",
                    id(doc),
                    1,
                    &row.payload,
                    128 * 1024,
                )
                .await
                .map_err(|_| DisclosureError::Corrupt)?;
            let rules: PrivacyRules =
                serde_json::from_slice(&bytes).map_err(|_| DisclosureError::Corrupt)?;
            rules.kinds().map_err(|_| DisclosureError::Corrupt)?;
            (revision, rules)
        } else {
            (0, PrivacyRules::default())
        };
        Ok(PrivacyPolicy {
            schema_version: 1,
            revision,
            rules,
            access_epoch: epoch,
        })
    }
    pub async fn privacy_policy(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<PrivacyPolicy, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        let row = self
            .disclosure_document(&mut tx, identity, doc, false)
            .await?;
        if self
            .disclosure_role(&mut tx, identity, doc, link)
            .await?
            .is_none()
        {
            return Err(DisclosureError::Unauthorized);
        }
        self.read_privacy_policy(
            &mut tx,
            doc,
            u64::try_from(row.access_epoch).map_err(|_| DisclosureError::Corrupt)?,
        )
        .await
    }
    pub async fn set_privacy_policy(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        expected_head: u64,
        expected_revision: u64,
        rules: PrivacyRules,
    ) -> Result<PrivacyPolicy, DisclosureError> {
        rules.kinds()?;
        let mut tx = self.begin_guarded().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let row = self
            .disclosure_document(&mut tx, identity, doc, true)
            .await?;
        self.require_disclosure_owner(&mut tx, identity, doc, None)
            .await?;
        let epoch = u64::try_from(row.access_epoch).map_err(|_| DisclosureError::Corrupt)?;
        let previous = self.read_privacy_policy(&mut tx, doc, epoch).await?;
        if previous.revision != expected_revision
            || u64::try_from(row.head_version).ok() != Some(expected_head)
        {
            return Err(DisclosureError::Conflict);
        }
        let revision = expected_revision
            .checked_add(1)
            .ok_or(DisclosureError::InvalidInput)?;
        let bytes =
            Zeroizing::new(serde_json::to_vec(&rules).map_err(|_| DisclosureError::InvalidInput)?);
        let payload = self
            .seal_record_guarded(
                doc,
                revision,
                "privacy-policy",
                id(doc),
                1,
                &bytes,
                128 * 1024,
            )
            .await
            .map_err(|_| DisclosureError::Unavailable)?;
        sqlx::query("INSERT INTO document_privacy_policies(document_id,revision,payload) VALUES($1,$2,$3) ON CONFLICT(document_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload").bind(id(doc)).bind(i64::try_from(revision).map_err(|_| DisclosureError::InvalidInput)?).bind(payload).execute(&mut *tx).await?;
        // Privacy affects every issued copy. Preserve rules while changing their stamp.
        let policy = self
            .read_disclosure_policy(&mut tx, doc, None, epoch)
            .await?;
        let policy_revision = policy
            .revision
            .checked_add(1)
            .ok_or(DisclosureError::InvalidInput)?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&policy.rules).map_err(|_| DisclosureError::Corrupt)?,
        );
        let payload = self
            .seal_record_guarded(
                doc,
                policy_revision,
                "disclosure-policy",
                id(doc),
                1,
                &bytes,
                4096,
            )
            .await
            .map_err(|_| DisclosureError::Unavailable)?;
        sqlx::query("INSERT INTO disclosure_policies(scope_id,document_id,link_id,revision,payload) VALUES($1,$1,NULL,$2,$3) ON CONFLICT(scope_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload").bind(id(doc)).bind(i64::try_from(policy_revision).map_err(|_| DisclosureError::InvalidInput)?).bind(payload).execute(&mut *tx).await?;
        let next_epoch = row
            .access_epoch
            .checked_add(1)
            .ok_or(DisclosureError::Corrupt)?;
        sqlx::query("UPDATE documents SET access_epoch=$1 WHERE document_id=$2")
            .bind(next_epoch)
            .bind(id(doc))
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE share_guest_sessions SET revoked_at=clock_timestamp() WHERE document_id=$1 AND revoked_at IS NULL").bind(id(doc)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(PrivacyPolicy {
            schema_version: 1,
            revision,
            rules,
            access_epoch: u64::try_from(next_epoch).map_err(|_| DisclosureError::Corrupt)?,
        })
    }
    pub async fn requires_anonymization(
        &self,
        identity: AuthenticatedActor,
        doc: DocumentId,
        link: Option<Uuid>,
    ) -> Result<bool, DisclosureError> {
        let mut tx = self.begin_guarded().await?;
        let row = self
            .disclosure_document(&mut tx, identity, doc, false)
            .await?;
        if self
            .disclosure_role(&mut tx, identity, doc, link)
            .await?
            .is_none()
        {
            return Err(DisclosureError::Unauthorized);
        }
        let epoch = u64::try_from(row.access_epoch).map_err(|_| DisclosureError::Corrupt)?;
        let book = self
            .read_disclosure_policy(&mut tx, doc, None, epoch)
            .await?;
        let shared = if let Some(link) = link {
            self.read_disclosure_policy(&mut tx, doc, Some(link), epoch)
                .await?
                .rules
                .require_anonymization
        } else {
            false
        };
        Ok(book.rules.require_anonymization || shared)
    }
}
