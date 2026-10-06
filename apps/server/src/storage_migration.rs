//! Explicit offline migration; configuration publication stays with the operator.

use crate::{
    storage_maintenance::{
        Connection, ProtectedDirectory, connection_from_sources, contained_file,
        excluded_secret_path, pg_binary, pg_command,
    },
    storage_restore::{
        RecoveryLifecycle, RecoverySignals, checked_binary, launcher_path, new_private_root,
        stop_cluster, verify_physical_recovery,
    },
};
use spreadsheet_storage::{
    LegacyMigrationReport, LocalFilesystemBlobStore, LocalKeyProvider, PgDatabase,
    PgMaintenanceInfo,
};
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum MigrationError {
    #[error("usage: pxel-server storage migrate-legacy --maintenance-confirmed")]
    Arguments,
    #[error("legacy migration configuration is invalid or unavailable")]
    Configuration,
    #[error("legacy migration requires distinct local PostgreSQL clusters")]
    DistinctClusters,
    #[error("legacy migration requires private, disjoint paths and new destinations")]
    UnsafePaths,
    #[error("legacy source schema or cluster preflight failed")]
    Source,
    #[error("fresh destination schema or cluster preflight failed")]
    Destination,
    #[error("encrypted legacy rollback backup failed")]
    Backup,
    #[error("private legacy rollback recovery verification failed")]
    Recovery,
    #[error("legacy migration failed; destination remains unpublished")]
    Migration,
    #[error("destination restart verification failed; writes remain closed")]
    Restart,
    #[error("destination could not be stopped after failed verification; it remains unpublished")]
    StopFailed,
}

struct Configuration {
    source: Connection,
    target: Connection,
    recovery: Connection,
    source_blobs: PathBuf,
    target_blobs: PathBuf,
    archive: PathBuf,
    verify_root: PathBuf,
    keys: Arc<LocalKeyProvider>,
}

fn required_path(variable: &str) -> Result<PathBuf, MigrationError> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(MigrationError::Configuration)
}

fn loopback_endpoint(connection: &Connection) -> (&str, &str) {
    let host = if matches!(connection.host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        "localhost"
    } else {
        &connection.host
    };
    (host, &connection.port)
}

impl Configuration {
    fn load() -> Result<Self, MigrationError> {
        if std::env::var_os("PXEL_S3_ENDPOINT").is_some() {
            return Err(MigrationError::Configuration);
        }
        let source = connection_from_sources(
            "PXEL_LEGACY_SOURCE_DATABASE_URL",
            "PXEL_LEGACY_SOURCE_DATABASE_URL_FILE",
            ".local/legacy-source-url",
        )
        .map_err(|_| MigrationError::Configuration)?;
        let target = connection_from_sources(
            "DATABASE_URL",
            "PXEL_DATABASE_URL_FILE",
            ".local/database-url",
        )
        .map_err(|_| MigrationError::Configuration)?;
        if source.url.as_str() == target.url.as_str()
            || loopback_endpoint(&source) == loopback_endpoint(&target)
        {
            return Err(MigrationError::DistinctClusters);
        }
        if std::env::var_os("PXEL_RESTORE_DATABASE_URL").is_none()
            && std::env::var_os("PXEL_RESTORE_DATABASE_URL_FILE").is_none()
        {
            return Err(MigrationError::Configuration);
        }
        let recovery = connection_from_sources(
            "PXEL_RESTORE_DATABASE_URL",
            "PXEL_RESTORE_DATABASE_URL_FILE",
            ".local/restore-database-url",
        )
        .map_err(|_| MigrationError::Configuration)?;
        let port = recovery
            .port
            .parse::<u16>()
            .map_err(|_| MigrationError::Configuration)?;
        if port < 1024
            || recovery.port == source.port
            || recovery.port == target.port
            || !matches!(recovery.host.as_str(), "localhost" | "127.0.0.1")
            || recovery.sslmode == "require"
            || recovery.password.is_empty()
            || recovery.user.as_str() != source.user.as_str()
            || recovery.password.as_str() != source.password.as_str()
            || recovery.database.as_str() != source.database.as_str()
        {
            return Err(MigrationError::Configuration);
        }
        let source_blobs = required_path("PXEL_LEGACY_SOURCE_BLOB_ROOT")?;
        let target_blobs = required_path("PXEL_BLOB_ROOT")?;
        let archive = required_path("PXEL_MIGRATION_ROLLBACK_ARCHIVE")?;
        let verify_root = required_path("PXEL_MIGRATION_VERIFY_ROOT")?;
        let keys = Arc::new(
            LocalKeyProvider::from_environment().map_err(|_| MigrationError::Configuration)?,
        );
        Ok(Self {
            source,
            target,
            recovery,
            source_blobs,
            target_blobs,
            archive,
            verify_root,
            keys,
        })
    }
}

struct NewDestination {
    path: PathBuf,
    parent: ProtectedDirectory,
}

impl NewDestination {
    fn prepare(path: &Path) -> Result<Self, MigrationError> {
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(MigrationError::UnsafePaths);
        }
        let absolute = std::path::absolute(path).map_err(|_| MigrationError::UnsafePaths)?;
        let parent =
            ProtectedDirectory::acquire(absolute.parent().ok_or(MigrationError::UnsafePaths)?)
                .map_err(|_| MigrationError::UnsafePaths)?;
        let path = parent
            .path
            .join(absolute.file_name().ok_or(MigrationError::UnsafePaths)?);
        let destination = Self { path, parent };
        destination.require_absent()?;
        Ok(destination)
    }
    fn require_absent(&self) -> Result<(), MigrationError> {
        self.parent
            .check()
            .map_err(|_| MigrationError::UnsafePaths)?;
        match std::fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(MigrationError::UnsafePaths),
        }
    }
}

struct Paths {
    source_pg: ProtectedDirectory,
    target_pg: ProtectedDirectory,
    source_blobs: ProtectedDirectory,
    target_blobs: NewDestination,
    archive: NewDestination,
    verification: NewDestination,
    source_information: PgMaintenanceInfo,
    target_information: PgMaintenanceInfo,
    _secret_files: Vec<std::fs::File>,
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn supported_cluster(information: &PgMaintenanceInfo) -> bool {
    information.server_version_num == 180006
        && !information.in_recovery
        && information.durable_settings
        && information.tablespace_names == ["pg_default", "pg_global"]
}

fn cluster_files(information: &PgMaintenanceInfo, pgdata: &Path) -> Result<(), MigrationError> {
    for file in [
        &information.config_file,
        &information.hba_file,
        &information.ident_file,
    ] {
        contained_file(Path::new(file), pgdata).map_err(|_| MigrationError::UnsafePaths)?;
    }
    for file in &information.configuration_files {
        contained_file(Path::new(file), pgdata).map_err(|_| MigrationError::UnsafePaths)?;
    }
    // pg_ctl internally launches postgres; reject its shell-sensitive paths.
    launcher_path(pgdata).map_err(|_| MigrationError::UnsafePaths)?;
    launcher_path(Path::new(&information.config_file)).map_err(|_| MigrationError::UnsafePaths)?;
    Ok(())
}

fn retain_secret_files(roots: &[&Path]) -> Result<Vec<std::fs::File>, MigrationError> {
    let mut files = Vec::new();
    for (variable, default) in [
        ("PXEL_KEYRING_FILE", None),
        (
            "PXEL_LEGACY_SOURCE_DATABASE_URL_FILE",
            Some(".local/legacy-source-url"),
        ),
        ("PXEL_DATABASE_URL_FILE", Some(".local/database-url")),
        (
            "PXEL_RESTORE_DATABASE_URL_FILE",
            Some(".local/restore-database-url"),
        ),
    ] {
        let configured = std::env::var_os(variable);
        let path = configured
            .map(PathBuf::from)
            .or_else(|| default.map(PathBuf::from));
        if let Some(path) = path {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let file = excluded_secret_path(&path, roots)
                        .map_err(|_| MigrationError::UnsafePaths)?;
                    files.push(file);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && variable != "PXEL_KEYRING_FILE" => {}
                Err(_) => return Err(MigrationError::UnsafePaths),
            }
        }
    }
    Ok(files)
}

impl Paths {
    fn prepare(
        source_blobs: PathBuf,
        target_blobs: PathBuf,
        archive: PathBuf,
        verification: PathBuf,
        source_information: PgMaintenanceInfo,
        target_information: PgMaintenanceInfo,
    ) -> Result<Self, MigrationError> {
        if !supported_cluster(&source_information) {
            return Err(MigrationError::Source);
        }
        if !supported_cluster(&target_information) || target_information.has_application_data {
            return Err(MigrationError::Destination);
        }
        if source_information.system_identifier == target_information.system_identifier {
            return Err(MigrationError::DistinctClusters);
        }
        let source_pg = ProtectedDirectory::acquire(Path::new(&source_information.data_directory))
            .map_err(|_| MigrationError::UnsafePaths)?;
        let target_pg = ProtectedDirectory::acquire(Path::new(&target_information.data_directory))
            .map_err(|_| MigrationError::UnsafePaths)?;
        let source_blobs =
            ProtectedDirectory::acquire(&source_blobs).map_err(|_| MigrationError::UnsafePaths)?;
        let target_blobs =
            NewDestination::prepare(&target_blobs).map_err(|_| MigrationError::UnsafePaths)?;
        let archive = NewDestination::prepare(&archive).map_err(|_| MigrationError::UnsafePaths)?;
        let verification =
            NewDestination::prepare(&verification).map_err(|_| MigrationError::UnsafePaths)?;
        let roots = [
            &source_pg.path,
            &target_pg.path,
            &source_blobs.path,
            &target_blobs.path,
            &archive.path,
            &verification.path,
        ];
        for (index, left) in roots.iter().enumerate() {
            for right in roots.iter().skip(index + 1) {
                if overlaps(left, right) {
                    return Err(MigrationError::UnsafePaths);
                }
            }
        }
        // Output parents must not be inside, or contain, physical source/data roots.
        for parent in [&archive.parent.path, &verification.parent.path] {
            for source in [&source_pg.path, &target_pg.path, &source_blobs.path] {
                if overlaps(parent, source) {
                    return Err(MigrationError::UnsafePaths);
                }
            }
        }
        cluster_files(&source_information, &source_pg.path)
            .map_err(|_| MigrationError::UnsafePaths)?;
        cluster_files(&target_information, &target_pg.path)
            .map_err(|_| MigrationError::UnsafePaths)?;
        let _secret_files = retain_secret_files(&[
            &source_pg.path,
            &target_pg.path,
            &source_blobs.path,
            &target_blobs.parent.path,
            &archive.parent.path,
            &verification.parent.path,
        ])
        .map_err(|_| MigrationError::UnsafePaths)?;
        let paths = Self {
            source_pg,
            target_pg,
            source_blobs,
            target_blobs,
            archive,
            verification,
            source_information,
            target_information,
            _secret_files,
        };
        paths.check()?;
        Ok(paths)
    }
    fn check(&self) -> Result<(), MigrationError> {
        for root in [
            &self.source_pg,
            &self.target_pg,
            &self.source_blobs,
            &self.target_blobs.parent,
            &self.archive.parent,
            &self.verification.parent,
        ] {
            root.check().map_err(|_| MigrationError::UnsafePaths)?;
        }
        Ok(())
    }
    fn check_information(
        &self,
        source: &PgMaintenanceInfo,
        target: &PgMaintenanceInfo,
    ) -> Result<(), MigrationError> {
        if !supported_cluster(source)
            || !supported_cluster(target)
            || source.system_identifier != self.source_information.system_identifier
            || target.system_identifier != self.target_information.system_identifier
            || crate::storage_maintenance::directory(Path::new(&source.data_directory), true)
                .map_err(|_| MigrationError::UnsafePaths)?
                != self.source_pg.path
            || crate::storage_maintenance::directory(Path::new(&target.data_directory), true)
                .map_err(|_| MigrationError::UnsafePaths)?
                != self.target_pg.path
            || source.config_file != self.source_information.config_file
            || source.hba_file != self.source_information.hba_file
            || source.ident_file != self.source_information.ident_file
            || source.configuration_files != self.source_information.configuration_files
            || target.config_file != self.target_information.config_file
            || target.hba_file != self.target_information.hba_file
            || target.ident_file != self.target_information.ident_file
            || target.configuration_files != self.target_information.configuration_files
        {
            return Err(MigrationError::Restart);
        }
        cluster_files(source, &self.source_pg.path)?;
        cluster_files(target, &self.target_pg.path)?;
        self.check()
    }
}

pub(super) async fn run(arguments: &[OsString]) -> Result<(), MigrationError> {
    match arguments {
        [command, action, confirmed]
            if command == "storage"
                && action == "migrate-legacy"
                && confirmed == "--maintenance-confirmed" => {}
        _ => return Err(MigrationError::Arguments),
    }
    Box::pin(migrate()).await
}

async fn migrate() -> Result<(), MigrationError> {
    let config = Configuration::load()?;
    // Check every required tool before applying even destination schema changes.
    let basebackup = pg_binary()
        .await
        .map_err(|_| MigrationError::Configuration)?;
    let bin = basebackup.parent().ok_or(MigrationError::Configuration)?;
    for name in ["pg_verifybackup", "pg_waldump", "pg_ctl", "postgres"] {
        checked_binary(bin, name)
            .await
            .map_err(|_| MigrationError::Configuration)?;
    }
    let (source_information, target_information) = tokio::try_join!(
        async {
            PgDatabase::inspect_legacy_source(&config.source.url)
                .await
                .map_err(|_| MigrationError::Source)
        },
        async {
            PgDatabase::inspect_legacy_destination(&config.target.url)
                .await
                .map_err(|_| MigrationError::Destination)
        },
    )?;
    let (source_blobs, target_blobs, archive, verification) = (
        config.source_blobs.clone(),
        config.target_blobs.clone(),
        config.archive.clone(),
        config.verify_root.clone(),
    );
    let paths = tokio::task::spawn_blocking(move || {
        Paths::prepare(
            source_blobs,
            target_blobs,
            archive,
            verification,
            source_information,
            target_information,
        )
    })
    .await
    .map_err(|_| MigrationError::UnsafePaths)??;
    let source_blobs = LocalFilesystemBlobStore::new(&paths.source_blobs.path, 32 * 1024 * 1024)
        .await
        .map_err(|_| MigrationError::Source)?;
    let database = PgDatabase::connect_with_keys(&config.target.url, 4, config.keys.clone())
        .await
        .map_err(|_| MigrationError::Destination)?;
    let prepared = async {
        database
            .migrate()
            .await
            .map_err(|_| MigrationError::Destination)?;
        database
            .readiness()
            .await
            .map_err(|_| MigrationError::Destination)?;
        database
            .encryption_readiness()
            .await
            .map_err(|_| MigrationError::Destination)?;
        paths.check()?;
        paths.archive.require_absent()?;
        crate::storage_maintenance::create_legacy_rollback(
            &paths.archive.path,
            &config.source,
            &paths.source_blobs.path,
            &database,
        )
        .await
        .map_err(|_| MigrationError::Backup)?;
        paths.verification.require_absent()?;
        Box::pin(verify_physical_recovery(
            &paths.archive.path,
            &paths.verification.path,
            &config.recovery,
        ))
        .await
        .map_err(|_| MigrationError::Recovery)?;
        paths.check()?;
        paths.target_blobs.require_absent()?;
        let private_blobs = new_private_root(&paths.target_blobs.path)
            .await
            .map_err(|_| MigrationError::UnsafePaths)?;
        let target_blobs = LocalFilesystemBlobStore::new(&private_blobs.path, 64 * 1024 * 1024)
            .await
            .map_err(|_| MigrationError::Destination)?;
        let report =
            Box::pin(database.migrate_legacy(&config.source.url, &source_blobs, &target_blobs))
                .await
                .map_err(|_| MigrationError::Migration)?;
        paths.check()?;
        private_blobs
            .check()
            .map_err(|_| MigrationError::UnsafePaths)?;
        Ok::<_, MigrationError>((report, private_blobs, target_blobs))
    }
    .await;
    database.close().await;
    let (report, private_blobs, target_blobs) = prepared?;
    Box::pin(restart_and_verify(
        &config,
        &paths,
        &source_blobs,
        &target_blobs,
        &private_blobs,
        &report,
    ))
    .await?;
    println!(
        "legacy migration verified after PostgreSQL restart: documents={}, journal_records={}, snapshots={}, export_audits={}, metadata_rows={}, missing_inactive_blobs={}; destination is read-only and configuration remains unpublished",
        report.documents,
        report.operation_records,
        report.snapshot_records,
        report.export_audit_records,
        report.metadata_rows,
        report.missing_inactive_blobs
    );
    Ok(())
}

async fn restart_and_verify(
    config: &Configuration,
    paths: &Paths,
    source_blobs: &LocalFilesystemBlobStore,
    target_blobs: &LocalFilesystemBlobStore,
    private_blobs: &ProtectedDirectory,
    report: &LegacyMigrationReport,
) -> Result<(), MigrationError> {
    let source = PgDatabase::inspect_legacy_source(&config.source.url)
        .await
        .map_err(|_| MigrationError::Source)?;
    let target = PgDatabase::inspect_storage_cluster(&config.target.url)
        .await
        .map_err(|_| MigrationError::Destination)?;
    paths.check_information(&source, &target)?;
    let basebackup = pg_binary()
        .await
        .map_err(|_| MigrationError::Configuration)?;
    let bin = basebackup.parent().ok_or(MigrationError::Configuration)?;
    let pg_ctl = checked_binary(bin, "pg_ctl")
        .await
        .map_err(|_| MigrationError::Configuration)?;
    checked_binary(bin, "postgres")
        .await
        .map_err(|_| MigrationError::Configuration)?;
    let pgdata = launcher_path(&paths.target_pg.path).map_err(|_| MigrationError::UnsafePaths)?;
    let config_file = launcher_path(Path::new(&paths.target_information.config_file))
        .map_err(|_| MigrationError::UnsafePaths)?;
    let port = config
        .target
        .port
        .parse::<u16>()
        .map_err(|_| MigrationError::Configuration)?;
    let options = format!(
        "-c config_file=\"{config_file}\" -c port={port} -c default_transaction_read_only=on"
    );
    let mut signals = RecoverySignals::install().map_err(|_| MigrationError::Configuration)?;
    let mut lifecycle = RecoveryLifecycle {
        pg_ctl: pg_ctl.clone(),
        pgdata,
        armed: true,
    };
    let result = {
        let operation = async {
            stop_cluster(&pg_ctl, &paths.target_pg.path)
                .await
                .map_err(|_| MigrationError::StopFailed)?;
            paths.check()?;
            let status = tokio::time::timeout(
                Duration::from_secs(70),
                pg_command(&pg_ctl)
                    .kill_on_drop(false)
                    .args(["start", "-D"])
                    .arg(&lifecycle.pgdata)
                    .args(["-w", "-t", "60", "-s", "-o"])
                    .arg(&options)
                    .stdout(Stdio::null())
                    .status(),
            )
            .await
            .map_err(|_| MigrationError::Restart)?
            .map_err(|_| MigrationError::Restart)?;
            if !status.success() {
                return Err(MigrationError::Restart);
            }
            let source = PgDatabase::inspect_legacy_source(&config.source.url)
                .await
                .map_err(|_| MigrationError::Restart)?;
            let target = PgDatabase::inspect_storage_cluster(&config.target.url)
                .await
                .map_err(|_| MigrationError::Restart)?;
            paths.check_information(&source, &target)?;
            let reconnected = PgDatabase::connect_for_restored_cluster(
                &config.target.url,
                4,
                config.keys.clone(),
            )
            .await
            .map_err(|_| MigrationError::Restart)?;
            let verified = Box::pin(reconnected.verify_legacy_after_restart(
                &config.source.url,
                source_blobs,
                target_blobs,
                report,
            ))
            .await
            .map_err(|_| MigrationError::Restart);
            reconnected.close().await;
            verified?;
            paths.check()?;
            private_blobs
                .check()
                .map_err(|_| MigrationError::UnsafePaths)?;
            Ok::<_, MigrationError>(())
        };
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            _=signals.wait()=>Err(MigrationError::Restart),
            result=&mut operation=>result,
        };
        result
    };
    if result.is_err() && lifecycle.armed {
        stop_cluster(&pg_ctl, &paths.target_pg.path)
            .await
            .map_err(|_| MigrationError::StopFailed)?;
    }
    lifecycle.armed = false;
    result
}
