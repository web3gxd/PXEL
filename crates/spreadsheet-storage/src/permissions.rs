use crate::PgDatabase;
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::{ActorId, AuthorizationChangeId, DocumentId};
use sqlx::Error as SqlxError;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionRole {
    Editor,
    Viewer,
}

impl PermissionRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionReceipt {
    pub change_id: AuthorizationChangeId,
    pub access_epoch: u64,
    pub newly_committed: bool,
}

#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("permission change is unauthorized")]
    Unauthorized,
    #[error("document or target membership does not exist")]
    NotFound,
    #[error("permission change identifier reused with different intent")]
    IdempotencyConflict,
    #[error("permission change cannot modify the owner")]
    OwnerImmutable,
    #[error("permission transaction conflicted; retry the whole command")]
    Conflict,
    #[error("permission store is unavailable")]
    Unavailable,
    #[error("permission store contained invalid data")]
    Corrupt,
}

fn map_error(error: SqlxError) -> PermissionError {
    match error {
        SqlxError::Database(database)
            if matches!(
                database.code().as_deref(),
                Some("40001" | "40P01" | "55P03" | "23505")
            ) =>
        {
            PermissionError::Conflict
        }
        _ => PermissionError::Unavailable,
    }
}

fn uuid(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}

impl PgDatabase {
    /// Called while the server holds the document's outbound write gate.
    /// Every accepted intent is one atomic ACL + epoch + append-only audit transaction.
    pub async fn change_permission(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
        target: ActorId,
        change_id: AuthorizationChangeId,
        role: Option<PermissionRole>,
    ) -> Result<PermissionReceipt, PermissionError> {
        let mut tx = self.pool.begin().await.map_err(map_error)?;
        let member = sqlx::query!(
            "SELECT role FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
            uuid(identity.workspace_id.as_bytes()),
            uuid(identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if member.is_none() {
            return Err(PermissionError::Unauthorized);
        }
        let target_user = sqlx::query!(
            "SELECT user_id FROM users WHERE user_id=$1 FOR SHARE",
            uuid(target.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if target_user.is_none() {
            return Err(PermissionError::NotFound);
        }
        let target_membership = sqlx::query!(
            "SELECT user_id FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
            uuid(identity.workspace_id.as_bytes()),
            uuid(target.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        let apparent_owner = sqlx::query!(
            "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2",
            uuid(document.as_bytes()),
            uuid(identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if !apparent_owner.is_some_and(|permission| permission.role == "owner") {
            return Err(PermissionError::Unauthorized);
        }
        if target_membership.is_none() {
            if role.is_none() {
                return Err(PermissionError::NotFound);
            }
            // Invite existing user to this workspace before taking the document lock.
            sqlx::query!(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member') ON CONFLICT(workspace_id,user_id) DO NOTHING",
                uuid(identity.workspace_id.as_bytes()),
                uuid(target.as_bytes())
            )
            .execute(&mut *tx)
            .await
            .map_err(map_error)?;
        }
        let doc = sqlx::query!(
            "SELECT access_epoch FROM documents WHERE workspace_id=$1 AND document_id=$2 AND state='active' FOR UPDATE",
            uuid(identity.workspace_id.as_bytes()),
            uuid(document.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?
        .ok_or(PermissionError::NotFound)?;
        let owner = sqlx::query!(
            "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2",
            uuid(document.as_bytes()),
            uuid(identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if !owner.is_some_and(|permission| permission.role == "owner") {
            return Err(PermissionError::Unauthorized);
        }
        if target == identity.actor_id {
            return Err(PermissionError::OwnerImmutable);
        }
        let prior = sqlx::query!(
            "SELECT actor_id,target_user_id,new_role,access_epoch,document_id FROM audit_events WHERE event_id=$1",
            uuid(change_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if let Some(prior) = prior {
            if prior.actor_id != uuid(identity.actor_id.as_bytes())
                || prior.target_user_id != uuid(target.as_bytes())
                || prior.document_id != uuid(document.as_bytes())
                || prior.new_role.as_deref() != role.map(PermissionRole::as_str)
            {
                return Err(PermissionError::IdempotencyConflict);
            }
            return Ok(PermissionReceipt {
                change_id,
                access_epoch: u64::try_from(prior.access_epoch)
                    .map_err(|_| PermissionError::Corrupt)?,
                newly_committed: false,
            });
        }
        let old = sqlx::query!(
            "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2",
            uuid(document.as_bytes()),
            uuid(target.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?;
        if old
            .as_ref()
            .is_some_and(|permission| permission.role == "owner")
        {
            return Err(PermissionError::OwnerImmutable);
        }
        if let Some(role) = role {
            sqlx::query!(
                "INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,$4) ON CONFLICT(document_id,user_id) DO UPDATE SET role=EXCLUDED.role",
                uuid(identity.workspace_id.as_bytes()),
                uuid(document.as_bytes()),
                uuid(target.as_bytes()),
                role.as_str()
            )
            .execute(&mut *tx)
            .await
            .map_err(map_error)?;
        } else {
            sqlx::query!(
                "DELETE FROM document_permissions WHERE document_id=$1 AND user_id=$2",
                uuid(document.as_bytes()),
                uuid(target.as_bytes())
            )
            .execute(&mut *tx)
            .await
            .map_err(map_error)?;
        }
        let epoch = doc
            .access_epoch
            .checked_add(1)
            .ok_or(PermissionError::Corrupt)?;
        let updated = sqlx::query!(
            "UPDATE documents SET access_epoch=$1 WHERE document_id=$2 AND access_epoch=$3",
            epoch,
            uuid(document.as_bytes()),
            doc.access_epoch
        )
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
        if updated.rows_affected() != 1 {
            return Err(PermissionError::Conflict);
        }
        sqlx::query!(
            "INSERT INTO audit_events(event_id,workspace_id,document_id,actor_id,target_user_id,event_type,old_role,new_role,access_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            uuid(change_id.as_bytes()),
            uuid(identity.workspace_id.as_bytes()),
            uuid(document.as_bytes()),
            uuid(identity.actor_id.as_bytes()),
            uuid(target.as_bytes()),
            if role.is_some() { "permission_granted" } else { "permission_revoked" },
            old.map(|permission| permission.role),
            role.map(PermissionRole::as_str),
            epoch
        )
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
        tx.commit().await.map_err(map_error)?;
        Ok(PermissionReceipt {
            change_id,
            access_epoch: u64::try_from(epoch).map_err(|_| PermissionError::Corrupt)?,
            newly_committed: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_application::journal::OperationJournal;
    use spreadsheet_domain::workbook::WorkspaceId;

    #[sqlx::test(migrations = false)]
    async fn acl_mutation_is_atomic_audited_idempotent_and_revokes_access(
        pool: sqlx::PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let db = PgDatabase { pool };
        db.migrate().await?;
        let owner = ActorId::from_bytes([1; 16])?;
        let target = ActorId::from_bytes([2; 16])?;
        let workspace = WorkspaceId::from_bytes([3; 16])?;
        let document = DocumentId::from_bytes([4; 16])?;
        for user in [owner, target] {
            sqlx::query("INSERT INTO users(user_id) VALUES($1)")
                .bind(uuid(user.as_bytes()))
                .execute(&db.pool)
                .await?;
        }
        sqlx::query("INSERT INTO workspaces(workspace_id) VALUES($1)")
            .bind(uuid(workspace.as_bytes()))
            .execute(&db.pool)
            .await?;
        for (user, role) in [(owner, "owner")] {
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,$3)")
                .bind(uuid(workspace.as_bytes()))
                .bind(uuid(user.as_bytes()))
                .bind(role)
                .execute(&db.pool)
                .await?;
        }
        sqlx::query("INSERT INTO documents(document_id,workspace_id) VALUES($1,$2)")
            .bind(uuid(document.as_bytes()))
            .bind(uuid(workspace.as_bytes()))
            .execute(&db.pool)
            .await?;
        sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'owner')")
            .bind(uuid(workspace.as_bytes()))
            .bind(uuid(document.as_bytes()))
            .bind(uuid(owner.as_bytes()))
            .execute(&db.pool)
            .await?;
        let identity = AuthenticatedActor {
            actor_id: owner,
            workspace_id: workspace,
        };
        let recipient = AuthenticatedActor {
            actor_id: target,
            workspace_id: workspace,
        };
        let first = AuthorizationChangeId::from_bytes([5; 16])?;
        let receipt = db
            .change_permission(
                identity,
                document,
                target,
                first,
                Some(PermissionRole::Viewer),
            )
            .await?;
        assert_eq!(receipt.access_epoch, 1);
        assert!(!db.access(recipient, document).await?.can_edit);
        let retry = db
            .change_permission(
                identity,
                document,
                target,
                first,
                Some(PermissionRole::Viewer),
            )
            .await?;
        assert_eq!(retry.access_epoch, receipt.access_epoch);
        assert!(!retry.newly_committed);
        assert!(matches!(
            db.change_permission(
                identity,
                document,
                target,
                first,
                Some(PermissionRole::Editor)
            )
            .await,
            Err(PermissionError::IdempotencyConflict)
        ));
        assert!(matches!(
            db.change_permission(
                recipient,
                document,
                owner,
                AuthorizationChangeId::from_bytes([6; 16])?,
                None
            )
            .await,
            Err(PermissionError::Unauthorized)
        ));
        assert!(matches!(
            db.change_permission(
                identity,
                document,
                owner,
                AuthorizationChangeId::from_bytes([9; 16])?,
                None
            )
            .await,
            Err(PermissionError::OwnerImmutable)
        ));
        let next = db
            .change_permission(
                identity,
                document,
                target,
                AuthorizationChangeId::from_bytes([7; 16])?,
                Some(PermissionRole::Editor),
            )
            .await?;
        assert_eq!(next.access_epoch, 2);
        assert!(db.access(recipient, document).await?.can_edit);
        let revoked = db
            .change_permission(
                identity,
                document,
                target,
                AuthorizationChangeId::from_bytes([8; 16])?,
                None,
            )
            .await?;
        assert_eq!(revoked.access_epoch, 3);
        assert!(db.access(recipient, document).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(count, 3);
        assert!(
            sqlx::query("UPDATE audit_events SET new_role='viewer'")
                .execute(&db.pool)
                .await
                .is_err()
        );
        Ok(())
    }
}
