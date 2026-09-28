use sqlx::{
    ConnectOptions, PgPool,
    migrate::{Migrate, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};
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
}

#[derive(Clone)]
pub struct PgDatabase {
    pub(crate) pool: PgPool,
}

pub(crate) fn bounded_options(options: PgConnectOptions) -> PgConnectOptions {
    options.disable_statement_logging().options([
        ("statement_timeout", "5000ms"),
        ("lock_timeout", "2000ms"),
        ("idle_in_transaction_session_timeout", "10000ms"),
    ])
}

impl PgDatabase {
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, DatabaseError> {
        if max_connections == 0 {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let options = bounded_options(PgConnectOptions::from_str(url)?);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), DatabaseError> {
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

    pub async fn close(&self) {
        self.pool.close().await;
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
        let database = PgDatabase { pool };
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
        let database = PgDatabase { pool };
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
        let database = PgDatabase { pool };
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
