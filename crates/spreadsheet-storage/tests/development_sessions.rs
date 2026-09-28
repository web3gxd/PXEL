use spreadsheet_domain::workbook::{ActorId, WorkspaceId};
use spreadsheet_storage::PgDatabase;
use uuid::Uuid;

#[tokio::test]
async fn revocation_is_token_scoped_idempotent_and_preserves_account_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let database = PgDatabase::connect(&std::env::var("DATABASE_URL")?, 2).await?;
    let first = *Uuid::now_v7().as_bytes();
    let second = *Uuid::now_v7().as_bytes();
    let mut hash_a = [0; 32];
    hash_a[..16].copy_from_slice(&first);
    let mut hash_b = [0; 32];
    hash_b[..16].copy_from_slice(&second);
    let account = Uuid::now_v7().simple().to_string();
    let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
    let workspace = WorkspaceId::from_bytes(*Uuid::now_v7().as_bytes())?;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let a = database
            .issue_development_account_session(hash_a, actor, workspace, &account)
            .await?;
        let b = database
            .issue_development_account_session(hash_b, actor, workspace, &account)
            .await?;
        if a.identity != b.identity {
            return Err("account identity changed".into());
        }
        database.revoke_development_session(hash_a).await?;
        if database.find_development_session(hash_a).await?.is_some() {
            return Err("session not revoked".into());
        }
        database.revoke_development_session(hash_a).await?;
        let remaining = database
            .find_development_session(hash_b)
            .await?
            .ok_or("unrelated session revoked")?;
        if remaining.identity != a.identity {
            return Err("remaining session identity changed".into());
        }
        Ok(())
    }
    .await;
    // Targeted cleanup also runs when one issuance or assertion-with-error fails.
    let cleanup_a = database.revoke_development_session(hash_a).await;
    let cleanup_b = database.revoke_development_session(hash_b).await;
    result?;
    cleanup_a?;
    cleanup_b?;
    Ok(())
}
