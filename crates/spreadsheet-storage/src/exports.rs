use crate::PgDatabase;
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::{DocumentId, DocumentVersion};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ExportAuditError {
    #[error("export is not authorized for this document")]
    Unauthorized,
    #[error("export audit storage is unavailable")]
    Unavailable,
}

impl PgDatabase {
    /// Rejects non-owners before a bounded export conversion consumes CPU.
    /// `record_export` repeats the authorization inside its audit transaction.
    pub async fn authorize_export(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> Result<(), ExportAuditError> {
        let allowed = sqlx::query_scalar!(
            r#"SELECT EXISTS(
                SELECT 1 FROM documents d
                JOIN document_permissions p ON p.document_id=d.document_id
                JOIN memberships m ON m.workspace_id=d.workspace_id AND m.user_id=p.user_id
                WHERE d.document_id=$1 AND d.workspace_id=$2 AND d.state='active'
                  AND p.user_id=$3 AND p.role='owner'
            ) AS "allowed!""#,
            Uuid::from_bytes(*document.as_bytes()),
            Uuid::from_bytes(*identity.workspace_id.as_bytes()),
            Uuid::from_bytes(*identity.actor_id.as_bytes())
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|_| ExportAuditError::Unavailable)?;
        if allowed {
            Ok(())
        } else {
            Err(ExportAuditError::Unauthorized)
        }
    }

    /// Audits an owner-only prepared export before any bytes are returned.
    pub async fn record_export(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        version: DocumentVersion,
        checksum: [u8; 32],
        size: usize,
    ) -> Result<(), ExportAuditError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| ExportAuditError::Unavailable)?;
        let owner = sqlx::query!(
            r#"SELECT d.head_version FROM documents d
               JOIN document_permissions p ON p.document_id=d.document_id
               JOIN memberships m ON m.workspace_id=d.workspace_id AND m.user_id=p.user_id
               WHERE d.document_id=$1 AND d.workspace_id=$2 AND d.state='active'
                 AND p.user_id=$3 AND p.role='owner'
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
        let size = i64::try_from(size).map_err(|_| ExportAuditError::Unavailable)?;
        sqlx::query!(
            "INSERT INTO export_audit_events(event_id,workspace_id,document_id,actor_id,document_version,payload_checksum,payload_size) VALUES($1,$2,$3,$4,$5,$6,$7)",
            Uuid::now_v7(),
            Uuid::from_bytes(*identity.workspace_id.as_bytes()),
            Uuid::from_bytes(*document.as_bytes()),
            Uuid::from_bytes(*identity.actor_id.as_bytes()),
            i64::try_from(version.get()).map_err(|_| ExportAuditError::Unavailable)?,
            checksum.as_slice(),
            size
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
        let db = PgDatabase { pool };
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
        let record = sqlx::query!(
            "SELECT actor_id,document_version,payload_size,payload_checksum FROM export_audit_events WHERE document_id=$1",
            Uuid::from_bytes(*document.as_bytes())
        ).fetch_one(&db.pool).await?;
        assert_eq!(record.actor_id, Uuid::from_bytes(*owner.as_bytes()));
        assert_eq!(record.document_version, 1);
        assert_eq!(record.payload_size, 101);
        assert_eq!(record.payload_checksum, vec![8; 32]);
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
