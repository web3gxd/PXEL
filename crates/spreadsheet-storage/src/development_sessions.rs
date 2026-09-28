use crate::{DatabaseError, PgDatabase};
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::{ActorId, WorkspaceId};
use uuid::Uuid;
#[derive(Clone, Copy, Debug)]
pub struct DevelopmentSession {
    pub identity: AuthenticatedActor,
    pub remaining_seconds: u64,
}
impl PgDatabase {
    /// Revokes only this bearer session. Account identity and other sessions survive.
    /// Repeated revocation succeeds without revealing whether the token existed.
    pub async fn revoke_development_session(&self, hash: [u8; 32]) -> Result<(), DatabaseError> {
        sqlx::query!(
            "DELETE FROM development_sessions WHERE token_hash=$1",
            &hash[..]
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn find_development_session(
        &self,
        hash: [u8; 32],
    ) -> Result<Option<DevelopmentSession>, DatabaseError> {
        let row=sqlx::query!(r#"SELECT user_id,workspace_id,ceil(extract(epoch FROM expires_at-now()))::bigint AS "remaining!" FROM development_sessions WHERE token_hash=$1 AND expires_at>now()"#,&hash[..]).fetch_optional(&self.pool).await?;
        row.map(|r| {
            Ok(DevelopmentSession {
                identity: AuthenticatedActor {
                    actor_id: ActorId::from_bytes(*r.user_id.as_bytes())
                        .map_err(|_| DatabaseError::SchemaNotReady)?,
                    workspace_id: WorkspaceId::from_bytes(*r.workspace_id.as_bytes())
                        .map_err(|_| DatabaseError::SchemaNotReady)?,
                },
                remaining_seconds: u64::try_from(r.remaining)
                    .map_err(|_| DatabaseError::SchemaNotReady)?,
            })
        })
        .transpose()
    }
    pub async fn issue_development_session(
        &self,
        hash: [u8; 32],
        actor: ActorId,
        workspace: WorkspaceId,
    ) -> Result<DevelopmentSession, DatabaseError> {
        let account = Uuid::from_bytes(*actor.as_bytes()).simple().to_string();
        self.issue_development_account_session(hash, actor, workspace, &account)
            .await
    }

    /// Explicitly insecure named local identities; callable only through dev-auth.
    /// The caller's newly generated IDs are used only when creating the named account.
    pub async fn issue_development_account_session(
        &self,
        hash: [u8; 32],
        actor: ActorId,
        workspace: WorkspaceId,
        account: &str,
    ) -> Result<DevelopmentSession, DatabaseError> {
        if account.is_empty()
            || account.len() > 32
            || !account
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let mut tx = self.pool.begin().await?;
        // Serialize only local development-session issuance, including the capacity check.
        sqlx::query!("SELECT pg_advisory_xact_lock(718034812)")
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM development_sessions WHERE expires_at<=now()")
            .execute(&mut *tx)
            .await?;
        let count = sqlx::query!(r#"SELECT count(*) AS "count!" FROM development_sessions"#)
            .fetch_one(&mut *tx)
            .await?;
        if count.count >= 128 {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let existing = sqlx::query!(
            "SELECT user_id,workspace_id FROM development_accounts WHERE account=$1",
            account
        )
        .fetch_optional(&mut *tx)
        .await?;
        let (actor, workspace) = if let Some(existing) = existing {
            (
                ActorId::from_bytes(*existing.user_id.as_bytes())
                    .map_err(|_| DatabaseError::SchemaNotReady)?,
                WorkspaceId::from_bytes(*existing.workspace_id.as_bytes())
                    .map_err(|_| DatabaseError::SchemaNotReady)?,
            )
        } else {
            sqlx::query!(
                "INSERT INTO users(user_id) VALUES($1)",
                Uuid::from_bytes(*actor.as_bytes())
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "INSERT INTO workspaces(workspace_id) VALUES($1)",
                Uuid::from_bytes(*workspace.as_bytes())
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')",
                Uuid::from_bytes(*workspace.as_bytes()),
                Uuid::from_bytes(*actor.as_bytes())
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "INSERT INTO development_accounts(account,user_id,workspace_id) VALUES($1,$2,$3)",
                account,
                Uuid::from_bytes(*actor.as_bytes()),
                Uuid::from_bytes(*workspace.as_bytes())
            )
            .execute(&mut *tx)
            .await?;
            (actor, workspace)
        };
        sqlx::query!(
            "INSERT INTO development_sessions(token_hash,user_id,workspace_id) VALUES($1,$2,$3)",
            &hash[..],
            Uuid::from_bytes(*actor.as_bytes()),
            Uuid::from_bytes(*workspace.as_bytes())
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(DevelopmentSession {
            identity: AuthenticatedActor {
                actor_id: actor,
                workspace_id: workspace,
            },
            remaining_seconds: 3600,
        })
    }
}
