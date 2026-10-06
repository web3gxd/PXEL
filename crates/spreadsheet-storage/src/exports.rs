use crate::{CryptoError, PgDatabase};
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::{DocumentId, DocumentVersion};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_EXPORT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ExportAuditError {
    #[error("export is not authorized for this document")]
    Unauthorized,
    #[error("export audit storage is unavailable")]
    Unavailable,
    #[error("export audit encryption failed")]
    Crypto(#[from] CryptoError),
}

impl PgDatabase {
    /// Checks the effective disclosure policy before export conversion consumes CPU.
    /// `record_export` repeats the authorization inside its audit transaction.
    pub async fn authorize_export(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> Result<crate::DisclosureReceipt, ExportAuditError> {
        let decision = self
            .authorize_disclosure(
                identity,
                document,
                None,
                crate::DisclosureAction::ExportXlsx,
                None,
            )
            .await
            .map_err(|error| match error {
                crate::DisclosureError::Unauthorized => ExportAuditError::Unauthorized,
                _ => ExportAuditError::Unavailable,
            })?;
        if decision.allowed {
            Ok(decision)
        } else {
            Err(ExportAuditError::Unauthorized)
        }
    }

    /// Audits a prepared export before any bytes are returned.
    pub async fn record_export(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        version: DocumentVersion,
        checksum: [u8; 32],
        size: usize,
    ) -> Result<(), ExportAuditError> {
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| ExportAuditError::Unavailable)?;
        let decision = self
            .disclosure_decision(
                &mut tx,
                identity,
                document,
                None,
                crate::DisclosureAction::ExportXlsx,
                None,
            )
            .await
            .map_err(|_| ExportAuditError::Unavailable)?;
        if !decision.allowed {
            return Err(ExportAuditError::Unauthorized);
        }
        let owner = sqlx::query!(
            r#"SELECT d.head_version FROM documents d
               JOIN document_permissions p ON p.document_id=d.document_id
               JOIN memberships m ON m.workspace_id=d.workspace_id AND m.user_id=p.user_id
               WHERE d.document_id=$1 AND d.workspace_id=$2 AND d.state='active'
                 AND p.user_id=$3
               FOR SHARE OF d"#,
            Uuid::from_bytes(*document.as_bytes()),
            Uuid::from_bytes(*identity.workspace_id.as_bytes()),
            Uuid::from_bytes(*identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ExportAuditError::Unavailable)?
        .ok_or(ExportAuditError::Unauthorized)?;
        if owner.head_version
            < i64::try_from(version.get()).map_err(|_| ExportAuditError::Unavailable)?
        {
            return Err(ExportAuditError::Unavailable);
        }
        if size == 0 || size > MAX_EXPORT_BYTES {
            return Err(ExportAuditError::Unavailable);
        }
        let size = u64::try_from(size).map_err(|_| ExportAuditError::Unavailable)?;
        let event_id = Uuid::now_v7();
        let mut audit = Zeroizing::new([0_u8; 40]);
        audit[..32].copy_from_slice(&checksum);
        audit[32..].copy_from_slice(&size.to_be_bytes());
        let encrypted_audit = self
            .seal_record_guarded(
                document,
                version.get(),
                "export-audit",
                event_id,
                1,
                &audit[..],
                audit.len(),
            )
            .await?;
        sqlx::query!(
            "INSERT INTO export_audit_events(event_id,workspace_id,document_id,actor_id,document_version,payload_checksum,payload_size) VALUES($1,$2,$3,$4,$5,$6,$7)",
            event_id,
            Uuid::from_bytes(*identity.workspace_id.as_bytes()),
            Uuid::from_bytes(*document.as_bytes()),
            Uuid::from_bytes(*identity.actor_id.as_bytes()),
            i64::try_from(version.get()).map_err(|_| ExportAuditError::Unavailable)?,
            &encrypted_audit,
            0_i64
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| ExportAuditError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ExportAuditError::Unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::{ActorId, WorkspaceId};

    #[sqlx::test(migrations = false)]
    async fn only_document_owner_can_append_export_audit(
        pool: sqlx::PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let owner = ActorId::from_bytes([1; 16])?;
        let viewer = ActorId::from_bytes([2; 16])?;
        let workspace = WorkspaceId::from_bytes([3; 16])?;
        let document = DocumentId::from_bytes([4; 16])?;
        for user in [owner, viewer] {
            sqlx::query("INSERT INTO users(user_id) VALUES($1)")
                .bind(Uuid::from_bytes(*user.as_bytes()))
                .execute(&db.pool)
                .await?;
        }
        sqlx::query("INSERT INTO workspaces(workspace_id) VALUES($1)")
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .execute(&db.pool)
            .await?;
        for user in [owner, viewer] {
            sqlx::query(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')",
            )
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .bind(Uuid::from_bytes(*user.as_bytes()))
            .execute(&db.pool)
            .await?;
        }
        sqlx::query("INSERT INTO documents(document_id,workspace_id,head_version) VALUES($1,$2,1)")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .execute(&db.pool)
            .await?;
        db.ensure_document_key(workspace, document).await?;
        for (user, role) in [(owner, "owner"), (viewer, "viewer")] {
            sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,$4)")
                .bind(Uuid::from_bytes(*workspace.as_bytes()))
                .bind(Uuid::from_bytes(*document.as_bytes()))
                .bind(Uuid::from_bytes(*user.as_bytes()))
                .bind(role).execute(&db.pool).await?;
        }
        let version = DocumentVersion::new(1)?;
        let identity = |actor_id| AuthenticatedActor {
            actor_id,
            workspace_id: workspace,
        };
        assert!(matches!(
            db.authorize_export(identity(viewer), document).await,
            Err(ExportAuditError::Unauthorized)
        ));
        db.authorize_export(identity(owner), document).await?;
        assert!(matches!(
            db.record_export(identity(viewer), document, version, [7; 32], 100)
                .await,
            Err(ExportAuditError::Unauthorized)
        ));
        db.record_export(identity(owner), document, version, [8; 32], 101)
            .await?;
        for size in [0, MAX_EXPORT_BYTES + 1] {
            assert!(matches!(
                db.record_export(identity(owner), document, version, [8; 32], size)
                    .await,
                Err(ExportAuditError::Unavailable)
            ));
        }
        let record = sqlx::query!(
            "SELECT event_id,actor_id,document_version,payload_size,payload_checksum FROM export_audit_events WHERE document_id=$1",
            Uuid::from_bytes(*document.as_bytes())
        ).fetch_one(&db.pool).await?;
        assert_eq!(record.actor_id, Uuid::from_bytes(*owner.as_bytes()));
        assert_eq!(record.document_version, 1);
        assert_eq!(record.payload_size, 0);
        assert_ne!(record.payload_checksum, vec![8; 32]);
        let audit = db
            .open_record(
                document,
                version.get(),
                "export-audit",
                record.event_id,
                1,
                &record.payload_checksum,
                40,
            )
            .await?;
        assert_eq!(&audit[..32], &[8; 32]);
        assert_eq!(&audit[32..], &101_u64.to_be_bytes());
        assert!(matches!(
            db.open_record(
                document,
                version.get(),
                "export-audit",
                Uuid::now_v7(),
                1,
                &record.payload_checksum,
                40,
            )
            .await,
            Err(CryptoError::AuthenticationFailed)
        ));
        db.record_export(identity(owner), document, version, [8; 32], 101)
            .await?;
        let retries = sqlx::query_scalar!(
            "SELECT count(*) FROM export_audit_events WHERE document_id=$1",
            Uuid::from_bytes(*document.as_bytes())
        )
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(retries, Some(2));
        assert!(
            sqlx::query("UPDATE export_audit_events SET payload_size=1 WHERE event_id=$1")
                .bind(record.event_id)
                .execute(&db.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM export_audit_events WHERE document_id=$1")
                .bind(Uuid::from_bytes(*document.as_bytes()))
                .execute(&db.pool)
                .await
                .is_err()
        );
        Ok(())
    }
}
