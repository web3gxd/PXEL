//! Quiescent, encrypted physical-backup CLI. Archive verification never restores.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_storage::{LocalKeyProvider, PgDatabase, decrypt_backup};
use std::{
    ffi::OsString,
    io::SeekFrom,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    process::Command,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const BUNDLE_MAGIC: &[u8; 8] = b"PXELBND1";
const BUNDLE_END: &[u8; 8] = b"PXELEND1";
const CHUNK_BYTES: usize = 64 * 1024;
const MAX_BLOBS: usize = 4096;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_BLOB_BYTES: u64 = 48 + 32 * 1024 * 1024 + 56;
const MAX_PG_TAR_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_DATABASE_URL_BYTES: usize = 16 * 1024;
const MAX_WAL_SEGMENT_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error(
        "usage: pxel-server backup create <archive> --maintenance-confirmed | backup verify <archive>"
    )]
    Arguments,
    #[error("backup configuration is invalid or unavailable")]
    Configuration,
    #[error("backup requires PostgreSQL 18.6 and a local, self-contained cluster")]
    UnsupportedCluster,
    #[error("backup requires a PostgreSQL 18.6 server")]
    UnsupportedServerVersion,
    #[error("backup requires a cluster without additional tablespaces")]
    UnsupportedTablespaces,
    #[error("backup PostgreSQL configuration must remain inside its data directory")]
    UnsupportedClusterFiles,
    #[error("backup requires a loopback PostgreSQL endpoint")]
    UnsupportedConnectionHost,
    #[error("backup requires the pinned PostgreSQL 18.6 backup utilities")]
    UnsupportedPostgresUtilities,
    #[error(
        "backup requires a private local blob directory and keyring outside the backup sources"
    )]
    UnsafeSource,
    #[error("backup archive destination must be a new private file outside its sources")]
    UnsafeDestination,
    #[error("backup maintenance lock or database preflight failed")]
    Database,
    #[error("PostgreSQL backup process failed")]
    Postgres,
    #[error("PostgreSQL backup verification failed")]
    PostgresVerification,
    #[error("private PostgreSQL recovery could not be stopped; staging remains unpublished")]
    RecoveryStopFailed,
    #[error("private PostgreSQL recovery configuration failed at {0}")]
    RecoveryConfiguration(&'static str),
    #[error("encrypted backup stream failed")]
    Encryption,
    #[error("backup bundle is malformed, incomplete, or inconsistent")]
    InvalidBundle,
    #[error("PostgreSQL backup archive contains an unsupported or malformed tar entry")]
    InvalidPostgresTar,
    #[error("PostgreSQL backup archive contains an invalid tar header")]
    InvalidPostgresTarHeader,
    #[error(
        "PostgreSQL backup archive contains an invalid tar checksum ({expected} != {observed})"
    )]
    InvalidPostgresTarChecksum { expected: u64, observed: u64 },
    #[error("PostgreSQL backup archive contains invalid tar field text")]
    InvalidPostgresTarText,
    #[error("PostgreSQL backup archive contains invalid tar entry metadata")]
    InvalidPostgresTarMetadata,
    #[error("PostgreSQL backup archive uses an unsupported tar dialect")]
    UnsupportedPostgresTarDialect,
    #[error("PostgreSQL backup archive contains an unsupported tar entry type")]
    UnsupportedPostgresTarType,
    #[error("PostgreSQL backup archive contains an unsafe tar path")]
    UnsafePostgresTarPath,
    #[error("PostgreSQL backup archive is missing a complete tar header")]
    TruncatedPostgresTarHeader,
    #[error("PostgreSQL backup archive has invalid tar entry ordering")]
    InvalidPostgresTarOrdering,
    #[error("PostgreSQL backup archive has truncated tar file data")]
    TruncatedPostgresTarData,
    #[error("PostgreSQL backup archive has invalid tar padding")]
    InvalidPostgresTarPadding,
    #[error("PostgreSQL backup archive has invalid end blocks")]
    InvalidPostgresTarTerminator,
    #[error("backup file operation failed")]
    Io,
    #[error("WAL restore command arguments are malformed")]
    WalRestoreArguments,
    #[error("WAL restore requested an invalid segment name")]
    InvalidWalName,
    #[error("WAL restore could not open the private target for replacement")]
    WalDestinationOpen,
    #[error(
        "WAL restore requested '{requested}' but PostgreSQL target basename was '{destination}'"
    )]
    WalDestinationNameMismatch {
        requested: String,
        destination: String,
    },
    #[error("WAL restore destination is outside its private pg_wal directory")]
    WalDestinationPath,
    #[error("private recovery database connection failed")]
    RecoveryDatabaseConnection,
}

enum Action {
    Create(PathBuf),
    Verify(PathBuf),
    WalArchive {
        directory: PathBuf,
        source: PathBuf,
        name: String,
    },
}

fn parse(arguments: &[OsString]) -> Result<Action, MaintenanceError> {
    match arguments {
        [command, action, archive, confirmed]
            if command == "backup"
                && action == "create"
                && confirmed == "--maintenance-confirmed"
                && !archive.is_empty()
                && !archive.to_string_lossy().starts_with('-') =>
        {
            Ok(Action::Create(PathBuf::from(archive)))
        }
        [command, action, archive]
            if command == "backup"
                && action == "verify"
                && !archive.is_empty()
                && !archive.to_string_lossy().starts_with('-') =>
        {
            Ok(Action::Verify(PathBuf::from(archive)))
        }
        [command, action, directory, source, name]
            if command == "backup"
                && action == "wal-archive"
                && !directory.is_empty()
                && !source.is_empty()
                && !name.is_empty()
                && !directory.to_string_lossy().starts_with('-')
                && !source.to_string_lossy().starts_with('-') =>
        {
            let name = name.to_str().ok_or(MaintenanceError::Arguments)?;
            if !valid_wal_name(name) {
                return Err(MaintenanceError::Arguments);
            }
            Ok(Action::WalArchive {
                directory: PathBuf::from(directory),
                source: PathBuf::from(source),
                name: name.to_owned(),
            })
        }
        _ => Err(MaintenanceError::Arguments),
    }
}

/// Called before normal server configuration so malformed commands never open a DB.
pub async fn run(arguments: &[OsString]) -> Result<(), MaintenanceError> {
    match parse(arguments)? {
        Action::Create(archive) => Box::pin(create(&archive)).await,
        Action::Verify(archive) => {
            let keys = LocalKeyProvider::from_environment()
                .map_err(|_| MaintenanceError::Configuration)?;
            Box::pin(verify_archive(&keys, &archive)).await?;
            println!("encrypted backup bundle integrity verified");
            Ok(())
        }
        Action::WalArchive {
            directory,
            source,
            name,
        } => Box::pin(archive_wal(&directory, &source, &name)).await,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WalManifest {
    pub(super) format_version: u32,
    pub(super) wal_name: String,
    pub(super) byte_length: u64,
    pub(super) checksum: [u8; 32],
}

pub(super) fn valid_wal_name(name: &str) -> bool {
    if !name.is_ascii() {
        return false;
    }
    if let Some(backup) = name.strip_suffix(".backup") {
        let Some((segment, offset)) = backup.split_once('.') else {
            return false;
        };
        return (24..=32).contains(&segment.len())
            && offset.len() == 8
            && segment.bytes().all(|byte| byte.is_ascii_hexdigit())
            && offset.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let (stem, history) = name
        .strip_suffix(".history")
        .map_or((name, false), |stem| (stem, true));
    let valid_length = if history {
        (1..=8).contains(&stem.len())
    } else {
        (24..=32).contains(&stem.len())
    };
    valid_length && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn validate_wal_manifest(
    manifest: &WalManifest,
    expected: &str,
) -> Result<(), MaintenanceError> {
    if manifest.format_version != 1
        || manifest.wal_name != expected
        || !valid_wal_name(&manifest.wal_name)
        || manifest.byte_length == 0
        || manifest.byte_length > MAX_WAL_SEGMENT_BYTES
    {
        return Err(MaintenanceError::InvalidBundle);
    }
    Ok(())
}

async fn compare_wal_stream<R: AsyncRead + Unpin, S: AsyncRead + Unpin>(
    decrypted: &mut R,
    source: &mut S,
) -> Result<([u8; 32], u64), MaintenanceError> {
    let mut left = vec![0; CHUNK_BYTES];
    let mut right = vec![0; CHUNK_BYTES];
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    loop {
        let left_read = decrypted
            .read(&mut left)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let right_read = source
            .read(&mut right)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        if left_read != right_read || left[..left_read] != right[..right_read] {
            return Err(MaintenanceError::InvalidBundle);
        }
        if left_read == 0 {
            return Ok((hash.finalize().into(), size));
        }
        hash.update(&left[..left_read]);
        size = size
            .checked_add(left_read as u64)
            .ok_or(MaintenanceError::InvalidBundle)?;
    }
}

async fn verify_wal_file(
    keys: &LocalKeyProvider,
    archive: &Path,
    source: &mut File,
    name: &str,
) -> Result<(), MaintenanceError> {
    let mut input = File::from_std(stable_regular_file(archive).map_err(|_| MaintenanceError::Io)?);
    input
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    source
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    let (mut producer, mut consumer) = tokio::io::duplex(CHUNK_BYTES);
    let decrypt = async {
        let report = Box::pin(decrypt_backup(keys, &mut input, &mut producer))
            .await
            .map_err(|_| MaintenanceError::Encryption)?;
        producer
            .shutdown()
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let manifest: WalManifest = serde_json::from_slice(&report.manifest)
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        validate_wal_manifest(&manifest, name)?;
        if report.payload_bytes != manifest.byte_length {
            return Err(MaintenanceError::InvalidBundle);
        }
        Ok::<_, MaintenanceError>(manifest)
    };
    let compare = compare_wal_stream(&mut consumer, source);
    let (manifest, (checksum, byte_length)) = tokio::try_join!(decrypt, compare)?;
    if manifest.checksum != checksum || manifest.byte_length != byte_length {
        return Err(MaintenanceError::InvalidBundle);
    }
    Ok(())
}

fn wal_source_file(path: &Path) -> Result<(std::fs::File, same_file::Handle), MaintenanceError> {
    directory(path.parent().ok_or(MaintenanceError::UnsafeSource)?, false)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(MaintenanceError::UnsafeSource);
    }
    let before = same_file::Handle::from_path(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    // PostgreSQL may retain an open WAL handle while invoking archive_command.
    // Permit its ordinary shared access modes, then bind the read to the same
    // file identity and verify the complete encrypted round-trip before publish.
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    let file = options
        .open(path)
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    let identity = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|_| MaintenanceError::UnsafeSource)?,
    )
    .map_err(|_| MaintenanceError::UnsafeSource)?;
    require_identity(path, &identity)?;
    if identity != before {
        return Err(MaintenanceError::UnsafeSource);
    }
    Ok((file, identity))
}

async fn archive_wal(
    archive_directory: &Path,
    source_path: &Path,
    name: &str,
) -> Result<(), MaintenanceError> {
    if !valid_wal_name(name)
        || source_path.file_name().and_then(|value| value.to_str()) != Some(name)
    {
        return Err(MaintenanceError::UnsafeSource);
    }
    let archive_directory = ProtectedDirectory::acquire(archive_directory)
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    let source_parent = directory(
        source_path.parent().ok_or(MaintenanceError::UnsafeSource)?,
        false,
    )
    .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
    if source_parent.file_name().and_then(|value| value.to_str()) != Some("pg_wal")
        || source_parent.starts_with(&archive_directory.path)
        || archive_directory.path.starts_with(&source_parent)
    {
        return Err(MaintenanceError::UnsafeSource);
    }
    let (source_file, source_identity) = wal_source_file(source_path)?;
    let source_len = source_file
        .metadata()
        .map_err(|_| MaintenanceError::UnsafeSource)?
        .len();
    if source_len == 0 || source_len > MAX_WAL_SEGMENT_BYTES {
        return Err(MaintenanceError::InvalidBundle);
    }
    let mut source = File::from_std(source_file);
    let keys = std::sync::Arc::new(
        LocalKeyProvider::from_environment().map_err(|_| MaintenanceError::Configuration)?,
    );
    let destination = archive_directory.path.join(format!("{name}.pxel"));
    if std::fs::symlink_metadata(&destination).is_ok() {
        verify_wal_file(&keys, &destination, &mut source, name).await?;
        println!("encrypted WAL segment verified");
        return Ok(());
    }
    let connection = database_connection()?;
    let database = PgDatabase::connect_with_keys(&connection.url, 2, keys.clone())
        .await
        .map_err(|_| MaintenanceError::Database)?;
    database
        .encryption_readiness()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    let mut partial = PartialArchive {
        path: archive_directory
            .path
            .join(format!(".{name}.{}.partial", Uuid::new_v4())),
        owned: false,
        identity: None,
    };
    let operation = async {
        let mut output = private_archive_file(&mut partial).await?;
        let manifest = WalManifest {
            format_version: 1,
            wal_name: name.to_owned(),
            byte_length: source_len,
            checksum: {
                source
                    .seek(SeekFrom::Start(0))
                    .await
                    .map_err(|_| MaintenanceError::Io)?;
                let mut hash = Sha256::new();
                let mut buffer = vec![0; CHUNK_BYTES];
                loop {
                    let read = source
                        .read(&mut buffer)
                        .await
                        .map_err(|_| MaintenanceError::Io)?;
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                }
                hash.finalize().into()
            },
        };
        let manifest_bytes =
            serde_json::to_vec(&manifest).map_err(|_| MaintenanceError::InvalidBundle)?;
        source
            .seek(SeekFrom::Start(0))
            .await
            .map_err(|_| MaintenanceError::Io)?;
        database
            .encrypt_backup(&manifest_bytes, &mut source, &mut output)
            .await
            .map_err(|_| MaintenanceError::Encryption)?;
        output.flush().await.map_err(|_| MaintenanceError::Io)?;
        output.sync_all().await.map_err(|_| MaintenanceError::Io)?;
        output
            .seek(SeekFrom::Start(0))
            .await
            .map_err(|_| MaintenanceError::Io)?;
        drop(output);
        drop(partial.identity.take());
        let retained_partial =
            stable_regular_file(&partial.path).map_err(|_| MaintenanceError::UnsafeDestination)?;
        partial.identity = Some(
            same_file::Handle::from_file(
                retained_partial
                    .try_clone()
                    .map_err(|_| MaintenanceError::UnsafeDestination)?,
            )
            .map_err(|_| MaintenanceError::UnsafeDestination)?,
        );
        drop(retained_partial);
        require_identity(source_path, &source_identity)?;
        if std::fs::symlink_metadata(&destination).is_err() {
            verify_wal_file(&keys, &partial.path, &mut source, name)
                .await
                .map_err(|_| MaintenanceError::PostgresVerification)?;
        } else {
            verify_wal_file(&keys, &destination, &mut source, name).await?;
            return Ok(());
        }
        if manifest.format_version != 1
            || manifest.wal_name != name
            || manifest.byte_length != source_len
        {
            return Err(MaintenanceError::InvalidBundle);
        }
        archive_directory.check()?;
        if std::fs::symlink_metadata(&destination).is_ok() {
            verify_wal_file(&keys, &destination, &mut source, name).await?;
            return Ok(());
        }
        let identity = partial
            .identity
            .as_ref()
            .ok_or(MaintenanceError::UnsafeDestination)?;
        require_identity(&partial.path, identity)?;
        require_identity(source_path, &source_identity)?;
        #[cfg(not(windows))]
        tokio::fs::hard_link(&partial.path, &destination)
            .await
            .map_err(|_| MaintenanceError::UnsafeDestination)?;
        #[cfg(windows)]
        let mut published = PartialArchive {
            path: destination.clone(),
            owned: false,
            identity: None,
        };
        #[cfg(windows)]
        let published_file = {
            let mut encrypted = File::from_std(
                stable_regular_file(&partial.path).map_err(|_| MaintenanceError::UnsafeSource)?,
            );
            copy_ciphertext(&mut encrypted, &mut published).await?
        };
        #[cfg(windows)]
        {
            drop(published_file);
            drop(published.identity.take());
            let retained_published = stable_regular_file(&destination)
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            published.identity = Some(
                same_file::Handle::from_file(
                    retained_published
                        .try_clone()
                        .map_err(|_| MaintenanceError::UnsafeDestination)?,
                )
                .map_err(|_| MaintenanceError::UnsafeDestination)?,
            );
            drop(retained_published);
            verify_wal_file(&keys, &destination, &mut source, name)
                .await
                .map_err(|_| MaintenanceError::PostgresVerification)?;
        }
        #[cfg(not(windows))]
        require_identity(&destination, identity)?;
        #[cfg(windows)]
        require_identity(
            &destination,
            published
                .identity
                .as_ref()
                .ok_or(MaintenanceError::UnsafeDestination)?,
        )?;
        archive_directory.check()?;
        sync_parent(&archive_directory.path).await?;
        #[cfg(windows)]
        {
            published.owned = false;
        }
        drop(partial.identity.take());
        tokio::fs::remove_file(&partial.path)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        partial.owned = false;
        Ok(())
    }
    .await;
    database.close().await;
    operation?;
    println!("encrypted WAL segment archived");
    Ok(())
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BlobEntry {
    pub(super) key: String,
    pub(super) byte_length: u64,
    pub(super) checksum: [u8; 32],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleManifest {
    pub(super) format_version: u32,
    pub(super) postgres_version: String,
    pub(super) blobs: Vec<BlobEntry>,
}

fn validate_manifest(manifest: &BundleManifest) -> Result<(), MaintenanceError> {
    if manifest.format_version != 1
        || manifest.postgres_version != "18.6"
        || manifest.blobs.len() > MAX_BLOBS
    {
        return Err(MaintenanceError::InvalidBundle);
    }
    let mut previous: Option<&str> = None;
    for blob in &manifest.blobs {
        let id = Uuid::parse_str(&blob.key).map_err(|_| MaintenanceError::InvalidBundle)?;
        if id.is_nil()
            || id.to_string() != blob.key
            || previous.is_some_and(|key| key >= blob.key.as_str())
            || !(104..=MAX_BLOB_BYTES).contains(&blob.byte_length)
        {
            return Err(MaintenanceError::InvalidBundle);
        }
        previous = Some(&blob.key);
    }
    Ok(())
}

pub(super) async fn verify_archive(
    keys: &LocalKeyProvider,
    archive: &Path,
) -> Result<BundleManifest, MaintenanceError> {
    let stable = stable_regular_file(archive).map_err(|_| MaintenanceError::Io)?;
    let mut input = File::from_std(stable);
    verify_reader(keys, &mut input).await
}

/// The caller retains the same protected file through both passes and publication.
pub(super) async fn verify_reader<R: AsyncRead + Unpin>(
    keys: &LocalKeyProvider,
    input: &mut R,
) -> Result<BundleManifest, MaintenanceError> {
    let (mut producer, mut consumer) = tokio::io::duplex(CHUNK_BYTES);
    let decrypt = async {
        let report = Box::pin(decrypt_backup(keys, input, &mut producer))
            .await
            .map_err(|_| MaintenanceError::Encryption)?;
        producer
            .shutdown()
            .await
            .map_err(|_| MaintenanceError::Io)?;
        Ok::<_, MaintenanceError>(report)
    };
    let (report, observed) = tokio::try_join!(decrypt, inspect_bundle(&mut consumer))?;
    if report.manifest.len() > MAX_MANIFEST_BYTES {
        return Err(MaintenanceError::InvalidBundle);
    }
    let manifest: BundleManifest =
        serde_json::from_slice(&report.manifest).map_err(|_| MaintenanceError::InvalidBundle)?;
    validate_manifest(&manifest)?;
    if manifest.blobs != observed {
        return Err(MaintenanceError::InvalidBundle);
    }
    Ok(manifest)
}

async fn inspect_bundle<R: AsyncRead + Unpin>(
    input: &mut R,
) -> Result<Vec<BlobEntry>, MaintenanceError> {
    let mut magic = [0; 8];
    input
        .read_exact(&mut magic)
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    if &magic != BUNDLE_MAGIC {
        return Err(MaintenanceError::InvalidBundle);
    }
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut pg_hash = Sha256::new();
    let mut pg_size = 0_u64;
    loop {
        let length = input
            .read_u32()
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)? as usize;
        if length == 0 {
            break;
        }
        if length > CHUNK_BYTES {
            return Err(MaintenanceError::InvalidBundle);
        }
        pg_size = pg_size
            .checked_add(length as u64)
            .filter(|size| *size <= MAX_PG_TAR_BYTES)
            .ok_or(MaintenanceError::InvalidBundle)?;
        input
            .read_exact(&mut buffer[..length])
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        pg_hash.update(&buffer[..length]);
    }
    let expected_size = input
        .read_u64()
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    let mut expected_hash = [0; 32];
    input
        .read_exact(&mut expected_hash)
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    if pg_size == 0 || pg_size != expected_size || pg_hash.finalize().as_slice() != expected_hash {
        return Err(MaintenanceError::InvalidBundle);
    }
    let count = input
        .read_u32()
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)? as usize;
    if count > MAX_BLOBS {
        return Err(MaintenanceError::InvalidBundle);
    }
    let mut blobs = Vec::with_capacity(count);
    for _ in 0..count {
        let mut id = [0; 16];
        input
            .read_exact(&mut id)
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        let length = input
            .read_u64()
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        if !(104..=MAX_BLOB_BYTES).contains(&length) {
            return Err(MaintenanceError::InvalidBundle);
        }
        let mut remaining = length;
        let mut hash = Sha256::new();
        while remaining != 0 {
            let take = remaining.min(CHUNK_BYTES as u64) as usize;
            input
                .read_exact(&mut buffer[..take])
                .await
                .map_err(|_| MaintenanceError::InvalidBundle)?;
            hash.update(&buffer[..take]);
            remaining -= take as u64;
        }
        blobs.push(BlobEntry {
            key: Uuid::from_bytes(id).to_string(),
            byte_length: length,
            checksum: hash.finalize().into(),
        });
    }
    input
        .read_exact(&mut magic)
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    let mut trailing = [0; 1];
    if &magic != BUNDLE_END
        || input
            .read(&mut trailing)
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?
            != 0
    {
        return Err(MaintenanceError::InvalidBundle);
    }
    Ok(blobs)
}

pub(super) struct Connection {
    pub(super) url: Zeroizing<String>,
    pub(super) host: String,
    pub(super) port: String,
    pub(super) user: Zeroizing<String>,
    pub(super) password: Zeroizing<String>,
    pub(super) database: Zeroizing<String>,
    pub(super) sslmode: String,
}

struct SecretUrl(Option<url::Url>);

impl Drop for SecretUrl {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            drop(Zeroizing::new(String::from(value)));
        }
    }
}

fn decode_component(value: &str) -> Result<Zeroizing<String>, MaintenanceError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(value.len()));
    bytes.extend(percent_encoding::percent_decode_str(value));
    let decoded = std::str::from_utf8(&bytes).map_err(|_| MaintenanceError::Configuration)?;
    if decoded.contains('\0') {
        return Err(MaintenanceError::Configuration);
    }
    Ok(Zeroizing::new(decoded.to_owned()))
}

fn database_connection() -> Result<Connection, MaintenanceError> {
    connection_from_sources(
        "DATABASE_URL",
        "PXEL_DATABASE_URL_FILE",
        ".local/database-url",
    )
}

pub(super) fn connection_from_sources(
    url_variable: &str,
    file_variable: &str,
    default_file: &str,
) -> Result<Connection, MaintenanceError> {
    let url = match std::env::var_os(url_variable) {
        Some(value) => {
            let bytes = Zeroizing::new(value.into_encoded_bytes());
            guarded_url(&bytes)?
        }
        None => {
            let path = std::env::var_os(file_variable)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(default_file));
            let mut file =
                private_secret_file(&path).map_err(|_| MaintenanceError::Configuration)?;
            bounded_url_file(&mut file)?
        }
    };
    parse_connection(url)
}

fn guarded_url(bytes: &[u8]) -> Result<Zeroizing<String>, MaintenanceError> {
    if bytes.len() > MAX_DATABASE_URL_BYTES || bytes.contains(&0) {
        return Err(MaintenanceError::Configuration);
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|_| MaintenanceError::Configuration)?
        .trim();
    if value.is_empty() {
        return Err(MaintenanceError::Configuration);
    }
    Ok(Zeroizing::new(value.to_owned()))
}

fn bounded_url_file(file: &mut std::fs::File) -> Result<Zeroizing<String>, MaintenanceError> {
    let mut input = Zeroizing::new(vec![0; MAX_DATABASE_URL_BYTES + 1]);
    let mut used = 0;
    while used < input.len() {
        let read = std::io::Read::read(file, &mut input[used..])
            .map_err(|_| MaintenanceError::Configuration)?;
        if read == 0 {
            return guarded_url(&input[..used]);
        }
        used += read;
    }
    Err(MaintenanceError::Configuration)
}

fn parse_connection(url: Zeroizing<String>) -> Result<Connection, MaintenanceError> {
    let parsed = SecretUrl(Some(
        url::Url::parse(&url).map_err(|_| MaintenanceError::Configuration)?,
    ));
    let parsed = parsed.0.as_ref().ok_or(MaintenanceError::Configuration)?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql")
        || parsed.username().is_empty()
        || parsed.fragment().is_some()
    {
        return Err(MaintenanceError::Configuration);
    }
    let host = match parsed.host() {
        Some(url::Host::Domain("localhost")) => "localhost".to_owned(),
        Some(url::Host::Domain(address))
            if address
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback()) =>
        {
            address.to_owned()
        }
        Some(url::Host::Ipv4(address)) if address.is_loopback() => address.to_string(),
        Some(url::Host::Ipv6(address)) if address.is_loopback() => address.to_string(),
        _ => return Err(MaintenanceError::UnsupportedConnectionHost),
    };
    let mut sslmode = "prefer".to_owned();
    for pair in parsed
        .query()
        .into_iter()
        .flat_map(|query| query.split('&'))
    {
        let (name, value) = pair
            .split_once('=')
            .ok_or(MaintenanceError::Configuration)?;
        if name != "sslmode" || !matches!(value, "disable" | "allow" | "prefer" | "require") {
            return Err(MaintenanceError::Configuration);
        }
        sslmode = value.to_owned();
    }
    let user = decode_component(parsed.username())?;
    let password = decode_component(parsed.password().unwrap_or(""))?;
    let database = decode_component(parsed.path().trim_start_matches('/'))?;
    let port = parsed.port().unwrap_or(5432).to_string();
    Ok(Connection {
        url,
        host,
        port,
        user,
        password,
        database,
        sslmode,
    })
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(windows)]
fn windows_path_check(path: &Path, check: &str) -> Result<(), MaintenanceError> {
    use std::os::windows::process::CommandExt;
    let root = std::env::var_os("SystemRoot").ok_or(MaintenanceError::UnsafeSource)?;
    let executable = PathBuf::from(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = std::process::Command::new(executable)
        .args(["-NoProfile", "-NonInteractive", "-Command", check])
        .env_clear()
        .env("SystemRoot", root)
        .env("PXEL_BACKUP_CHECK_PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(MaintenanceError::UnsafeSource)
}

#[cfg(unix)]
fn current_uid() -> Result<u32, MaintenanceError> {
    // A trusted absolute binary avoids unsafe libc calls and environment UID claims.
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    if !output.status.success() {
        return Err(MaintenanceError::UnsafeSource);
    }
    std::str::from_utf8(&output.stdout)
        .map_err(|_| MaintenanceError::UnsafeSource)?
        .trim()
        .parse()
        .map_err(|_| MaintenanceError::UnsafeSource)
}

fn private_access(path: &Path, metadata: &std::fs::Metadata) -> Result<(), MaintenanceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != current_uid()? || metadata.mode() & 0o077 != 0 {
            return Err(MaintenanceError::UnsafeSource);
        }
        let _ = path;
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        windows_path_check(
            path,
            r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $allowed=@($sid.Value,'S-1-5-18','S-1-5-32-544');
          $acl=Get-Acl -LiteralPath $env:PXEL_BACKUP_CHECK_PATH;
          if ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 1 };
          foreach($rule in $acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
            if ($rule.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and $allowed -notcontains $rule.IdentityReference.Value) { exit 1 }
          }; exit 0
        } catch { exit 1 }"#,
        )?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, metadata);
        return Err(MaintenanceError::UnsafeSource);
    }
    Ok(())
}

pub(super) fn directory(path: &Path, private: bool) -> Result<PathBuf, MaintenanceError> {
    let absolute = std::path::absolute(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MaintenanceError::UnsafeSource);
        }
        current.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata =
            std::fs::symlink_metadata(&current).map_err(|_| MaintenanceError::UnsafeSource)?;
        if is_link(&metadata) || !metadata.is_dir() {
            return Err(MaintenanceError::UnsafeSource);
        }
    }
    if private {
        let metadata = std::fs::metadata(&absolute).map_err(|_| MaintenanceError::UnsafeSource)?;
        private_access(&absolute, &metadata)?;
    }
    std::fs::canonicalize(absolute).map_err(|_| MaintenanceError::UnsafeSource)
}

/// Retain every ancestor so Windows cannot rename any part of the checked path.
/// Unix ancestors must prevent another identity from replacing their children.
pub(super) struct ProtectedDirectory {
    pub(super) path: PathBuf,
    identities: Vec<(PathBuf, same_file::Handle)>,
}

impl ProtectedDirectory {
    pub(super) fn acquire(path: &Path) -> Result<Self, MaintenanceError> {
        let path = directory(path, true)?;
        let mut current = PathBuf::new();
        let mut identities = Vec::new();
        #[cfg(unix)]
        let uid = current_uid()?;
        for component in path.components() {
            current.push(component);
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            let metadata =
                std::fs::symlink_metadata(&current).map_err(|_| MaintenanceError::UnsafeSource)?;
            if is_link(&metadata) || !metadata.is_dir() {
                return Err(MaintenanceError::UnsafeSource);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if ![0, uid].contains(&metadata.uid())
                    || metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0
                {
                    return Err(MaintenanceError::UnsafeSource);
                }
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // Keep the directory itself from being renamed while allowing
                // PostgreSQL and the restore writer to update its contents.
                options.custom_flags(0x0200_0000).share_mode(1 | 2);
            }
            let file = options
                .open(&current)
                .map_err(|_| MaintenanceError::UnsafeSource)?;
            let identity =
                same_file::Handle::from_file(file).map_err(|_| MaintenanceError::UnsafeSource)?;
            require_identity(&current, &identity)?;
            identities.push((current.clone(), identity));
        }
        let protected = Self { path, identities };
        protected.check()?;
        Ok(protected)
    }

    pub(super) fn check(&self) -> Result<(), MaintenanceError> {
        for (path, identity) in &self.identities {
            require_identity(path, identity)?;
        }
        let metadata = std::fs::metadata(&self.path).map_err(|_| MaintenanceError::UnsafeSource)?;
        private_access(&self.path, &metadata)
    }
}

pub(super) fn stable_regular_file(path: &Path) -> Result<std::fs::File, MaintenanceError> {
    directory(path.parent().ok_or(MaintenanceError::UnsafeSource)?, false)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(MaintenanceError::UnsafeSource);
    }
    let before = same_file::Handle::from_path(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options
        .open(path)
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    let retained = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|_| MaintenanceError::UnsafeSource)?,
    )
    .map_err(|_| MaintenanceError::UnsafeSource)?;
    require_identity(path, &retained)?;
    if retained != before
        || !file
            .metadata()
            .map_err(|_| MaintenanceError::UnsafeSource)?
            .is_file()
    {
        return Err(MaintenanceError::UnsafeSource);
    }
    Ok(file)
}

pub(super) fn require_identity(
    path: &Path,
    retained: &same_file::Handle,
) -> Result<(), MaintenanceError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    if is_link(&metadata)
        || same_file::Handle::from_path(path).map_err(|_| MaintenanceError::UnsafeSource)?
            != *retained
    {
        return Err(MaintenanceError::UnsafeSource);
    }
    Ok(())
}

fn single_link(file: &std::fs::File, metadata: &std::fs::Metadata) -> Result<(), MaintenanceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(MaintenanceError::UnsafeSource);
        }
        let _ = file;
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        let information = winapi_util::file::information(winapi_util::HandleRef::from_file(file))
            .map_err(|_| MaintenanceError::UnsafeSource)?;
        if information.number_of_links() != 1 {
            return Err(MaintenanceError::UnsafeSource);
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, metadata);
        return Err(MaintenanceError::UnsafeSource);
    }
    Ok(())
}

pub(super) fn private_secret_file(path: &Path) -> Result<std::fs::File, MaintenanceError> {
    let file = stable_regular_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    private_access(path, &metadata)?;
    single_link(&file, &metadata)?;
    Ok(file)
}

pub(super) fn contained_file(path: &Path, root: &Path) -> Result<(), MaintenanceError> {
    directory(
        path.parent()
            .ok_or(MaintenanceError::UnsupportedClusterFiles)?,
        false,
    )
    .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
    let real =
        std::fs::canonicalize(path).map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
    if is_link(&metadata) || !metadata.is_file() || !real.starts_with(root) {
        return Err(MaintenanceError::UnsupportedClusterFiles);
    }
    Ok(())
}

fn exclude_secret_file(
    roots: &[&Path],
    variable: &str,
    default: Option<&str>,
) -> Result<Option<std::fs::File>, MaintenanceError> {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .or_else(|| default.map(PathBuf::from));
    if let Some(path) = path {
        match std::fs::canonicalize(&path) {
            Ok(_) => return excluded_secret_path(&path, roots).map(Some),
            Err(_) if variable == "PXEL_KEYRING_FILE" => {
                return Err(MaintenanceError::UnsafeSource);
            }
            Err(_) => {}
        }
    }
    Ok(None)
}

pub(super) fn excluded_secret_path(
    path: &Path,
    roots: &[&Path],
) -> Result<std::fs::File, MaintenanceError> {
    let real = std::fs::canonicalize(path).map_err(|_| MaintenanceError::UnsafeSource)?;
    for root in roots {
        let root = std::fs::canonicalize(root).map_err(|_| MaintenanceError::UnsafeSource)?;
        if real.starts_with(root) {
            return Err(MaintenanceError::UnsafeSource);
        }
    }
    private_secret_file(path)
}

async fn scan_blobs(root: &Path, encrypted: bool) -> Result<Vec<BlobEntry>, MaintenanceError> {
    let mut directory = tokio::fs::read_dir(root)
        .await
        .map_err(|_| MaintenanceError::UnsafeSource)?;
    let mut paths = Vec::new();
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|_| MaintenanceError::UnsafeSource)?
    {
        let name = entry.file_name();
        let name = name.to_str().ok_or(MaintenanceError::UnsafeSource)?;
        let metadata = tokio::fs::symlink_metadata(entry.path())
            .await
            .map_err(|_| MaintenanceError::UnsafeSource)?;
        if is_link(&metadata) || !metadata.is_file() {
            return Err(MaintenanceError::UnsafeSource);
        }
        // Incomplete filesystem puts are never live blob objects.
        if name.starts_with('.') {
            continue;
        }
        let id = Uuid::parse_str(name).map_err(|_| MaintenanceError::UnsafeSource)?;
        if id.is_nil() || id.to_string() != name || paths.len() >= MAX_BLOBS {
            return Err(MaintenanceError::UnsafeSource);
        }
        paths.push((name.to_owned(), entry.path(), metadata.len()));
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    let mut blobs = Vec::with_capacity(paths.len());
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    for (key, path, length) in paths {
        if !(104..=MAX_BLOB_BYTES).contains(&length) {
            return Err(MaintenanceError::UnsafeSource);
        }
        let mut file = File::open(path).await.map_err(|_| MaintenanceError::Io)?;
        let mut header = [0; 48];
        file.read_exact(&mut header)
            .await
            .map_err(|_| MaintenanceError::UnsafeSource)?;
        let payload_length = u64::from_le_bytes(
            header[8..16]
                .try_into()
                .map_err(|_| MaintenanceError::UnsafeSource)?,
        );
        if header[..8] != *b"PXELBLB\x01" || payload_length.checked_add(48) != Some(length) {
            return Err(MaintenanceError::UnsafeSource);
        }
        let mut physical_hash = Sha256::new();
        physical_hash.update(header);
        let mut prefix = [0; 16];
        file.read_exact(&mut prefix)
            .await
            .map_err(|_| MaintenanceError::UnsafeSource)?;
        if encrypted
            && (prefix[..12] != *b"PXELENC\0\0\x01\0\x01"
                || prefix[12..16] == [0; 4]
                || prefix[12] & 0x80 != 0)
        {
            return Err(MaintenanceError::UnsafeSource);
        }
        let mut payload_hash = Sha256::new();
        payload_hash.update(prefix);
        physical_hash.update(prefix);
        let mut read_bytes = 16_u64;
        loop {
            let read = file
                .read(&mut buffer[..])
                .await
                .map_err(|_| MaintenanceError::Io)?;
            if read == 0 {
                break;
            }
            read_bytes = read_bytes
                .checked_add(read as u64)
                .filter(|size| *size <= payload_length)
                .ok_or(MaintenanceError::UnsafeSource)?;
            physical_hash.update(&buffer[..read]);
            payload_hash.update(&buffer[..read]);
        }
        if read_bytes != payload_length || payload_hash.finalize().as_slice() != &header[16..48] {
            return Err(MaintenanceError::UnsafeSource);
        }
        blobs.push(BlobEntry {
            key,
            byte_length: length,
            checksum: physical_hash.finalize().into(),
        });
    }
    Ok(blobs)
}

#[cfg(windows)]
fn normal_windows_command_path(path: &Path) -> PathBuf {
    path.to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

pub(super) fn pg_command(binary: &Path) -> Command {
    #[cfg(windows)]
    let binary_path = normal_windows_command_path(binary);
    #[cfg(not(windows))]
    let binary_path = binary.to_path_buf();
    let mut command = Command::new(&binary_path);
    command
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // PostgreSQL utilities such as pg_verifybackup locate their companion
    // tools through PATH. Restrict lookup to the already validated, pinned bin
    // directory instead of inheriting the caller's environment.
    if let Some(bin) = binary_path.parent() {
        command.env("PATH", bin);
    }
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            command
                .env("SystemRoot", &root)
                .env("ComSpec", Path::new(&root).join("System32").join("cmd.exe"));
        }
        command.creation_flags(0x0800_0000);
    }
    command
}

#[cfg(test)]
#[cfg(windows)]
#[test]
fn postgres_utility_paths_drop_the_windows_extended_prefix() {
    assert_eq!(
        normal_windows_command_path(Path::new(r"\\?\C:\pg\bin\pg_ctl.exe")),
        PathBuf::from(r"C:\pg\bin\pg_ctl.exe")
    );
}

pub(super) async fn pg_binary() -> Result<PathBuf, MaintenanceError> {
    let bin = std::env::var_os("PXEL_PG_BIN").ok_or(MaintenanceError::Configuration)?;
    let bin = directory(Path::new(&bin), false)?;
    let binary = bin.join(if cfg!(windows) {
        "pg_basebackup.exe"
    } else {
        "pg_basebackup"
    });
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        pg_command(&binary).arg("--version").output(),
    )
    .await
    .map_err(|_| MaintenanceError::UnsupportedPostgresUtilities)?
    .map_err(|_| MaintenanceError::UnsupportedPostgresUtilities)?;
    if !output.status.success()
        || std::str::from_utf8(&output.stdout).ok().map(str::trim)
            != Some("pg_basebackup (PostgreSQL) 18.6")
    {
        return Err(MaintenanceError::UnsupportedPostgresUtilities);
    }
    Ok(binary)
}

#[cfg(any(windows, test))]
fn restore_windows_binary_stdout(
    input: &[u8],
    pending_carriage_return: &mut bool,
    output: &mut Vec<u8>,
) {
    output.clear();
    for byte in input.iter().copied() {
        if *pending_carriage_return {
            if byte == b'\n' {
                output.push(b'\n');
                *pending_carriage_return = false;
                continue;
            }
            output.push(b'\r');
            *pending_carriage_return = false;
        }
        if byte == b'\r' {
            *pending_carriage_return = true;
        } else {
            output.push(byte);
        }
    }
}

async fn write_pg_tar_chunk<W: AsyncWrite + Unpin>(
    output: &mut W,
    size: &mut u64,
    hash: &mut Sha256,
    bytes: &[u8],
) -> Result<(), MaintenanceError> {
    *size = size
        .checked_add(bytes.len() as u64)
        .filter(|size| *size <= MAX_PG_TAR_BYTES)
        .ok_or(MaintenanceError::InvalidBundle)?;
    hash.update(bytes);
    output
        .write_u32(u32::try_from(bytes.len()).map_err(|_| MaintenanceError::InvalidBundle)?)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    output
        .write_all(bytes)
        .await
        .map_err(|_| MaintenanceError::Io)
}

async fn produce_bundle<W: AsyncWrite + Unpin>(
    binary: &Path,
    connection: &Connection,
    blob_root: &Path,
    blobs: &[BlobEntry],
    output: &mut W,
) -> Result<(), MaintenanceError> {
    let mut child = pg_command(binary)
        .args([
            "-Ft",
            "-D",
            "-",
            "-X",
            "fetch",
            "--checkpoint=fast",
            "--manifest-checksums=SHA256",
            "--no-password",
        ])
        .env("PGHOST", &connection.host)
        .env("PGPORT", &connection.port)
        .env("PGUSER", connection.user.as_str())
        .env("PGPASSWORD", connection.password.as_str())
        .env("PGDATABASE", connection.database.as_str())
        .env("PGSSLMODE", &connection.sslmode)
        .env("PGCONNECT_TIMEOUT", "5")
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| MaintenanceError::Postgres)?;
    let mut source = child.stdout.take().ok_or(MaintenanceError::Postgres)?;
    output
        .write_all(BUNDLE_MAGIC)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    #[cfg(windows)]
    let mut normalized = Zeroizing::new(Vec::with_capacity(CHUNK_BYTES + 1));
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    #[cfg(windows)]
    let mut pending_carriage_return = false;
    loop {
        let read = source
            .read(&mut buffer[..])
            .await
            .map_err(|_| MaintenanceError::Postgres)?;
        if read == 0 {
            break;
        }
        #[cfg(windows)]
        let output_bytes = {
            restore_windows_binary_stdout(
                &buffer[..read],
                &mut pending_carriage_return,
                &mut normalized,
            );
            &normalized[..]
        };
        #[cfg(not(windows))]
        let output_bytes = &buffer[..read];
        if !output_bytes.is_empty() {
            write_pg_tar_chunk(output, &mut size, &mut hash, output_bytes).await?;
        }
    }
    #[cfg(windows)]
    if pending_carriage_return {
        write_pg_tar_chunk(output, &mut size, &mut hash, b"\r").await?;
    }
    if size == 0
        || !child
            .wait()
            .await
            .map_err(|_| MaintenanceError::Postgres)?
            .success()
    {
        return Err(MaintenanceError::Postgres);
    }
    output
        .write_u32(0)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    output
        .write_u64(size)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    output
        .write_all(&hash.finalize())
        .await
        .map_err(|_| MaintenanceError::Io)?;
    output
        .write_u32(u32::try_from(blobs.len()).map_err(|_| MaintenanceError::InvalidBundle)?)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    for blob in blobs {
        let id = Uuid::parse_str(&blob.key).map_err(|_| MaintenanceError::InvalidBundle)?;
        output
            .write_all(id.as_bytes())
            .await
            .map_err(|_| MaintenanceError::Io)?;
        output
            .write_u64(blob.byte_length)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let mut file = File::open(blob_root.join(&blob.key))
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let mut remaining = blob.byte_length;
        let mut hash = Sha256::new();
        while remaining != 0 {
            let take = remaining.min(CHUNK_BYTES as u64) as usize;
            file.read_exact(&mut buffer[..take])
                .await
                .map_err(|_| MaintenanceError::UnsafeSource)?;
            hash.update(&buffer[..take]);
            output
                .write_all(&buffer[..take])
                .await
                .map_err(|_| MaintenanceError::Io)?;
            remaining -= take as u64;
        }
        let mut extra = [0; 1];
        if file
            .read(&mut extra)
            .await
            .map_err(|_| MaintenanceError::Io)?
            != 0
            || hash.finalize().as_slice() != blob.checksum
        {
            return Err(MaintenanceError::UnsafeSource);
        }
    }
    output
        .write_all(BUNDLE_END)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    output.shutdown().await.map_err(|_| MaintenanceError::Io)?;
    Ok(())
}

#[cfg(windows)]
async fn copy_ciphertext(
    input: &mut File,
    destination: &mut PartialArchive,
) -> Result<File, MaintenanceError> {
    input
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    let mut output = private_archive_file(destination).await?;
    let mut buffer = vec![0; CHUNK_BYTES];
    loop {
        let read = input
            .read(&mut buffer)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|_| MaintenanceError::Io)?;
    }
    output.flush().await.map_err(|_| MaintenanceError::Io)?;
    output.sync_all().await.map_err(|_| MaintenanceError::Io)?;
    output
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    Ok(output)
}

struct PartialArchive {
    path: PathBuf,
    owned: bool,
    identity: Option<same_file::Handle>,
}

impl Drop for PartialArchive {
    fn drop(&mut self) {
        // This is the single create_new file owned by this attempt, never a tree.
        if self.owned
            && self
                .identity
                .as_ref()
                .is_some_and(|identity| require_identity(&self.path, identity).is_ok())
        {
            // Release the Windows deny-delete handle before unlinking our file.
            drop(self.identity.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn private_archive_file(partial: &mut PartialArchive) -> Result<File, MaintenanceError> {
    let path = &partial.path;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options
        .open(path)
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    partial.owned = true;
    partial.identity = Some(
        same_file::Handle::from_file(
            file.try_clone()
                .map_err(|_| MaintenanceError::UnsafeDestination)?,
        )
        .map_err(|_| MaintenanceError::UnsafeDestination)?,
    );
    #[cfg(windows)]
    {
        let script = r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $allowed=@($sid.Value,'S-1-5-18','S-1-5-32-544');
          $acl=[System.Security.AccessControl.FileSecurity]::new();
          $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
          foreach($value in $allowed) {
            $identity=[System.Security.Principal.SecurityIdentifier]::new($value);
            $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($identity,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);
            $acl.AddAccessRule($rule)
          };
          Set-Acl -LiteralPath $env:PXEL_BACKUP_ACL_PATH -AclObject $acl;
          $actual=Get-Acl -LiteralPath $env:PXEL_BACKUP_ACL_PATH;
          if ($actual.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 1 };
          foreach($rule in $actual.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
            if ($rule.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and $allowed -notcontains $rule.IdentityReference.Value) { exit 1 }
          };
          exit 0
        } catch { exit 1 }"#;
        let root = std::env::var_os("SystemRoot").ok_or(MaintenanceError::UnsafeDestination)?;
        let executable = PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            pg_command(&executable)
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("PXEL_BACKUP_ACL_PATH", path)
                .stdout(Stdio::null())
                .status(),
        )
        .await
        .map_err(|_| MaintenanceError::UnsafeDestination)?
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
        if !status.success() {
            return Err(MaintenanceError::UnsafeDestination);
        }
    }
    Ok(File::from_std(file))
}

pub(super) async fn sync_parent(parent: &Path) -> Result<(), MaintenanceError> {
    #[cfg(unix)]
    {
        let path = parent.to_owned();
        tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all())
            .await
            .map_err(|_| MaintenanceError::Io)?
            .map_err(|_| MaintenanceError::Io)?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

async fn create(archive: &Path) -> Result<(), MaintenanceError> {
    let connection = database_connection()?;
    let blob_root = std::env::var_os("PXEL_BLOB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".local/blobs"));
    create_bundle(archive, &connection, &blob_root, None).await
}

#[cfg(feature = "legacy-migration")]
pub(super) async fn create_legacy_rollback(
    archive: &Path,
    source: &Connection,
    source_blobs: &Path,
    nonce_database: &PgDatabase,
) -> Result<(), MaintenanceError> {
    create_bundle(archive, source, source_blobs, Some(nonce_database)).await
}

async fn create_bundle(
    archive: &Path,
    connection: &Connection,
    blob_root: &Path,
    legacy_nonce_database: Option<&PgDatabase>,
) -> Result<(), MaintenanceError> {
    if std::env::var_os("PXEL_S3_ENDPOINT").is_some() {
        return Err(MaintenanceError::UnsafeSource);
    }
    let binary = pg_binary().await?;
    let keys = std::sync::Arc::new(
        LocalKeyProvider::from_environment().map_err(|_| MaintenanceError::Configuration)?,
    );
    let blob_directory = ProtectedDirectory::acquire(blob_root)?;
    let blob_root = blob_directory.path.clone();
    let absolute = std::path::absolute(archive).map_err(|_| MaintenanceError::UnsafeDestination)?;
    let parent = ProtectedDirectory::acquire(
        absolute
            .parent()
            .ok_or(MaintenanceError::UnsafeDestination)?,
    )
    .map_err(|_| MaintenanceError::UnsafeDestination)?;
    let name = absolute
        .file_name()
        .ok_or(MaintenanceError::UnsafeDestination)?;
    let destination = parent.path.join(name);
    if destination.starts_with(&blob_root) || std::fs::symlink_metadata(&destination).is_ok() {
        return Err(MaintenanceError::UnsafeDestination);
    }
    let database = PgDatabase::connect_with_keys(&connection.url, 2, keys.clone())
        .await
        .map_err(|_| MaintenanceError::Database)?;
    if legacy_nonce_database.is_none() {
        database
            .readiness()
            .await
            .map_err(|_| MaintenanceError::Database)?;
        database
            .encryption_readiness()
            .await
            .map_err(|_| MaintenanceError::Database)?;
    }
    let guard = database
        .begin_maintenance()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    let nonce_guard = match legacy_nonce_database {
        Some(database) => Some(
            database
                .begin_maintenance()
                .await
                .map_err(|_| MaintenanceError::Database)?,
        ),
        None => None,
    };
    let information = guard
        .information()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    if information.server_version_num != 180006 {
        return Err(MaintenanceError::UnsupportedServerVersion);
    }
    if information.tablespace_names != ["pg_default", "pg_global"] {
        return Err(MaintenanceError::UnsupportedTablespaces);
    }
    let preflight = {
        let blob_path = blob_root.clone();
        let parent_path = parent.path.clone();
        let destination_path = destination.clone();
        tokio::task::spawn_blocking(move || {
            let pgdata = ProtectedDirectory::acquire(Path::new(&information.data_directory))
                .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
            directory(&pgdata.path.join("pg_wal"), false)
                .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
            for path in [
                &information.config_file,
                &information.hba_file,
                &information.ident_file,
            ] {
                contained_file(Path::new(path), &pgdata.path)
                    .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
            }
            for path in &information.configuration_files {
                contained_file(Path::new(path), &pgdata.path)
                    .map_err(|_| MaintenanceError::UnsupportedClusterFiles)?;
            }
            let roots = [&*pgdata.path, &*blob_path, &*parent_path];
            // Retain secret identities until publication and reject aliases even
            // when the configured paths are outside every backup directory.
            let keyring_file = exclude_secret_file(&roots, "PXEL_KEYRING_FILE", None)
                .map_err(|_| MaintenanceError::UnsafeSource)?;
            let url_file_default = (std::env::var_os("DATABASE_URL").is_none()
                && std::env::var_os("PXEL_DATABASE_URL_FILE").is_none())
            .then_some(".local/database-url");
            let url_file = exclude_secret_file(&roots, "PXEL_DATABASE_URL_FILE", url_file_default)
                .map_err(|_| MaintenanceError::Configuration)?;
            if destination_path.starts_with(&pgdata.path)
                || std::fs::symlink_metadata(&destination_path).is_ok()
            {
                return Err(MaintenanceError::UnsafeDestination);
            }
            let source_url_file =
                exclude_secret_file(&roots, "PXEL_LEGACY_SOURCE_DATABASE_URL_FILE", None)
                    .map_err(|_| MaintenanceError::Configuration)?;
            Ok::<_, MaintenanceError>((pgdata, keyring_file, url_file, source_url_file))
        })
    };
    let mut partial = PartialArchive {
        path: parent
            .path
            .join(format!(".pxel-backup-{}.partial", Uuid::new_v4())),
        owned: false,
        identity: None,
    };
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Keep the exclusive database fence live while physical and blob bytes are read,
    // and while the final encrypted archive is independently verified.
    {
        let pipeline = async {
            let (_pgdata, _keyring_file, _url_file, _source_url_file) = preflight
                .await
                .map_err(|_| MaintenanceError::UnsafeSource)??;
            let mut output = private_archive_file(&mut partial).await?;
            let blobs = scan_blobs(&blob_root, legacy_nonce_database.is_none()).await?;
            let manifest = BundleManifest {
                format_version: 1,
                postgres_version: "18.6".to_owned(),
                blobs,
            };
            validate_manifest(&manifest)?;
            let encoded = Zeroizing::new(
                serde_json::to_vec(&manifest).map_err(|_| MaintenanceError::InvalidBundle)?,
            );
            if encoded.len() > MAX_MANIFEST_BYTES {
                return Err(MaintenanceError::InvalidBundle);
            }
            let (mut producer, mut consumer) = tokio::io::duplex(CHUNK_BYTES);
            let encrypt = async {
                Box::pin(nonce_guard.as_ref().unwrap_or(&guard).encrypt_backup(
                    &encoded,
                    &mut consumer,
                    &mut output,
                ))
                .await
                .map_err(|_| MaintenanceError::Encryption)?;
                Ok::<_, MaintenanceError>(())
            };
            tokio::try_join!(
                produce_bundle(
                    &binary,
                    connection,
                    &blob_root,
                    &manifest.blobs,
                    &mut producer
                ),
                encrypt,
            )?;
            output.flush().await.map_err(|_| MaintenanceError::Io)?;
            output.sync_all().await.map_err(|_| MaintenanceError::Io)?;
            output
                .seek(SeekFrom::Start(0))
                .await
                .map_err(|_| MaintenanceError::Io)?;
            verify_reader(&keys, &mut output).await?;
            blob_directory.check()?;
            parent
                .check()
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            let identity = partial
                .identity
                .as_ref()
                .ok_or(MaintenanceError::UnsafeDestination)?;
            require_identity(&partial.path, identity)
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            #[cfg(not(windows))]
            tokio::fs::hard_link(&partial.path, &destination)
                .await
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            #[cfg(windows)]
            let mut published = PartialArchive {
                path: destination.clone(),
                owned: false,
                identity: None,
            };
            #[cfg(windows)]
            let mut published_file = copy_ciphertext(&mut output, &mut published).await?;
            #[cfg(windows)]
            {
                // NTFS cannot add a hardlink while our write/delete denial is live.
                // A new private ciphertext file retains that protection throughout
                // copying and verification. A reader can accept it only after the
                // authenticated final tag; the command reports completion afterward.
                verify_reader(&keys, &mut published_file).await?;
            }
            #[cfg(not(windows))]
            require_identity(&destination, identity)
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            #[cfg(windows)]
            require_identity(
                &destination,
                published
                    .identity
                    .as_ref()
                    .ok_or(MaintenanceError::UnsafeDestination)?,
            )
            .map_err(|_| MaintenanceError::UnsafeDestination)?;
            require_identity(&partial.path, identity)
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            parent
                .check()
                .map_err(|_| MaintenanceError::UnsafeDestination)?;
            #[cfg(windows)]
            {
                published.owned = false;
                drop(published_file);
            }
            drop(output);
            drop(partial.identity.take());
            tokio::fs::remove_file(&partial.path)
                .await
                .map_err(|_| MaintenanceError::Io)?;
            partial.owned = false;
            sync_parent(&parent.path).await?;
            Ok::<_, MaintenanceError>(())
        };
        tokio::pin!(pipeline);
        loop {
            tokio::select! {
                result = &mut pipeline => { result?; break; }
                _ = heartbeat.tick() => {
                    guard.keep_alive().await.map_err(|_| MaintenanceError::Database)?;
                    if let Some(guard) = &nonce_guard {
                        guard.keep_alive().await.map_err(|_| MaintenanceError::Database)?;
                    }
                }
            }
        }
    }
    guard
        .finish()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    if let Some(guard) = nonce_guard {
        guard
            .finish()
            .await
            .map_err(|_| MaintenanceError::Database)?;
    }
    database.close().await;
    println!("encrypted PostgreSQL and blob backup created and verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn fixture_bundle() -> Vec<u8> {
        let pg = b"synthetic PostgreSQL tar bytes";
        let mut body = BUNDLE_MAGIC.to_vec();
        body.extend_from_slice(&(pg.len() as u32).to_be_bytes());
        body.extend_from_slice(pg);
        body.extend_from_slice(&0_u32.to_be_bytes());
        body.extend_from_slice(&(pg.len() as u64).to_be_bytes());
        body.extend_from_slice(&Sha256::digest(pg));
        body.extend_from_slice(&0_u32.to_be_bytes());
        body.extend_from_slice(BUNDLE_END);
        body
    }

    #[test]
    fn wal_archives_accept_only_postgres_segment_and_history_names() {
        for name in [
            "000000010000000000000001",
            "00000001000000000000000100000001",
            "00000001.history",
            "000000010000000000000002.00000028.backup",
            "A.history",
        ] {
            assert!(valid_wal_name(name), "{name}");
        }
        for name in [
            "../000000010000000000000001",
            "000000010000000000000001/../../outside",
            "0000000g.history",
            "000000010000000000000002.0000002g.backup",
            ".history",
            "000000010000000000000001.pxel",
            "файл.history",
        ] {
            assert!(!valid_wal_name(name), "{name}");
        }
    }

    #[tokio::test]
    async fn bundle_parser_requires_complete_size_hash_marker_and_eof() {
        let body = fixture_bundle();
        assert!(
            matches!(inspect_bundle(&mut body.as_slice()).await, Ok(blobs) if blobs.is_empty())
        );
        for cut in [0, 8, body.len() - 1] {
            assert!(inspect_bundle(&mut &body[..cut]).await.is_err());
        }
        let mut appended = body.clone();
        appended.push(0);
        assert!(inspect_bundle(&mut appended.as_slice()).await.is_err());
        let mut wrong_hash = body.clone();
        wrong_hash[20] ^= 1;
        assert!(inspect_bundle(&mut wrong_hash.as_slice()).await.is_err());
        let mut oversized = BUNDLE_MAGIC.to_vec();
        oversized.extend_from_slice(&((CHUNK_BYTES + 1) as u32).to_be_bytes());
        assert!(inspect_bundle(&mut oversized.as_slice()).await.is_err());
    }

    #[test]
    fn manifest_rejects_unknown_version_noncanonical_ids_duplicates_and_limits() {
        let key = Uuid::new_v4().to_string();
        let mut manifest = BundleManifest {
            format_version: 1,
            postgres_version: "18.6".to_owned(),
            blobs: vec![BlobEntry {
                key: key.clone(),
                byte_length: 104,
                checksum: [0; 32],
            }],
        };
        assert!(validate_manifest(&manifest).is_ok());
        manifest.format_version = 2;
        assert!(validate_manifest(&manifest).is_err());
        manifest.format_version = 1;
        manifest.blobs[0].byte_length = MAX_BLOB_BYTES + 1;
        assert!(validate_manifest(&manifest).is_err());
        manifest.blobs[0].byte_length = 104;
        manifest.blobs[0].key = "../blob".to_owned();
        assert!(validate_manifest(&manifest).is_err());
        manifest.blobs[0].key = key.clone();
        manifest.blobs.push(BlobEntry {
            key,
            byte_length: 104,
            checksum: [0; 32],
        });
        assert!(validate_manifest(&manifest).is_err());
    }

    #[tokio::test]
    async fn partial_cleanup_never_deletes_a_file_it_did_not_create() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let path = temporary.path().join("existing.partial");
        std::fs::write(&path, b"preserved fixture").expect("fixture file");
        {
            let mut partial = PartialArchive {
                path: path.clone(),
                owned: false,
                identity: None,
            };
            assert!(private_archive_file(&mut partial).await.is_err());
        }
        assert_eq!(
            std::fs::read(path).expect("existing file preserved"),
            b"preserved fixture"
        );
    }

    #[tokio::test]
    async fn newly_created_partial_has_private_access_and_is_cleaned_on_failure() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let path = temporary.path().join("owned.partial");
        {
            let mut partial = PartialArchive {
                path: path.clone(),
                owned: false,
                identity: None,
            };
            let file = private_archive_file(&mut partial)
                .await
                .expect("private fixture file");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path)
                        .expect("fixture metadata")
                        .permissions()
                        .mode()
                        & 0o077,
                    0
                );
            }
            drop(file);
        }
        assert!(!path.exists());
    }

    #[test]
    fn percent_decoding_rejects_invalid_unicode_and_nul() {
        assert!(
            matches!(decode_component("owner%40local"), Ok(value) if value.as_str() == "owner@local")
        );
        assert!(decode_component("%FF").is_err());
        assert!(decode_component("%00").is_err());
    }

    #[test]
    fn database_urls_reject_empty_invalid_unicode_nul_and_oversized_input() {
        for bytes in [
            b"".as_slice(),
            b"  \n".as_slice(),
            b"postgres://owner\0@localhost/db".as_slice(),
            &[0xff],
        ] {
            assert!(guarded_url(bytes).is_err());
        }
        assert!(guarded_url(&vec![b'x'; MAX_DATABASE_URL_BYTES + 1]).is_err());
        let temporary = tempfile::tempdir().expect("fixture directory");
        let path = temporary.path().join("bounded-url.fixture");
        std::fs::write(&path, vec![b'x'; MAX_DATABASE_URL_BYTES + 4096])
            .expect("oversized fixture");
        let mut file = std::fs::File::open(&path).expect("fixture handle");
        assert!(bounded_url_file(&mut file).is_err());
        assert_eq!(
            std::io::Seek::stream_position(&mut file).expect("bounded position"),
            (MAX_DATABASE_URL_BYTES + 1) as u64
        );
        std::fs::write(&path, b"postgres://owner:p%40ss@localhost/db\n").expect("bounded fixture");
        let mut file = std::fs::File::open(&path).expect("fixture handle");
        let url = bounded_url_file(&mut file).expect("bounded URL");
        let connection = parse_connection(url).expect("loopback fixture URL");
        assert_eq!(connection.password.as_str(), "p@ss");
        let numeric_loopback = parse_connection(Zeroizing::new(
            "postgresql://owner:password@127.0.0.1:5432/database".to_owned(),
        ));
        assert!(numeric_loopback.is_ok());
    }

    #[test]
    fn windows_text_stdout_normalization_preserves_binary_bytes_across_chunks() {
        let original = b"first\nsecond\r\nthird\r\r\nfourth\r";
        let mut translated = Vec::new();
        for byte in original {
            if *byte == b'\n' {
                translated.push(b'\r');
            }
            translated.push(*byte);
        }

        let mut restored = Vec::new();
        let mut pending_carriage_return = false;
        for chunk in translated.chunks(7) {
            let mut buffer = Vec::with_capacity(chunk.len() + 1);
            restore_windows_binary_stdout(chunk, &mut pending_carriage_return, &mut buffer);
            restored.extend_from_slice(&buffer);
        }
        if pending_carriage_return {
            restored.push(b'\r');
        }

        assert_eq!(restored, original);
    }

    fn private_fixture_directory(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .expect("fixture permissions");
        }
        #[cfg(windows)]
        windows_path_check(path, r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $acl=[System.Security.AccessControl.DirectorySecurity]::new();
          $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
          foreach($value in @($sid.Value,'S-1-5-18','S-1-5-32-544')) {
            $identity=[System.Security.Principal.SecurityIdentifier]::new($value);
            $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($identity,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);
            $acl.AddAccessRule($rule)
          }; Set-Acl -LiteralPath $env:PXEL_BACKUP_CHECK_PATH -AclObject $acl; exit 0
        } catch { exit 1 }"#).expect("private fixture ACL");
    }

    #[test]
    fn destination_directory_rejects_access_by_another_identity() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        private_fixture_directory(temporary.path());
        assert!(ProtectedDirectory::acquire(temporary.path()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o777))
                .expect("public fixture permissions");
        }
        #[cfg(windows)]
        windows_path_check(temporary.path(), r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $acl=[System.Security.AccessControl.DirectorySecurity]::new();
          $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
          $own=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);
          $acl.AddAccessRule($own);
          $identity=[System.Security.Principal.SecurityIdentifier]::new('S-1-1-0');
          $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($identity,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);
          $acl.AddAccessRule($rule); [IO.Directory]::SetAccessControl($env:PXEL_BACKUP_CHECK_PATH,$acl); exit 0
        } catch { exit 1 }"#).expect("foreign access fixture ACL");
        assert!(directory(temporary.path(), true).is_err());
        assert!(ProtectedDirectory::acquire(temporary.path()).is_err());
    }

    #[tokio::test]
    async fn secret_file_hardlinks_and_backup_directory_containment_are_rejected() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        private_fixture_directory(temporary.path());
        let outside = temporary.path().join("outside");
        let backup = temporary.path().join("backup");
        std::fs::create_dir(&outside).expect("outside fixture directory");
        std::fs::create_dir(&backup).expect("backup fixture directory");
        private_fixture_directory(&outside);
        private_fixture_directory(&backup);
        let path = outside.join("secret.fixture");
        let mut partial = PartialArchive {
            path: path.clone(),
            owned: false,
            identity: None,
        };
        let mut file = private_archive_file(&mut partial)
            .await
            .expect("private synthetic secret");
        file.write_all(b"synthetic-secret-never-print")
            .await
            .expect("fixture bytes");
        file.flush().await.expect("fixture flush");
        drop(file);
        drop(partial.identity.take());
        partial.owned = false;
        assert!(excluded_secret_path(&path, &[&backup]).is_ok());
        assert!(excluded_secret_path(&path, &[&outside]).is_err());
        let alias = backup.join("secret-alias.fixture");
        std::fs::hard_link(&path, &alias).expect("hardlinked secret fixture");
        assert!(excluded_secret_path(&path, &[&backup]).is_err());
        assert!(private_secret_file(&path).is_err());
    }

    #[tokio::test]
    async fn retained_archive_can_publish_without_allowing_replacement() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        private_fixture_directory(temporary.path());
        let parent =
            ProtectedDirectory::acquire(temporary.path()).expect("protected fixture directory");
        let path = parent.path.join("verified.partial");
        let destination = parent.path.join("published.fixture");
        let mut partial = PartialArchive {
            path: path.clone(),
            owned: false,
            identity: None,
        };
        let mut file = private_archive_file(&mut partial)
            .await
            .expect("private fixture archive");
        file.write_all(b"verified encrypted fixture")
            .await
            .expect("fixture bytes");
        file.flush().await.expect("fixture flush");
        #[cfg(windows)]
        {
            assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
            assert!(std::fs::remove_file(&path).is_err());
            assert!(std::fs::rename(&parent.path, parent.path.with_extension("replaced")).is_err());
        }
        #[cfg(unix)]
        {
            let displaced = parent.path.join("displaced.fixture");
            std::fs::rename(&path, &displaced).expect("same-owner adversarial replacement");
            std::fs::write(&path, b"substituted fixture").expect("replacement fixture");
            assert!(
                require_identity(&path, partial.identity.as_ref().expect("retained identity"))
                    .is_err()
            );
            std::fs::remove_file(&path).expect("replacement cleanup");
            std::fs::rename(&displaced, &path).expect("original identity restored");
        }
        #[cfg(not(windows))]
        std::fs::hard_link(&path, &destination).expect("atomic publication");
        #[cfg(windows)]
        let mut published = PartialArchive {
            path: destination.clone(),
            owned: false,
            identity: None,
        };
        #[cfg(windows)]
        let published_file = copy_ciphertext(&mut file, &mut published)
            .await
            .expect("protected ciphertext publication");
        #[cfg(not(windows))]
        require_identity(
            &destination,
            partial.identity.as_ref().expect("retained identity"),
        )
        .expect("same verified published file");
        #[cfg(windows)]
        {
            require_identity(
                &destination,
                published.identity.as_ref().expect("publication identity"),
            )
            .expect("owned published file");
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&destination)
                    .is_err()
            );
            assert!(std::fs::remove_file(&destination).is_err());
            assert!(
                copy_ciphertext(
                    &mut file,
                    &mut PartialArchive {
                        path: destination.clone(),
                        owned: false,
                        identity: None
                    }
                )
                .await
                .is_err()
            );
            published.owned = false;
            drop(published_file);
            drop(published);
        }
        parent.check().expect("same protected directory");
        drop(file);
        drop(partial);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(&destination).expect("published fixture"),
            b"verified encrypted fixture"
        );
        drop(parent);
    }
}
