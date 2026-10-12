//! Durable, single-instance self-hosted accounts. Password work belongs to the
//! bounded HTTP adapter; this module independently validates its stored verifier.
use crate::{PgDatabase, ShareSecretKind, ShareSecretPhc};
use serde::Serialize;
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_domain::workbook::{ActorId, WorkspaceId};
use sqlx::{FromRow, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const ACCOUNT_LOCK: i64 = 1_347_962_199;

#[derive(Debug, Error)]
pub enum LocalAccountError {
    #[error("local account input is invalid")]
    InvalidInput,
    #[error("local instance is already initialized")]
    AlreadyInitialized,
    #[error("existing documents require an explicit migration")]
    ExistingDocuments,
    #[error("local account action is unauthorized")]
    Unauthorized,
    #[error("local account capacity reached")]
    Capacity,
    #[error("local account already exists")]
    Conflict,
    #[error("master account is protected")]
    MasterProtected,
    #[error("local account store is unavailable")]
    Unavailable,
}
impl From<sqlx::Error> for LocalAccountError {
    fn from(error: sqlx::Error) -> Self {
        if matches!(&error, sqlx::Error::Database(db) if db.code().as_deref()==Some("23505")) {
            Self::Conflict
        } else {
            Self::Unavailable
        }
    }
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct LocalAccount {
    pub user_id: Uuid,
    pub username: String,
    pub is_master: bool,
    pub enabled: bool,
    pub must_change_password: bool,
}

pub struct LocalCredential {
    pub account: LocalAccount,
    pub credential_epoch: i64,
    pub phc: Option<Zeroizing<String>>,
}

#[derive(Clone)]
pub struct LocalSession {
    pub identity: AuthenticatedActor,
    pub account: LocalAccount,
    pub credential_epoch: i64,
    pub remaining_seconds: u64,
}

#[derive(FromRow)]
struct CredentialRow {
    user_id: Uuid,
    workspace_id: Uuid,
    username: String,
    is_master: bool,
    enabled: bool,
    must_change_password: bool,
    credential_epoch: i64,
    password_phc: String,
    unlocked: bool,
}
impl CredentialRow {
    fn account(&self) -> LocalAccount {
        LocalAccount {
            user_id: self.user_id,
            username: self.username.clone(),
            is_master: self.is_master,
            enabled: self.enabled,
            must_change_password: self.must_change_password,
        }
    }
    fn identity(&self) -> Result<AuthenticatedActor, LocalAccountError> {
        Ok(AuthenticatedActor {
            actor_id: ActorId::from_bytes(*self.user_id.as_bytes())
                .map_err(|_| LocalAccountError::Unavailable)?,
            workspace_id: WorkspaceId::from_bytes(*self.workspace_id.as_bytes())
                .map_err(|_| LocalAccountError::Unavailable)?,
        })
    }
}
impl Drop for CredentialRow {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.password_phc);
    }
}

pub fn valid_local_username(value: &str) -> bool {
    (3..=32).contains(&value.len())
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}
pub fn valid_local_password(value: &str) -> bool {
    value.len() <= 512 && (12..=128).contains(&value.chars().count())
}
fn validate_phc(phc: &Zeroizing<String>) -> Result<(), LocalAccountError> {
    ShareSecretPhc::new(ShareSecretKind::Password, phc.clone())
        .map(|_| ())
        .map_err(|_| LocalAccountError::InvalidInput)
}
async fn lock_accounts(tx: &mut Transaction<'_, Postgres>) -> Result<(), LocalAccountError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ACCOUNT_LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn require_master(
    tx: &mut Transaction<'_, Postgres>,
    session_hash: [u8; 32],
) -> Result<(), LocalAccountError> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_sessions s JOIN local_accounts a ON a.user_id=s.user_id JOIN local_instance i ON i.master_user_id=a.user_id AND i.workspace_id=a.workspace_id WHERE s.token_hash=$1 AND s.expires_at>clock_timestamp() AND s.credential_epoch=a.credential_epoch AND a.enabled AND a.is_master AND NOT a.must_change_password)")
        .bind(&session_hash[..]).fetch_one(&mut **tx).await?;
    if allowed {
        Ok(())
    } else {
        Err(LocalAccountError::Unauthorized)
    }
}
async fn insert_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    epoch: i64,
    hash: [u8; 32],
) -> Result<(), LocalAccountError> {
    sqlx::query("DELETE FROM account_sessions WHERE expires_at<=clock_timestamp()")
        .execute(&mut **tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_sessions")
        .fetch_one(&mut **tx)
        .await?;
    if count >= 10000 {
        return Err(LocalAccountError::Capacity);
    }
    sqlx::query(
        "INSERT INTO account_sessions(token_hash,user_id,credential_epoch) VALUES($1,$2,$3)",
    )
    .bind(&hash[..])
    .bind(user_id)
    .bind(epoch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl PgDatabase {
    pub async fn local_instance_initialized(&self) -> Result<bool, LocalAccountError> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_instance)")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn local_setup_available(&self) -> Result<bool, LocalAccountError> {
        Ok(sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM local_instance) AND NOT EXISTS(SELECT 1 FROM documents)").fetch_one(&self.pool).await?)
    }
    /// PostgreSQL table lock serializes bootstrap with document creation and an
    /// advisory lock serializes all account/session mutations across processes.
    pub async fn bootstrap_local_instance(
        &self,
        username: &str,
        phc: Zeroizing<String>,
        session_hash: [u8; 32],
    ) -> Result<LocalAccount, LocalAccountError> {
        if !valid_local_username(username) {
            return Err(LocalAccountError::InvalidInput);
        }
        validate_phc(&phc)?;
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        sqlx::query("LOCK TABLE documents IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM local_instance)")
            .fetch_one(&mut *tx)
            .await?
        {
            return Err(LocalAccountError::AlreadyInitialized);
        }
        if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM documents)")
            .fetch_one(&mut *tx)
            .await?
        {
            return Err(LocalAccountError::ExistingDocuments);
        }
        let user = Uuid::now_v7();
        let workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO users(user_id) VALUES($1)")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO workspaces(workspace_id) VALUES($1)")
            .bind(workspace)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')")
            .bind(workspace)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO local_accounts(user_id,workspace_id,username,password_phc,is_master,must_change_password) VALUES($1,$2,$3,$4,true,false)").bind(user).bind(workspace).bind(username).bind(phc.as_str()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO local_instance(workspace_id,master_user_id) VALUES($1,$2)")
            .bind(workspace)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        insert_session(&mut tx, user, 0, session_hash).await?;
        tx.commit().await?;
        Ok(LocalAccount {
            user_id: user,
            username: username.into(),
            is_master: true,
            enabled: true,
            must_change_password: false,
        })
    }
    pub async fn local_credential(
        &self,
        username: &str,
    ) -> Result<Option<LocalCredential>, LocalAccountError> {
        let row = sqlx::query_as::<_,CredentialRow>("SELECT a.user_id,a.workspace_id,a.username,a.is_master,a.enabled,a.must_change_password,a.credential_epoch,a.password_phc,(a.locked_until IS NULL OR a.locked_until<=clock_timestamp()) AS unlocked FROM local_accounts a JOIN local_instance i ON i.workspace_id=a.workspace_id WHERE a.username=$1")
            .bind(username).fetch_optional(&self.pool).await?;
        row.map(|row| {
            let _identity = row.identity()?;
            let phc = Zeroizing::new(row.password_phc.clone());
            validate_phc(&phc).map_err(|_| LocalAccountError::Unavailable)?;
            Ok(LocalCredential {
                account: row.account(),
                credential_epoch: row.credential_epoch,
                phc: (row.enabled && row.unlocked).then_some(phc),
            })
        })
        .transpose()
    }
    /// Login completion rechecks the exact verified epoch/credential under the
    /// mutation lock. A concurrent reset can never issue a session for the old hash.
    pub async fn complete_local_login(
        &self,
        credential: Option<&LocalCredential>,
        valid: bool,
        hash: [u8; 32],
    ) -> Result<Option<LocalAccount>, LocalAccountError> {
        let Some(credential) = credential else {
            return Ok(None);
        };
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        let row=sqlx::query_as::<_,CredentialRow>("SELECT user_id,workspace_id,username,is_master,enabled,must_change_password,credential_epoch,password_phc,(locked_until IS NULL OR locked_until<=clock_timestamp()) AS unlocked FROM local_accounts WHERE user_id=$1 FOR UPDATE")
            .bind(credential.account.user_id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else { return Ok(None) };
        if !row.enabled
            || !row.unlocked
            || row.credential_epoch != credential.credential_epoch
            || credential.phc.as_deref().map(String::as_str) != Some(row.password_phc.as_str())
        {
            return Ok(None);
        }
        if !valid {
            sqlx::query("UPDATE local_accounts SET failed_attempts=CASE WHEN failed_window_started_at IS NULL OR failed_window_started_at<=clock_timestamp()-interval '15 minutes' THEN 1 ELSE LEAST(5,failed_attempts+1) END, failed_window_started_at=CASE WHEN failed_window_started_at IS NULL OR failed_window_started_at<=clock_timestamp()-interval '15 minutes' THEN clock_timestamp() ELSE failed_window_started_at END, locked_until=CASE WHEN failed_window_started_at>clock_timestamp()-interval '15 minutes' AND failed_attempts>=4 THEN clock_timestamp()+interval '15 minutes' ELSE NULL END WHERE user_id=$1")
                .bind(row.user_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE local_accounts SET failed_attempts=0,failed_window_started_at=NULL,locked_until=NULL WHERE user_id=$1").bind(row.user_id).execute(&mut *tx).await?;
        insert_session(&mut tx, row.user_id, row.credential_epoch, hash).await?;
        let account = row.account();
        tx.commit().await?;
        Ok(Some(account))
    }
    pub async fn local_session(
        &self,
        hash: [u8; 32],
    ) -> Result<Option<LocalSession>, LocalAccountError> {
        let row=sqlx::query_as::<_,(Uuid,Uuid,String,bool,bool,bool,i64,i64)>("SELECT a.user_id,a.workspace_id,a.username,a.is_master,a.enabled,a.must_change_password,a.credential_epoch,ceil(extract(epoch FROM s.expires_at-clock_timestamp()))::bigint FROM account_sessions s JOIN local_accounts a ON a.user_id=s.user_id JOIN local_instance i ON i.workspace_id=a.workspace_id WHERE s.token_hash=$1 AND s.expires_at>clock_timestamp() AND s.credential_epoch=a.credential_epoch AND a.enabled")
            .bind(&hash[..]).fetch_optional(&self.pool).await?;
        row.map(
            |(
                user,
                workspace,
                username,
                is_master,
                enabled,
                must_change_password,
                credential_epoch,
                remaining,
            )| {
                let identity = AuthenticatedActor {
                    actor_id: ActorId::from_bytes(*user.as_bytes())
                        .map_err(|_| LocalAccountError::Unavailable)?,
                    workspace_id: WorkspaceId::from_bytes(*workspace.as_bytes())
                        .map_err(|_| LocalAccountError::Unavailable)?,
                };
                Ok(LocalSession {
                    identity,
                    account: LocalAccount {
                        user_id: user,
                        username,
                        is_master,
                        enabled,
                        must_change_password,
                    },
                    credential_epoch,
                    remaining_seconds: u64::try_from(remaining)
                        .map_err(|_| LocalAccountError::Unavailable)?,
                })
            },
        )
        .transpose()
    }
    pub async fn revoke_local_session(&self, hash: [u8; 32]) -> Result<(), LocalAccountError> {
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        sqlx::query("DELETE FROM account_sessions WHERE token_hash=$1")
            .bind(&hash[..])
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn change_local_password(
        &self,
        session_hash: [u8; 32],
        credential: &LocalCredential,
        phc: Zeroizing<String>,
        replacement_hash: [u8; 32],
    ) -> Result<LocalAccount, LocalAccountError> {
        validate_phc(&phc)?;
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_sessions s JOIN local_accounts a ON a.user_id=s.user_id WHERE s.token_hash=$1 AND s.user_id=$2 AND s.expires_at>clock_timestamp() AND s.credential_epoch=a.credential_epoch AND a.credential_epoch=$3 AND a.password_phc=$4 AND a.enabled)")
            .bind(&session_hash[..]).bind(credential.account.user_id).bind(credential.credential_epoch).bind(credential.phc.as_deref().map(String::as_str)).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(LocalAccountError::Unauthorized);
        };
        let epoch:i64=sqlx::query_scalar("UPDATE local_accounts SET password_phc=$2,credential_epoch=credential_epoch+1,must_change_password=false,failed_attempts=0,failed_window_started_at=NULL,locked_until=NULL WHERE user_id=$1 AND credential_epoch<9223372036854775807 RETURNING credential_epoch")
            .bind(credential.account.user_id).bind(phc.as_str()).fetch_one(&mut *tx).await?;
        sqlx::query("DELETE FROM account_sessions WHERE user_id=$1")
            .bind(credential.account.user_id)
            .execute(&mut *tx)
            .await?;
        insert_session(&mut tx, credential.account.user_id, epoch, replacement_hash).await?;
        tx.commit().await?;
        let mut account = credential.account.clone();
        account.must_change_password = false;
        Ok(account)
    }
    pub async fn list_local_accounts(
        &self,
        session_hash: [u8; 32],
    ) -> Result<Vec<LocalAccount>, LocalAccountError> {
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        require_master(&mut tx, session_hash).await?;
        let accounts=sqlx::query_as("SELECT user_id,username,is_master,enabled,must_change_password FROM local_accounts ORDER BY is_master DESC,username LIMIT 1000").fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(accounts)
    }
    pub async fn create_local_account(
        &self,
        session_hash: [u8; 32],
        username: &str,
        phc: Zeroizing<String>,
    ) -> Result<LocalAccount, LocalAccountError> {
        if !valid_local_username(username) {
            return Err(LocalAccountError::InvalidInput);
        };
        validate_phc(&phc)?;
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        require_master(&mut tx, session_hash).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM local_accounts")
            .fetch_one(&mut *tx)
            .await?;
        if count >= 1000 {
            return Err(LocalAccountError::Capacity);
        };
        let workspace: Uuid =
            sqlx::query_scalar("SELECT workspace_id FROM local_instance WHERE singleton")
                .fetch_one(&mut *tx)
                .await?;
        let user = Uuid::now_v7();
        sqlx::query("INSERT INTO users(user_id) VALUES($1)")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
            .bind(workspace)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO local_accounts(user_id,workspace_id,username,password_phc) VALUES($1,$2,$3,$4)").bind(user).bind(workspace).bind(username).bind(phc.as_str()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(LocalAccount {
            user_id: user,
            username: username.into(),
            is_master: false,
            enabled: true,
            must_change_password: true,
        })
    }
    pub async fn update_local_account(
        &self,
        session_hash: [u8; 32],
        target: Uuid,
        enabled: Option<bool>,
        reset: Option<Zeroizing<String>>,
    ) -> Result<(), LocalAccountError> {
        if enabled.is_some() == reset.is_some() {
            return Err(LocalAccountError::InvalidInput);
        };
        if let Some(phc) = &reset {
            validate_phc(phc)?
        }
        let mut tx = self
            .begin_guarded()
            .await
            .map_err(|_| LocalAccountError::Unavailable)?;
        lock_accounts(&mut tx).await?;
        require_master(&mut tx, session_hash).await?;
        let master: Option<bool> =
            sqlx::query_scalar("SELECT is_master FROM local_accounts WHERE user_id=$1 FOR UPDATE")
                .bind(target)
                .fetch_optional(&mut *tx)
                .await?;
        match master {
            Some(false) => {}
            Some(true) => return Err(LocalAccountError::MasterProtected),
            None => return Err(LocalAccountError::InvalidInput),
        };
        if let Some(value) = enabled {
            sqlx::query("UPDATE local_accounts SET enabled=$2,credential_epoch=credential_epoch+1 WHERE user_id=$1 AND credential_epoch<9223372036854775807").bind(target).bind(value).execute(&mut *tx).await?;
        }
        if let Some(phc) = reset {
            sqlx::query("UPDATE local_accounts SET password_phc=$2,must_change_password=true,credential_epoch=credential_epoch+1,failed_attempts=0,failed_window_started_at=NULL,locked_until=NULL WHERE user_id=$1 AND credential_epoch<9223372036854775807").bind(target).bind(phc.as_str()).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM account_sessions WHERE user_id=$1")
            .bind(target)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_application::journal::OperationJournal;

    fn test_phc() -> Zeroizing<String> {
        Zeroizing::new("$argon2id$v=19$m=65536,t=3,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into())
    }

    #[test]
    fn local_identity_and_password_boundaries_are_explicit() {
        for username in ["master", "alice.smith", "a-1", "user_name"] {
            assert!(valid_local_username(username));
        }
        for username in ["ab", "Master", "a b", "юзер", "", &"a".repeat(33)] {
            assert!(!valid_local_username(username));
        }
        assert!(valid_local_password("  пароль 🔑  !"));
        assert!(!valid_local_password("short"));
        assert!(!valid_local_password(&"a".repeat(129)));
    }

    #[sqlx::test(migrations = false)]
    async fn local_accounts_bootstrap_race_acl_epochs_and_lockout(
        pool: sqlx::PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        assert!(db.local_setup_available().await?);
        let (first, second) = tokio::join!(
            db.bootstrap_local_instance("master", test_phc(), [1; 32]),
            db.bootstrap_local_instance("master2", test_phc(), [2; 32])
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let (master, master_hash) = match (first, second) {
            (Ok(account), Err(LocalAccountError::AlreadyInitialized)) => (account, [1; 32]),
            (Err(LocalAccountError::AlreadyInitialized), Ok(account)) => (account, [2; 32]),
            _ => return Err("bootstrap did not have exactly one atomic winner".into()),
        };
        assert!(!db.local_setup_available().await?);
        assert_eq!(db.list_local_accounts(master_hash).await?.len(), 1);
        let alice = db
            .create_local_account(master_hash, "alice", test_phc())
            .await?;
        assert!(!alice.is_master);
        assert!(alice.must_change_password);
        let bob = db
            .create_local_account(master_hash, "bob", test_phc())
            .await?;
        let credential = db.local_credential("alice").await?.ok_or("alice missing")?;
        assert!(
            db.complete_local_login(Some(&credential), true, [3; 32])
                .await?
                .is_some()
        );
        let temporary = db
            .local_session([3; 32])
            .await?
            .ok_or("temporary missing")?;
        assert!(temporary.account.must_change_password);
        assert!(matches!(
            db.create_local_account([3; 32], "intruder", test_phc())
                .await,
            Err(LocalAccountError::Unauthorized)
        ));
        db.change_local_password([3; 32], &credential, test_phc(), [4; 32])
            .await?;
        assert!(db.local_session([3; 32]).await?.is_none());
        let alice_session = db
            .local_session([4; 32])
            .await?
            .ok_or("alice session missing")?;
        assert!(!alice_session.account.must_change_password);
        let workspace = Uuid::from_bytes(*alice_session.identity.workspace_id.as_bytes());
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(document_id,workspace_id)VALUES($1,$2)")
            .bind(document)
            .bind(workspace)
            .execute(&db.pool)
            .await?;
        sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role)VALUES($1,$2,$3,'owner')").bind(workspace).bind(document).bind(alice.user_id).execute(&db.pool).await?;
        let master_session = db
            .local_session(master_hash)
            .await?
            .ok_or("master session missing")?;
        let document_id =
            spreadsheet_domain::workbook::DocumentId::from_bytes(*document.as_bytes())?;
        assert_eq!(
            db.document_role_name(master_session.identity, document_id)
                .await?,
            Some("owner".into())
        );
        assert_eq!(
            db.document_role_name(alice_session.identity, document_id)
                .await?,
            Some("owner".into())
        );
        let bob_identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes(*bob.user_id.as_bytes())?,
            workspace_id: alice_session.identity.workspace_id,
        };
        assert!(matches!(
            db.access(bob_identity, document_id).await,
            Err(spreadsheet_application::journal::JournalError::Unauthorized)
        ));
        assert!(matches!(
            db.change_permission(
                alice_session.identity,
                document_id,
                master_session.identity.actor_id,
                spreadsheet_domain::workbook::AuthorizationChangeId::from_bytes(
                    *Uuid::now_v7().as_bytes()
                )?,
                None
            )
            .await,
            Err(crate::PermissionError::OwnerImmutable)
        ));
        assert!(matches!(
            db.update_local_account(master_hash, master.user_id, Some(false), None)
                .await,
            Err(LocalAccountError::MasterProtected)
        ));
        assert!(matches!(
            db.update_local_account(master_hash, master.user_id, None, Some(test_phc()))
                .await,
            Err(LocalAccountError::MasterProtected)
        ));
        let other_workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(workspace_id)VALUES($1)")
            .bind(other_workspace)
            .execute(&db.pool)
            .await?;
        assert!(
            sqlx::query("INSERT INTO documents(document_id,workspace_id)VALUES($1,$2)")
                .bind(Uuid::now_v7())
                .bind(other_workspace)
                .execute(&db.pool)
                .await
                .is_err()
        );
        let verified_before_reset = db.local_credential("alice").await?.ok_or("alice missing")?;
        db.update_local_account(master_hash, alice.user_id, None, Some(test_phc()))
            .await?;
        assert!(db.local_session([4; 32]).await?.is_none());
        assert!(
            db.complete_local_login(Some(&verified_before_reset), true, [5; 32])
                .await?
                .is_none()
        );
        let reset = db.local_credential("alice").await?.ok_or("alice missing")?;
        assert!(
            db.complete_local_login(Some(&reset), true, [6; 32])
                .await?
                .is_some_and(|a| a.must_change_password)
        );
        db.update_local_account(master_hash, alice.user_id, Some(false), None)
            .await?;
        assert!(db.local_session([6; 32]).await?.is_none());
        assert!(
            db.local_credential("alice")
                .await?
                .ok_or("alice missing")?
                .phc
                .is_none()
        );
        db.update_local_account(master_hash, alice.user_id, Some(true), None)
            .await?;
        assert!(db.local_session([6; 32]).await?.is_none());
        for _ in 0..5 {
            let credential = db.local_credential("alice").await?.ok_or("alice missing")?;
            assert!(
                db.complete_local_login(Some(&credential), false, [7; 32])
                    .await?
                    .is_none()
            );
        }
        let reconnected = PgDatabase::for_test(db.pool.clone())?;
        let locked = reconnected
            .local_credential("alice")
            .await?
            .ok_or("alice missing")?;
        assert!(locked.phc.is_none());
        assert!(
            db.complete_local_login(Some(&locked), true, [8; 32])
                .await?
                .is_none()
        );
        assert!(db.local_session([8; 32]).await?.is_none());
        db.revoke_local_session(master_hash).await?;
        assert!(db.local_session(master_hash).await?.is_none());
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn local_bootstrap_refuses_existing_documents_without_adoption(
        pool: sqlx::PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let user = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let workspace = WorkspaceId::from_bytes(*Uuid::now_v7().as_bytes())?;
        db.issue_development_session([10; 32], user, workspace)
            .await?;
        sqlx::query("INSERT INTO documents(document_id,workspace_id)VALUES($1,$2)")
            .bind(Uuid::now_v7())
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .execute(&db.pool)
            .await?;
        assert!(!db.local_setup_available().await?);
        assert!(matches!(
            db.bootstrap_local_instance("master", test_phc(), [11; 32])
                .await,
            Err(LocalAccountError::ExistingDocuments)
        ));
        assert!(!db.local_instance_initialized().await?);
        let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM local_accounts")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(accounts, 0);
        Ok(())
    }
}
