use spreadsheet_application::keys::KeyProvider;
use sqlx::{
    ConnectOptions, PgPool,
    migrate::{Migrate, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, sync::Arc, time::Duration};
use thiserror::Error;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("invalid database adapter configuration")]
    InvalidConfiguration,
    #[error("database connection or query failed")]
    Query(#[from] sqlx::Error),
    #[error("database migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("database schema is not ready")]
    SchemaNotReady,
    #[error("storage key configuration is unavailable")]
    Keys(#[from] spreadsheet_application::keys::KeyProviderError),
    #[error("legacy plaintext storage requires an offline migration into a new cluster")]
    LegacyPlaintext,
}

#[derive(Clone)]
pub struct PgDatabase {
    pub(crate) pool: PgPool,
    pub(crate) crypto_pool: PgPool,
    pub(crate) keys: Arc<dyn KeyProvider>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessEpochChange {
    pub document: spreadsheet_domain::workbook::DocumentId,
    pub epoch: u64,
}

pub struct AccessEpochListener {
    listener: sqlx::postgres::PgListener,
}

impl AccessEpochListener {
    pub async fn recv(&mut self) -> Result<AccessEpochChange, DatabaseError> {
        let notification = self.listener.recv().await?;
        let Some((document, epoch)) = notification.payload().rsplit_once(':') else {
            return Err(DatabaseError::InvalidConfiguration);
        };
        let document =
            uuid::Uuid::parse_str(document).map_err(|_| DatabaseError::InvalidConfiguration)?;
        let document = spreadsheet_domain::workbook::DocumentId::from_bytes(*document.as_bytes())
            .map_err(|_| DatabaseError::InvalidConfiguration)?;
        let epoch = epoch
            .parse::<u64>()
            .map_err(|_| DatabaseError::InvalidConfiguration)?;
        Ok(AccessEpochChange { document, epoch })
    }
}

pub(crate) fn bounded_options(options: PgConnectOptions) -> PgConnectOptions {
    options.disable_statement_logging().options([
        ("statement_timeout", "5000ms"),
        ("lock_timeout", "2000ms"),
        ("idle_in_transaction_session_timeout", "10000ms"),
        ("TimeZone", "UTC"),
    ])
}

pub(crate) fn restored_options(options: PgConnectOptions) -> PgConnectOptions {
    bounded_options(options).options([
        ("session_preload_libraries", ""),
        ("local_preload_libraries", ""),
        ("log_statement", "none"),
        ("log_min_error_statement", "panic"),
        ("log_parameter_max_length_on_error", "0"),
    ])
}

impl PgDatabase {
    pub(crate) async fn with_shared_fence<T, E>(
        &self,
        work: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E>
    where
        E: From<sqlx::Error>,
    {
        let mut fence = self.begin_guarded().await?;
        tokio::pin!(work);
        let mut timer = tokio::time::interval(Duration::from_secs(2));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result = &mut work => break result,
                _ = timer.tick() => { sqlx::query("SELECT 1").execute(&mut *fence).await?; }
            }
        };
        match result {
            Ok(value) => {
                fence.commit().await?;
                Ok(value)
            }
            Err(error) => {
                fence.rollback().await?;
                Err(error)
            }
        }
    }
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, DatabaseError> {
        if max_connections == 0 {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let keys = Arc::new(crate::LocalKeyProvider::from_environment()?);
        Self::connect_with_keys(url, max_connections, keys).await
    }

    pub async fn connect_with_keys(
        url: &str,
        max_connections: u32,
        keys: Arc<dyn KeyProvider>,
    ) -> Result<Self, DatabaseError> {
        Self::connect_with_options(url, max_connections, keys, false).await
    }

    /// Unpublished physical recovery must not execute restored per-role libraries
    /// before the first SQL query validates its private cluster identity.
    pub async fn connect_for_restored_cluster(
        url: &str,
        max_connections: u32,
        keys: Arc<dyn KeyProvider>,
    ) -> Result<Self, DatabaseError> {
        Self::connect_with_options(url, max_connections, keys, true).await
    }

    async fn connect_with_options(
        url: &str,
        max_connections: u32,
        keys: Arc<dyn KeyProvider>,
        restoring: bool,
    ) -> Result<Self, DatabaseError> {
        if max_connections == 0 {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let parsed = PgConnectOptions::from_str(url)?;
        let options = if restoring {
            restored_options(parsed)
        } else {
            bounded_options(parsed)
        };
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with(options.clone())
            .await?;
        let crypto_pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with(options)
            .await?;
        Ok(Self {
            pool,
            crypto_pool,
            keys,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(pool: PgPool) -> Result<Self, DatabaseError> {
        Ok(Self {
            crypto_pool: pool.clone(),
            pool,
            keys: Arc::new(crate::LocalKeyProvider::for_test()?),
        })
    }
    pub(crate) async fn begin_guarded(
        &self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        self.guard_transaction(&mut tx).await?;
        Ok(tx)
    }

    /// Enter the write fence after the caller has acquired its delivery gate.
    /// Share admission uses an existing, otherwise read-only per-link transaction.
    pub(crate) async fn guard_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_xact_lock_shared(1347962188)")
            .execute(&mut **tx)
            .await?;
        // Independent crypto commits can outlast the generic idle timeout.
        // Cancellation rolls back this fence; queries and pool acquisition
        // retain their individual bounds.
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0")
            .execute(&mut **tx)
            .await?;
        self.check_write_lineage().await.map_err(|_| {
            sqlx::Error::Protocol("storage write generations require activation".to_owned())
        })?;
        Ok(())
    }

    pub async fn migrate(&self) -> Result<(), DatabaseError> {
        // Refuse before changing legacy schemas. The migration CLI must target
        // a separate, empty cluster; runtime has no plaintext decoding fallback.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT to_regclass('public.document_operations')::text")
                .fetch_one(&self.pool)
                .await?;
        if existing.is_some() {
            let legacy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_operations WHERE octet_length(command_fingerprint)=32 OR octet_length(canonical_event_hash)=32)")
                .fetch_one(&self.pool).await?;
            if legacy {
                return Err(DatabaseError::LegacyPlaintext);
            }
        }
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn readiness(&self) -> Result<(), DatabaseError> {
        // Checked against PostgreSQL at build/prepare time. No document data is read.
        let result = sqlx::query!(
            r#"SELECT to_regclass('public.documents') IS NOT NULL
                AND to_regclass('public.memberships') IS NOT NULL
                AND to_regclass('public.document_permissions') IS NOT NULL
                AND to_regclass('public._sqlx_migrations') IS NOT NULL
                AND current_setting('fsync') = 'on'
                AND current_setting('full_page_writes') = 'on'
                AND current_setting('synchronous_commit') = 'on' AS "ready!""#
        )
        .fetch_one(&self.pool)
        .await?;
        if !result.ready {
            return Err(DatabaseError::SchemaNotReady);
        }
        let mut connection = self.pool.acquire().await?;
        if connection
            .dirty_version("public._sqlx_migrations")
            .await?
            .is_some()
        {
            return Err(DatabaseError::SchemaNotReady);
        }
        let applied = connection
            .list_applied_migrations("public._sqlx_migrations")
            .await?;
        if applied.len() != MIGRATOR.iter().count()
            || !MIGRATOR
                .iter()
                .zip(applied.iter())
                .all(|(expected, actual)| {
                    expected.version == actual.version && expected.checksum == actual.checksum
                })
        {
            return Err(DatabaseError::SchemaNotReady);
        }
        Ok(())
    }

    /// Subscribe to committed document access-epoch changes for cross-process
    /// delivery invalidation. Notification payloads contain only document IDs
    /// and monotone epochs; the trigger sends them only after transaction commit.
    pub async fn listen_access_epochs(&self) -> Result<AccessEpochListener, DatabaseError> {
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool).await?;
        listener.listen("pxel_document_access_epoch").await?;
        Ok(AccessEpochListener { listener })
    }

    pub async fn key_rotation_timestamp(&self) -> Result<Option<i64>, DatabaseError> {
        let timestamp = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT floor(extract(epoch FROM rotated_at))::bigint FROM crypto_key_state WHERE singleton=true",
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(timestamp)
    }

    pub async fn close(&self) {
        self.pool.close().await;
        self.crypto_pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseError, PgDatabase};
    use sqlx::PgPool;

    #[tokio::test]
    async fn empty_connection_pool_is_a_typed_configuration_error() {
        assert!(matches!(
            PgDatabase::connect("", 0).await,
            Err(DatabaseError::InvalidConfiguration)
        ));
    }

    #[sqlx::test(migrations = false)]
    async fn readiness_requires_the_exact_successful_migration_set(
        pool: PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let database = PgDatabase::for_test(pool)?;
        assert!(matches!(
            database.readiness().await,
            Err(DatabaseError::SchemaNotReady)
        ));
        database.migrate().await?;
        database.migrate().await?;
        database.readiness().await?;
        // Deliberate corruption fixture in SQLx's isolated test database.
        sqlx::raw_sql("UPDATE public._sqlx_migrations SET checksum = decode('00', 'hex')")
            .execute(&database.pool)
            .await?;
        assert!(matches!(
            database.readiness().await,
            Err(DatabaseError::SchemaNotReady)
        ));
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn readiness_rejects_disabled_commit_durability(
        pool: PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await?;
        let database = PgDatabase::for_test(pool)?;
        database.migrate().await?;
        database.readiness().await?;
        sqlx::raw_sql("SET synchronous_commit = off")
            .execute(&database.pool)
            .await?;
        assert!(matches!(
            database.readiness().await,
            Err(DatabaseError::SchemaNotReady)
        ));
        sqlx::raw_sql("SET synchronous_commit = on")
            .execute(&database.pool)
            .await?;
        database.readiness().await?;
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn foundation_rejects_cross_tenant_permission_and_invalid_role(
        pool: PgPool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let database = PgDatabase::for_test(pool)?;
        database.migrate().await?;
        sqlx::raw_sql(
            "INSERT INTO users(user_id) VALUES ('00000000-0000-7000-8000-000000000001');
             INSERT INTO workspaces(workspace_id) VALUES
                ('00000000-0000-7000-8000-000000000002'),
                ('00000000-0000-7000-8000-000000000003');
             INSERT INTO memberships(workspace_id, user_id, role) VALUES (
                '00000000-0000-7000-8000-000000000002',
                '00000000-0000-7000-8000-000000000001', 'owner');
             INSERT INTO documents(document_id, workspace_id) VALUES (
                '00000000-0000-7000-8000-000000000004',
                '00000000-0000-7000-8000-000000000003');",
        )
        .execute(&database.pool)
        .await?;
        let result = sqlx::raw_sql(
            "INSERT INTO document_permissions(workspace_id, document_id, user_id, role) VALUES (
                '00000000-0000-7000-8000-000000000003',
                '00000000-0000-7000-8000-000000000004',
                '00000000-0000-7000-8000-000000000001', 'editor')",
        )
        .execute(&database.pool)
        .await;
        assert!(
            matches!(result, Err(sqlx::Error::Database(ref error)) if error.is_foreign_key_violation())
        );
        let result = sqlx::raw_sql("UPDATE memberships SET role = 'administrator'")
            .execute(&database.pool)
            .await;
        assert!(
            matches!(result, Err(sqlx::Error::Database(ref error)) if error.is_check_violation())
        );
        Ok(())
    }
}
