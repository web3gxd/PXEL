use crate::{CryptoError, PgDatabase};
use sqlx::{Postgres, Row, Transaction};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
};

/// Exclusive maintenance lifetime; its SQLx transaction remains private.
pub struct PgMaintenanceGuard<'a> {
    tx: Mutex<Transaction<'a, Postgres>>,
    database: PgDatabase,
}

pub struct PgMaintenanceInfo {
    pub system_identifier: String,
    pub has_application_data: bool,
    pub in_recovery: bool,
    pub durable_settings: bool,
    pub server_version_num: u32,
    pub configuration_files: Vec<String>,
    pub data_directory: String,
    pub config_file: String,
    pub hba_file: String,
    pub ident_file: String,
    pub tablespace_names: Vec<String>,
}

impl PgDatabase {
    /// Read-only preflight before a maintenance command may change any schema.
    pub async fn inspect_storage_cluster(url: &str) -> Result<PgMaintenanceInfo, CryptoError> {
        use std::str::FromStr;
        let options =
            crate::postgres::restored_options(sqlx::postgres::PgConnectOptions::from_str(url)?)
                .options([("default_transaction_read_only", "on")]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_with(options)
            .await?;
        let result = async {
            let mut connection = pool.acquire().await?;
            cluster_information(&mut connection).await
        }
        .await;
        pool.close().await;
        result
    }
    pub async fn begin_maintenance(&self) -> Result<PgMaintenanceGuard<'_>, CryptoError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(1347962188)")
            .fetch_one(&mut *tx)
            .await?;
        if !acquired {
            return Err(CryptoError::KeyConflict);
        }
        Ok(PgMaintenanceGuard {
            tx: Mutex::new(tx),
            database: self.clone(),
        })
    }
}

impl PgMaintenanceGuard<'_> {
    pub async fn keep_alive(&self) -> Result<(), CryptoError> {
        let mut tx = self.tx.lock().await;
        sqlx::query("SELECT 1").execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn information(&self) -> Result<PgMaintenanceInfo, CryptoError> {
        let mut tx = self.tx.lock().await;
        cluster_information(&mut tx).await
    }
    pub async fn finish(self) -> Result<(), CryptoError> {
        self.tx.into_inner().commit().await?;
        Ok(())
    }

    /// The unforgeable guard owns the exclusive fence for this exact database.
    /// Its independent crypto pool can reserve nonces while keep_alive runs.
    pub async fn encrypt_backup<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &self,
        manifest: &[u8],
        input: &mut R,
        output: &mut W,
    ) -> Result<crate::BackupReport, crate::BackupError> {
        self.keep_alive().await?;
        self.database
            .encrypt_backup_guarded(manifest, input, output)
            .await
    }
}

async fn cluster_information(
    tx: &mut sqlx::PgConnection,
) -> Result<PgMaintenanceInfo, CryptoError> {
    let row = sqlx::query("SELECT pg_is_in_recovery() AS in_recovery, current_setting('fsync')='on' AND current_setting('full_page_writes')='on' AND current_setting('synchronous_commit')='on' AS durable_settings,current_setting('server_version_num') AS server_version_num,current_setting('data_directory') AS data_directory,current_setting('config_file') AS config_file,current_setting('hba_file') AS hba_file,current_setting('ident_file') AS ident_file").fetch_one(&mut *tx).await?;
    let names =
        sqlx::query_scalar::<_, String>("SELECT spcname FROM pg_tablespace ORDER BY spcname")
            .fetch_all(&mut *tx)
            .await?;
    let configuration_files = sqlx::query_scalar::<_, String>("SELECT DISTINCT sourcefile FROM pg_file_settings WHERE sourcefile IS NOT NULL ORDER BY sourcefile").fetch_all(&mut *tx).await?;
    let system_identifier =
        sqlx::query_scalar::<_, String>("SELECT system_identifier::text FROM pg_control_system()")
            .fetch_one(&mut *tx)
            .await?;
    let mut has_application_data = false;
    for table in [
        "users",
        "workspaces",
        "memberships",
        "documents",
        "development_accounts",
        "development_sessions",
        "document_permissions",
        "document_operations",
        "audit_events",
        "document_snapshots",
        "export_audit_events",
        "document_crypto_keys",
        "crypto_key_state",
        "crypto_restore_runs",
        "storage_migration_state",
        "share_links",
        "share_guest_sessions",
        "share_audit_events",
        "share_owner_notifications",
        "document_comment_heads",
        "document_comment_events",
        "disclosure_policies",
        "disclosure_audit_events",
        "personal_document_views",
    ] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table}"))
            .fetch_one(&mut *tx)
            .await?;
        if exists
            && sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
                "SELECT EXISTS(SELECT 1 FROM {table})"
            )))
            .fetch_one(&mut *tx)
            .await?
        {
            has_application_data = true;
            break;
        }
    }
    Ok(PgMaintenanceInfo {
        system_identifier,
        has_application_data,
        in_recovery: row.try_get("in_recovery")?,
        durable_settings: row.try_get("durable_settings")?,
        configuration_files,
        server_version_num: row
            .try_get::<String, _>("server_version_num")?
            .parse()
            .map_err(|_| CryptoError::InvalidContext)?,
        data_directory: row.try_get("data_directory")?,
        config_file: row.try_get("config_file")?,
        hba_file: row.try_get("hba_file")?,
        ident_file: row.try_get("ident_file")?,
        tablespace_names: names,
    })
}
