//! Restore into an unpublished private directory, then verify real PG recovery.

use crate::storage_maintenance::{
    BundleManifest, Connection, MaintenanceError, ProtectedDirectory, WalManifest,
    connection_from_sources, directory, excluded_secret_path, pg_binary, pg_command,
    require_identity, stable_regular_file, sync_parent, valid_wal_name, validate_wal_manifest,
    verify_reader,
};
use same_file::Handle;
use sha2::{Digest, Sha256};
use spreadsheet_application::keys::KeyProvider;
use spreadsheet_storage::{LocalKeyProvider, PgDatabase, decrypt_backup};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::SeekFrom,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const CHUNK_BYTES: usize = 64 * 1024;
const MAX_AUTO_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_PG_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_BLOB_BYTES: u64 = 48 + 32 * 1024 * 1024 + 56;
const MAX_TAR_ENTRIES: usize = 1_000_000;
const MAX_TAR_PATH_BYTES: usize = 32 * 1024 * 1024;
const BUNDLE_MAGIC: &[u8; 8] = b"PXELBND1";
const BUNDLE_END: &[u8; 8] = b"PXELEND1";

struct WalRestorePartial {
    path: PathBuf,
    identity: Option<Handle>,
    owned: bool,
}

impl Drop for WalRestorePartial {
    fn drop(&mut self) {
        if self.owned
            && self
                .identity
                .as_ref()
                .is_some_and(|identity| require_identity(&self.path, identity).is_ok())
        {
            drop(self.identity.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn restore_wal_file(
    archive_directory: &Path,
    name: &str,
    destination: &Path,
) -> Result<(), MaintenanceError> {
    if !valid_wal_name(name) {
        return Err(MaintenanceError::InvalidWalName);
    }
    let destination_name = destination.file_name().and_then(|value| value.to_str());
    if !destination_name
        .is_some_and(|destination_name| wal_destination_matches(name, destination_name))
    {
        let destination = destination_name
            .filter(|value| value.len() <= 96 && value.bytes().all(|byte| byte.is_ascii_graphic()))
            .unwrap_or("<noncanonical>")
            .to_owned();
        return Err(MaintenanceError::WalDestinationNameMismatch {
            requested: name.to_owned(),
            destination,
        });
    }
    let archive_directory = ProtectedDirectory::acquire(archive_directory)?;
    let destination_parent = ProtectedDirectory::acquire(
        destination
            .parent()
            .ok_or(MaintenanceError::UnsafeDestination)?,
    )?;
    if destination_parent
        .path
        .file_name()
        .and_then(|value| value.to_str())
        != Some("pg_wal")
        || destination_parent.path.starts_with(&archive_directory.path)
        || archive_directory.path.starts_with(&destination_parent.path)
    {
        return Err(MaintenanceError::WalDestinationPath);
    }
    let archive = archive_directory.path.join(format!("{name}.pxel"));
    let mut input =
        File::from_std(stable_regular_file(&archive).map_err(|_| MaintenanceError::Io)?);
    let keys = LocalKeyProvider::from_environment().map_err(|_| MaintenanceError::Configuration)?;
    let mut partial = WalRestorePartial {
        path: destination_parent
            .path
            .join(format!(".{name}.{}.partial", Uuid::new_v4())),
        identity: None,
        owned: false,
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&partial.path)
        .map_err(|_| MaintenanceError::WalDestinationPath)?;
    partial.owned = true;
    partial.identity = Some(
        Handle::from_file(file.try_clone().map_err(|_| MaintenanceError::Io)?)
            .map_err(|_| MaintenanceError::Io)?,
    );
    let mut output = File::from_std(file);
    let report = Box::pin(decrypt_backup(&keys, &mut input, &mut output))
        .await
        .map_err(|_| MaintenanceError::Encryption)?;
    let manifest: WalManifest =
        serde_json::from_slice(&report.manifest).map_err(|_| MaintenanceError::InvalidBundle)?;
    validate_wal_manifest(&manifest, name)?;
    if report.payload_bytes != manifest.byte_length {
        return Err(MaintenanceError::InvalidBundle);
    }
    output.flush().await.map_err(|_| MaintenanceError::Io)?;
    output.sync_all().await.map_err(|_| MaintenanceError::Io)?;
    output
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = vec![0; CHUNK_BYTES];
    loop {
        let read = output
            .read(&mut buffer)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        if read == 0 {
            break;
        }
        count = count
            .checked_add(read as u64)
            .ok_or(MaintenanceError::InvalidBundle)?;
        hash.update(&buffer[..read]);
    }
    if count != manifest.byte_length || <[u8; 32]>::from(hash.finalize()) != manifest.checksum {
        return Err(MaintenanceError::InvalidBundle);
    }
    let temp_identity = partial
        .identity
        .as_ref()
        .ok_or(MaintenanceError::UnsafeDestination)?;
    require_identity(&partial.path, temp_identity)?;
    drop(output);
    if std::fs::symlink_metadata(destination).is_ok() {
        let mut existing = File::from_std(
            stable_regular_file(destination).map_err(|_| MaintenanceError::UnsafeDestination)?,
        );
        let existing_clone = existing
            .try_clone()
            .await
            .map_err(|_| MaintenanceError::UnsafeDestination)?
            .into_std()
            .await;
        let existing_identity =
            Handle::from_file(existing_clone).map_err(|_| MaintenanceError::UnsafeDestination)?;
        let mut decoded = File::open(&partial.path)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let mut left = vec![0; CHUNK_BYTES];
        let mut right = vec![0; CHUNK_BYTES];
        let mut identical = true;
        loop {
            let left_read = existing
                .read(&mut left)
                .await
                .map_err(|_| MaintenanceError::Io)?;
            let right_read = decoded
                .read(&mut right)
                .await
                .map_err(|_| MaintenanceError::Io)?;
            if left_read != right_read || left[..left_read] != right[..right_read] {
                identical = false;
                break;
            }
            if left_read == 0 {
                break;
            }
        }
        drop(existing);
        if identical {
            drop(partial.identity.take());
            tokio::fs::remove_file(&partial.path)
                .await
                .map_err(|_| MaintenanceError::Io)?;
            partial.owned = false;
            return Ok(());
        }
        require_identity(destination, &existing_identity)?;
        drop(existing_identity);
        let mut replacement = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(destination)
            .await
            .map_err(|_| MaintenanceError::WalDestinationOpen)?;
        let replacement_clone = replacement
            .try_clone()
            .await
            .map_err(|_| MaintenanceError::UnsafeDestination)?
            .into_std()
            .await;
        let replacement_identity = Handle::from_file(replacement_clone)
            .map_err(|_| MaintenanceError::UnsafeDestination)?;
        require_identity(destination, &replacement_identity)?;
        decoded
            .seek(SeekFrom::Start(0))
            .await
            .map_err(|_| MaintenanceError::Io)?;
        loop {
            let read = decoded
                .read(&mut right)
                .await
                .map_err(|_| MaintenanceError::Io)?;
            if read == 0 {
                break;
            }
            replacement
                .write_all(&right[..read])
                .await
                .map_err(|_| MaintenanceError::Io)?;
        }
        replacement
            .flush()
            .await
            .map_err(|_| MaintenanceError::Io)?;
        replacement
            .sync_all()
            .await
            .map_err(|_| MaintenanceError::Io)?;
        require_identity(destination, &replacement_identity)?;
        drop(replacement);
        drop(replacement_identity);
        drop(partial.identity.take());
        tokio::fs::remove_file(&partial.path)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        partial.owned = false;
        return Ok(());
    } else {
        tokio::fs::hard_link(&partial.path, destination)
            .await
            .map_err(|_| MaintenanceError::UnsafeDestination)?;
        require_identity(destination, temp_identity)?;
        sync_parent(&destination_parent.path).await?;
    }
    drop(partial.identity.take());
    tokio::fs::remove_file(&partial.path)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    partial.owned = false;
    Ok(())
}

fn wal_destination_matches(name: &str, destination_name: &str) -> bool {
    if destination_name == name {
        return true;
    }
    #[cfg(windows)]
    {
        if destination_name == "RECOVERYXLOG" {
            return !name.ends_with(".history") && !name.ends_with(".backup");
        }
        if destination_name == "RECOVERYHISTORY" {
            return name.ends_with(".history") || name.ends_with(".backup");
        }
    }
    false
}

pub(super) async fn run_wal_restore(arguments: &[OsString]) -> Result<(), MaintenanceError> {
    let [command, action, directory, name, destination] = arguments else {
        return Err(MaintenanceError::WalRestoreArguments);
    };
    if command != "backup" || action != "wal-restore" {
        return Err(MaintenanceError::WalRestoreArguments);
    }
    let name = name.to_str().ok_or(MaintenanceError::WalRestoreArguments)?;
    restore_wal_file(Path::new(directory), name, Path::new(destination)).await?;
    Ok(())
}

struct TarEntry {
    relative: PathBuf,
    size: u64,
    directory: bool,
}

fn octal(field: &[u8]) -> Result<u64, MaintenanceError> {
    let field = field.trim_ascii();
    let field = field.strip_suffix(&[0]).unwrap_or(field).trim_ascii();
    if field.is_empty() {
        return Ok(0);
    }
    field.iter().try_fold(0_u64, |number, byte| {
        if !(b'0'..=b'7').contains(byte) {
            return Err(MaintenanceError::InvalidBundle);
        }
        number
            .checked_mul(8)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or(MaintenanceError::InvalidBundle)
    })
}

fn tar_text(field: &[u8]) -> Result<&str, MaintenanceError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err(MaintenanceError::InvalidPostgresTarHeader);
    }
    std::str::from_utf8(&field[..end]).map_err(|_| MaintenanceError::InvalidPostgresTarHeader)
}

fn tar_path(value: &str, is_directory: bool) -> Result<PathBuf, MaintenanceError> {
    // PostgreSQL emits one USTAR entry for the archive root. The extraction
    // root already exists, so accepting this empty directory entry is safe.
    if is_directory && matches!(value, "." | "./") {
        return Ok(PathBuf::new());
    }
    // PostgreSQL itself prefixes the two synthetic pg_wal directories with ./.
    // Normalize only one leading component; embedded . and every .. stay invalid.
    let value = value.strip_prefix("./").unwrap_or(value);
    let value = if is_directory {
        value.strip_suffix('/').unwrap_or(value)
    } else {
        value
    };
    if value.is_empty()
        || value.len() > 4096
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte < 32 || byte == 127 || matches!(byte, b'\\' | b':'))
    {
        return Err(MaintenanceError::UnsafePostgresTarPath);
    }
    for component in value.split('/') {
        let upper = component.to_ascii_uppercase();
        let stem = upper.split('.').next().unwrap_or("");
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.len() > 255
            || component.ends_with(['.', ' '])
            || matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
            || stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')
        {
            return Err(MaintenanceError::UnsafePostgresTarPath);
        }
    }
    let control_path = value.to_ascii_lowercase();
    if control_path == "postmaster.pid" || control_path.starts_with("pg_tblspc/") {
        return Err(MaintenanceError::UnsafePostgresTarPath);
    }
    Ok(PathBuf::from(value))
}

fn tar_header(header: &[u8; 512]) -> Result<TarEntry, MaintenanceError> {
    let expected =
        octal(&header[148..156]).map_err(|_| MaintenanceError::InvalidPostgresTarHeader)?;
    let observed: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    if expected != observed {
        return Err(MaintenanceError::InvalidPostgresTarChecksum { expected, observed });
    }
    if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
        return Err(MaintenanceError::UnsupportedPostgresTarDialect);
    }
    if header[157..257].iter().any(|byte| *byte != 0) {
        return Err(MaintenanceError::UnsupportedPostgresTarType);
    }
    let directory = match header[156] {
        0 | b'0' => false,
        b'5' => true,
        _ => return Err(MaintenanceError::UnsupportedPostgresTarType),
    };
    let size =
        octal(&header[124..136]).map_err(|_| MaintenanceError::InvalidPostgresTarMetadata)?;
    if size > MAX_PG_BYTES || directory && size != 0 {
        return Err(MaintenanceError::InvalidPostgresTarMetadata);
    }
    let name = tar_text(&header[..100]).map_err(|_| MaintenanceError::InvalidPostgresTarText)?;
    let prefix =
        tar_text(&header[345..500]).map_err(|_| MaintenanceError::InvalidPostgresTarText)?;
    let path = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    };
    // PostgreSQL consumes this file to create tablespace symlinks during recovery.
    // A self-contained backup with only the two built-in tablespaces has no map.
    if path
        .strip_prefix("./")
        .unwrap_or(&path)
        .eq_ignore_ascii_case("tablespace_map")
        && size != 0
    {
        return Err(MaintenanceError::InvalidPostgresTarMetadata);
    }
    Ok(TarEntry {
        relative: tar_path(&path, directory)?,
        size,
        directory,
    })
}

async fn create_directory(path: &Path) -> Result<(), MaintenanceError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(path)
    })
    .await
    .map_err(|_| MaintenanceError::Io)?
    .map_err(|_| MaintenanceError::UnsafeDestination)
}

pub(super) async fn new_private_root(path: &Path) -> Result<ProtectedDirectory, MaintenanceError> {
    create_directory(path).await?;
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").ok_or(MaintenanceError::UnsafeDestination)?;
        let executable = PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let script = r#"$ErrorActionPreference='Stop'; try {
          $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;
          $acl=[System.Security.AccessControl.DirectorySecurity]::new();
          $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($sid);
          foreach($value in @($sid.Value,'S-1-5-18','S-1-5-32-544')) {
            $identity=[System.Security.Principal.SecurityIdentifier]::new($value);
            $rule=[System.Security.AccessControl.FileSystemAccessRule]::new($identity,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);
            $acl.AddAccessRule($rule)
          }; Set-Acl -LiteralPath $env:PXEL_RESTORE_ACL_PATH -AclObject $acl; exit 0
        } catch { exit 1 }"#;
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            pg_command(&executable)
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("PXEL_RESTORE_ACL_PATH", path)
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
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || ProtectedDirectory::acquire(&path))
        .await
        .map_err(|_| MaintenanceError::UnsafeDestination)?
}

async fn parents(root: &Path, relative: &Path) -> Result<(), MaintenanceError> {
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(MaintenanceError::UnsafeDestination);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_directory(&current).await?
            }
            _ => return Err(MaintenanceError::UnsafeDestination),
        }
    }
    Ok(())
}

async fn new_file(path: &Path) -> Result<File, MaintenanceError> {
    let path = path.to_owned();
    let file = tokio::task::spawn_blocking(move || {
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
        options.open(path)
    })
    .await
    .map_err(|_| MaintenanceError::Io)?
    .map_err(|_| MaintenanceError::UnsafeDestination)?;
    Ok(File::from_std(file))
}

struct CipherArchive {
    path: PathBuf,
    file: Option<File>,
    identity: Option<same_file::Handle>,
}

impl CipherArchive {
    fn reader(&mut self) -> Result<&mut File, MaintenanceError> {
        self.file.as_mut().ok_or(MaintenanceError::Io)
    }

    fn check(&self) -> Result<(), MaintenanceError> {
        require_identity(
            &self.path,
            self.identity.as_ref().ok_or(MaintenanceError::Io)?,
        )
    }
}

impl Drop for CipherArchive {
    fn drop(&mut self) {
        let owned = self.check().is_ok();
        // A retained Windows handle denies write/delete throughout both passes.
        drop(self.file.take());
        drop(self.identity.take());
        if owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn archive_copy_limit() -> Result<u64, MaintenanceError> {
    // PG framing may contain one-byte chunks. STREAM adds at most 21 bytes per
    // each of its u32-counted records, including header and authentication tag.
    MAX_PG_BYTES
        .checked_mul(5)
        .and_then(|value| {
            MAX_BLOB_BYTES
                .checked_mul(4096)
                .and_then(|blobs| value.checked_add(blobs))
        })
        .and_then(|value| {
            (u64::from(u32::MAX))
                .checked_mul(21)
                .and_then(|frames| value.checked_add(frames))
        })
        .and_then(|value| value.checked_add(2 * 1024 * 1024 + 4096 * 24 + 1024))
        .ok_or(MaintenanceError::InvalidBundle)
}

async fn copy_ciphertext<R: AsyncRead + Unpin>(
    original: &mut R,
    parent: &ProtectedDirectory,
) -> Result<CipherArchive, MaintenanceError> {
    parent
        .check()
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    let path = parent
        .path
        .join(format!(".pxel-restore-{}.cipher.partial", Uuid::new_v4()));
    let file = new_file(&path).await?;
    let retained = file.try_clone().await.map_err(|_| MaintenanceError::Io)?;
    let identity = same_file::Handle::from_file(retained.into_std().await)
        .map_err(|_| MaintenanceError::Io)?;
    let mut copy = CipherArchive {
        path,
        file: Some(file),
        identity: Some(identity),
    };
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut length = 0_u64;
    let limit = archive_copy_limit()?;
    loop {
        let read = original
            .read(&mut buffer)
            .await
            .map_err(|_| MaintenanceError::Io)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .filter(|length| *length <= limit)
            .ok_or(MaintenanceError::InvalidBundle)?;
        copy.reader()?
            .write_all(&buffer[..read])
            .await
            .map_err(|_| MaintenanceError::Io)?;
    }
    copy.reader()?
        .flush()
        .await
        .map_err(|_| MaintenanceError::Io)?;
    copy.reader()?
        .sync_all()
        .await
        .map_err(|_| MaintenanceError::Io)?;
    copy.reader()?
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    copy.check()?;
    parent
        .check()
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    Ok(copy)
}

async fn extract_tar<R: AsyncRead + Unpin>(
    input: &mut R,
    root: &Path,
) -> Result<(), MaintenanceError> {
    let mut header = Zeroizing::new([0; 512]);
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut seen = BTreeSet::new();
    let mut path_bytes = 0_usize;
    loop {
        input
            .read_exact(&mut header[..])
            .await
            .map_err(|_| MaintenanceError::TruncatedPostgresTarHeader)?;
        if header.iter().all(|byte| *byte == 0) {
            input
                .read_exact(&mut header[..])
                .await
                .map_err(|_| MaintenanceError::InvalidPostgresTarTerminator)?;
            if header.iter().any(|byte| *byte != 0) {
                return Err(MaintenanceError::InvalidPostgresTarTerminator);
            }
            let mut trailing = [0; 1];
            if input
                .read(&mut trailing)
                .await
                .map_err(|_| MaintenanceError::InvalidPostgresTarTerminator)?
                != 0
            {
                return Err(MaintenanceError::InvalidPostgresTarTerminator);
            }
            return Ok(());
        }
        let entry = tar_header(&header)?;
        let spelling = entry
            .relative
            .to_str()
            .ok_or(MaintenanceError::UnsafePostgresTarPath)?
            .to_ascii_lowercase();
        path_bytes = path_bytes
            .checked_add(spelling.len())
            .filter(|count| *count <= MAX_TAR_PATH_BYTES)
            .ok_or(MaintenanceError::InvalidPostgresTarOrdering)?;
        if seen.len() >= MAX_TAR_ENTRIES || !seen.insert(spelling) {
            return Err(MaintenanceError::InvalidPostgresTarOrdering);
        }
        if entry.directory {
            parents(root, &entry.relative).await?;
            continue;
        }
        if let Some(parent) = entry.relative.parent() {
            parents(root, parent).await?;
        }
        let mut file = new_file(&root.join(&entry.relative)).await?;
        let mut remaining = entry.size;
        while remaining != 0 {
            let take = remaining.min(CHUNK_BYTES as u64) as usize;
            input
                .read_exact(&mut buffer[..take])
                .await
                .map_err(|_| MaintenanceError::TruncatedPostgresTarData)?;
            file.write_all(&buffer[..take])
                .await
                .map_err(|_| MaintenanceError::Io)?;
            remaining -= take as u64;
        }
        let padding = ((512 - entry.size % 512) % 512) as usize;
        input
            .read_exact(&mut header[..padding])
            .await
            .map_err(|_| MaintenanceError::InvalidPostgresTarPadding)?;
        // USTAR padding bytes are not file content; consume the exact aligned
        // length, while the enclosing authenticated stream protects their bytes.
        file.flush().await.map_err(|_| MaintenanceError::Io)?;
        file.sync_all().await.map_err(|_| MaintenanceError::Io)?;
        drop(file.into_std().await);
    }
}

async fn extract_bundle<R: AsyncRead + Unpin>(
    input: &mut R,
    tar_output: &mut tokio::io::DuplexStream,
    blobs_root: &Path,
    manifest: &BundleManifest,
    encrypted_blobs: bool,
) -> Result<(), MaintenanceError> {
    let mut magic = [0; 8];
    input
        .read_exact(&mut magic)
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    if &magic != BUNDLE_MAGIC {
        return Err(MaintenanceError::InvalidBundle);
    }
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut pg_size = 0_u64;
    let mut pg_hash = Sha256::new();
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
            .filter(|size| *size <= MAX_PG_BYTES)
            .ok_or(MaintenanceError::InvalidBundle)?;
        input
            .read_exact(&mut buffer[..length])
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        pg_hash.update(&buffer[..length]);
        tar_output
            .write_all(&buffer[..length])
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
    }
    tar_output
        .shutdown()
        .await
        .map_err(|_| MaintenanceError::Io)?;
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
    if count != manifest.blobs.len() || count > 4096 {
        return Err(MaintenanceError::InvalidBundle);
    }
    for expected in &manifest.blobs {
        let mut id = [0; 16];
        input
            .read_exact(&mut id)
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        let length = input
            .read_u64()
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        if Uuid::from_bytes(id).to_string() != expected.key
            || length != expected.byte_length
            || !(104..=MAX_BLOB_BYTES).contains(&length)
        {
            return Err(MaintenanceError::InvalidBundle);
        }
        let mut file = new_file(&blobs_root.join(&expected.key)).await?;
        let mut remaining = length;
        let mut physical_hash = Sha256::new();
        let mut payload_hash = Sha256::new();
        let mut payload_checksum = [0; 32];
        while remaining != 0 {
            let take = remaining.min(CHUNK_BYTES as u64) as usize;
            input
                .read_exact(&mut buffer[..take])
                .await
                .map_err(|_| MaintenanceError::InvalidBundle)?;
            physical_hash.update(&buffer[..take]);
            if remaining == length {
                if &buffer[..8] != b"PXELBLB\x01"
                    || u64::from_le_bytes(
                        buffer[8..16]
                            .try_into()
                            .map_err(|_| MaintenanceError::InvalidBundle)?,
                    )
                    .checked_add(48)
                        != Some(length)
                {
                    return Err(MaintenanceError::InvalidBundle);
                }
                payload_checksum.copy_from_slice(&buffer[16..48]);
                if encrypted_blobs
                    && (&buffer[48..56] != b"PXELENC\0" || buffer[56..60] != [0, 1, 0, 1])
                {
                    return Err(MaintenanceError::InvalidBundle);
                }
                if encrypted_blobs {
                    let generation = u32::from_be_bytes(
                        buffer[60..64]
                            .try_into()
                            .map_err(|_| MaintenanceError::InvalidBundle)?,
                    );
                    if generation == 0 || generation > i32::MAX as u32 {
                        return Err(MaintenanceError::InvalidBundle);
                    }
                }
                payload_hash.update(&buffer[48..take]);
            } else {
                payload_hash.update(&buffer[..take]);
            }
            file.write_all(&buffer[..take])
                .await
                .map_err(|_| MaintenanceError::Io)?;
            remaining -= take as u64;
        }
        if physical_hash.finalize().as_slice() != expected.checksum
            || payload_hash.finalize().as_slice() != payload_checksum
        {
            return Err(MaintenanceError::InvalidBundle);
        }
        file.flush().await.map_err(|_| MaintenanceError::Io)?;
        file.sync_all().await.map_err(|_| MaintenanceError::Io)?;
    }
    input
        .read_exact(&mut magic)
        .await
        .map_err(|_| MaintenanceError::InvalidBundle)?;
    let mut extra = [0; 1];
    if &magic != BUNDLE_END
        || input
            .read(&mut extra)
            .await
            .map_err(|_| MaintenanceError::InvalidBundle)?
            != 0
    {
        return Err(MaintenanceError::InvalidBundle);
    }
    Ok(())
}

async fn decrypt_and_extract(
    keys: &LocalKeyProvider,
    input: &mut File,
    manifest: &BundleManifest,
    pgdata: &Path,
    blobs: &Path,
    encrypted_blobs: bool,
) -> Result<(), MaintenanceError> {
    let (mut producer, mut consumer) = tokio::io::duplex(CHUNK_BYTES);
    let (mut tar_producer, mut tar_consumer) = tokio::io::duplex(CHUNK_BYTES);
    let decrypt = async {
        let report = Box::pin(decrypt_backup(keys, input, &mut producer))
            .await
            .map_err(|_| MaintenanceError::Encryption)?;
        producer
            .shutdown()
            .await
            .map_err(|_| MaintenanceError::Io)?;
        let repeated: BundleManifest = serde_json::from_slice(&report.manifest)
            .map_err(|_| MaintenanceError::InvalidBundle)?;
        if repeated.format_version != manifest.format_version
            || repeated.postgres_version != manifest.postgres_version
            || repeated.blobs != manifest.blobs
        {
            return Err(MaintenanceError::InvalidBundle);
        }
        Ok::<_, MaintenanceError>(())
    };
    tokio::try_join!(
        decrypt,
        extract_bundle(
            &mut consumer,
            &mut tar_producer,
            blobs,
            manifest,
            encrypted_blobs
        ),
        async {
            extract_tar(&mut tar_consumer, pgdata)
                .await
                .map_err(|error| match error {
                    MaintenanceError::InvalidBundle => MaintenanceError::InvalidPostgresTar,
                    other => other,
                })
        },
    )?;
    Ok(())
}

struct PitrRecovery {
    archive_directory: PathBuf,
    target_name: String,
    keyring_path: PathBuf,
}

#[derive(Clone, Copy)]
enum Finalization<'a> {
    Renew {
        to: &'a str,
        pitr: Option<&'a PitrRecovery>,
    },
    VerifyOnly,
}

fn valid_restore_target_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn safe_shell_path(path: &Path) -> Result<String, MaintenanceError> {
    let path = launcher_path(path)?;
    if path.is_empty()
        || !path.is_ascii()
        || path.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'\\' | b':' | b'.' | b'_' | b'-'))
        })
    {
        return Err(MaintenanceError::UnsafeDestination);
    }
    Ok(path)
}

fn pitr_command(archive_directory: &Path) -> Result<String, MaintenanceError> {
    let executable = std::env::current_exe().map_err(|_| MaintenanceError::Configuration)?;
    let executable = safe_shell_path(&executable)?;
    let archive_directory = safe_shell_path(archive_directory)?;
    #[cfg(windows)]
    let command = format!(r#""{executable}" backup wal-restore "{archive_directory}" %f "%p""#);
    #[cfg(not(windows))]
    let command = format!("'{executable}' backup wal-restore '{archive_directory}' %f '%p'");
    Ok(command)
}

fn validate_connection(connection: &Connection) -> Result<(), MaintenanceError> {
    if !matches!(connection.host.as_str(), "localhost" | "127.0.0.1")
        || connection.sslmode == "require"
        || connection.password.is_empty()
        || connection.database.is_empty()
    {
        return Err(MaintenanceError::Configuration);
    }
    let port: u16 = connection
        .port
        .parse()
        .map_err(|_| MaintenanceError::Configuration)?;
    if port < 1024 {
        return Err(MaintenanceError::Configuration);
    }
    Ok(())
}

pub(super) fn launcher_path(path: &Path) -> Result<String, MaintenanceError> {
    let text = path.to_str().ok_or(MaintenanceError::UnsafeDestination)?;
    #[cfg(windows)]
    let text = {
        let text = match text.strip_prefix(r"\\?\") {
            Some(local)
                if local.as_bytes().get(1) == Some(&b':')
                    && local
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphabetic) =>
            {
                local
            }
            Some(_) => return Err(MaintenanceError::UnsafeDestination),
            None => text,
        };
        if text.starts_with(['\\', '/']) {
            return Err(MaintenanceError::UnsafeDestination);
        }
        text
    };
    if text.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\'' | '"' | '`' | '$' | '!' | '&' | ';' | '|' | '<' | '>' | '%'
            )
    }) {
        return Err(MaintenanceError::UnsafeDestination);
    }
    #[cfg(windows)]
    {
        Ok(text.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        if text.contains('\\') {
            return Err(MaintenanceError::UnsafeDestination);
        }
        Ok(text.to_owned())
    }
}

pub(super) async fn checked_binary(bin: &Path, name: &str) -> Result<PathBuf, MaintenanceError> {
    let path = bin.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    launcher_path(&path)?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        pg_command(&path).arg("--version").output(),
    )
    .await
    .map_err(|_| MaintenanceError::UnsupportedCluster)?
    .map_err(|_| MaintenanceError::UnsupportedCluster)?;
    let expected = format!("{name} (PostgreSQL) 18.6");
    if !output.status.success()
        || std::str::from_utf8(&output.stdout).ok().map(str::trim) != Some(expected.as_str())
    {
        return Err(MaintenanceError::UnsupportedCluster);
    }
    Ok(path)
}

async fn write_control_file(path: &Path, bytes: &[u8]) -> Result<(), MaintenanceError> {
    let mut file = new_file(path).await?;
    file.write_all(bytes)
        .await
        .map_err(|_| MaintenanceError::Io)?;
    file.flush().await.map_err(|_| MaintenanceError::Io)?;
    file.sync_all().await.map_err(|_| MaintenanceError::Io)
}

async fn quarantine_auto_config(root: &Path, auto: &Path) -> Result<(), MaintenanceError> {
    let metadata = tokio::fs::symlink_metadata(auto)
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    if !metadata.is_file() || metadata.len() > MAX_AUTO_CONFIG_BYTES {
        return Err(MaintenanceError::RecoveryConfiguration(
            "postgresql.auto.conf",
        ));
    }
    let mut source = File::open(auto)
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    let mut archive = new_file(&root.join("archived-postgresql-auto.conf"))
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    let mut buffer = Zeroizing::new(vec![0_u8; 16 * 1024]);
    let mut length = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .filter(|length| *length <= MAX_AUTO_CONFIG_BYTES)
            .ok_or(MaintenanceError::RecoveryConfiguration(
                "postgresql.auto.conf",
            ))?;
        archive
            .write_all(&buffer[..read])
            .await
            .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    }
    archive
        .flush()
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    archive
        .sync_all()
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    drop(archive.into_std().await);
    drop(source.into_std().await);
    let mut sanitized = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(auto)
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    sanitized
        .write_all(b"# original settings archived for isolated recovery\n")
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    sanitized
        .sync_all()
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.auto.conf"))?;
    drop(sanitized.into_std().await);
    Ok(())
}

async fn recovery_config(
    root: &Path,
    pgdata: &Path,
    read_only: bool,
    pitr: Option<&PitrRecovery>,
) -> Result<PathBuf, MaintenanceError> {
    // pg_verifybackup deliberately ignores auto.conf. Preserve its original bytes
    // privately while preventing restored includes or commands from being read.
    let auto = pgdata.join("postgresql.auto.conf");
    match tokio::fs::symlink_metadata(&auto).await {
        Ok(_) => quarantine_auto_config(root, &auto).await?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(MaintenanceError::Io),
    }
    for signal in ["standby.signal", "recovery.signal"] {
        if tokio::fs::symlink_metadata(pgdata.join(signal))
            .await
            .is_ok()
        {
            return Err(MaintenanceError::UnsupportedCluster);
        }
    }
    if let Some(pitr) = pitr
        && (read_only || !valid_restore_target_name(&pitr.target_name))
    {
        return Err(MaintenanceError::Arguments);
    }
    let hba = root.join("restore-pg-hba.conf");
    let ident = root.join("restore-pg-ident.conf");
    write_control_file(&hba, b"host all all 127.0.0.1/32 scram-sha-256\n")
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("pg_hba.conf"))?;
    write_control_file(&ident, b"# isolated restore\n")
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("pg_ident.conf"))?;
    let config = root.join("restore-postgresql.conf");
    let pitr_settings = match pitr {
        Some(pitr) => {
            let command = pitr_command(&pitr.archive_directory)?;
            format!(
                "restore_command = '{}'\nrecovery_target_name = '{}'\nrecovery_target_action = 'promote'\n",
                command.replace('\'', "''"),
                pitr.target_name
            )
        }
        None => "restore_command = ''\n".to_owned(),
    };
    let text = format!(
        "hba_file = '{}'\nident_file = '{}'\nlisten_addresses = '127.0.0.1'\nunix_socket_directories = ''\nssl = off\nfsync = on\nfull_page_writes = on\nsynchronous_commit = on\nshared_buffers = '32MB'\nmax_connections = 20\nhot_standby = off
max_wal_senders = 0\nmax_logical_replication_workers = 0\nautovacuum = off\narchive_mode = off\narchive_command = ''\narchive_library = ''\n{}archive_cleanup_command = ''\nrecovery_end_command = ''\nprimary_conninfo = ''\nprimary_slot_name = ''\nshared_preload_libraries = ''\nsession_preload_libraries = ''\nlocal_preload_libraries = ''\nlogging_collector = off\nlog_destination = 'stderr'\nlog_statement = 'none'\nlog_duration = off\nlog_min_duration_statement = -1\nlog_min_duration_sample = -1\nlog_statement_sample_rate = 0\nlog_parameter_max_length = 0\nlog_parameter_max_length_on_error = 0\nlog_min_error_statement = panic\nlog_min_messages = panic\ndefault_transaction_read_only = {}\n",
        launcher_path(&hba)?,
        launcher_path(&ident)?,
        pitr_settings,
        if read_only { "on" } else { "off" }
    );
    write_control_file(&config, text.as_bytes())
        .await
        .map_err(|_| MaintenanceError::RecoveryConfiguration("postgresql.conf"))?;
    if pitr.is_some() {
        write_control_file(&pgdata.join("recovery.signal"), b"").await?;
    }
    Ok(config)
}

pub(super) async fn stop_cluster(pg_ctl: &Path, pgdata: &Path) -> Result<(), MaintenanceError> {
    match tokio::fs::symlink_metadata(pgdata.join("postmaster.pid")).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(MaintenanceError::Postgres),
        Ok(_) => {}
    }
    let status = tokio::time::timeout(
        Duration::from_secs(40),
        pg_command(pg_ctl)
            .args(["stop", "-D"])
            .arg(launcher_path(pgdata)?)
            .args(["-m", "fast", "-w", "-t", "30", "-s"])
            .stdout(Stdio::null())
            .status(),
    )
    .await
    .map_err(|_| MaintenanceError::Postgres)?
    .map_err(|_| MaintenanceError::Postgres)?;
    if !status.success() {
        return Err(MaintenanceError::Postgres);
    }
    Ok(())
}

pub(super) struct RecoverySignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
}

impl RecoverySignals {
    pub(super) fn install() -> Result<Self, MaintenanceError> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())
                    .map_err(|_| MaintenanceError::Configuration)?,
                terminate: signal(SignalKind::terminate())
                    .map_err(|_| MaintenanceError::Configuration)?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                interrupt: tokio::signal::windows::ctrl_c()
                    .map_err(|_| MaintenanceError::Configuration)?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(MaintenanceError::UnsupportedCluster)
        }
    }

    pub(super) async fn wait(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
        #[cfg(windows)]
        {
            self.interrupt.recv().await;
        }
    }
}

pub(super) struct RecoveryLifecycle {
    pub(super) pg_ctl: PathBuf,
    pub(super) pgdata: String,
    pub(super) armed: bool,
}

impl Drop for RecoveryLifecycle {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // A cancelled Rust future cannot await. Keep this fallback bounded and
        // private; normal errors/signals use the typed asynchronous fast stop.
        // SIGKILL/host crashes cannot run Drop and require manual staging cleanup.
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            let mut command = std::process::Command::new(&self.pg_ctl);
            command
                .env_clear()
                .args(["stop", "-D"])
                .arg(&self.pgdata)
                .args(["-m", "immediate", "-w", "-t", "2", "-s"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                if let Some(root) = std::env::var_os("SystemRoot") {
                    command.env("SystemRoot", root);
                }
                command.creation_flags(0x0800_0000);
            }
            let Ok(mut child) = command.spawn() else {
                return;
            };
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return,
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if started.elapsed() < Duration::from_secs(3) => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.try_wait();
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

async fn inspect_cluster(database: &PgDatabase, pgdata: &Path) -> Result<(), MaintenanceError> {
    let guard = database
        .begin_maintenance()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    let information = guard
        .information()
        .await
        .map_err(|_| MaintenanceError::Database)?;
    let data_directory = directory(Path::new(&information.data_directory), true)?;
    if information.server_version_num != 180006
        || information.in_recovery
        || !information.durable_settings
        || data_directory != pgdata
        || information.tablespace_names != ["pg_default", "pg_global"]
    {
        return Err(MaintenanceError::UnsupportedCluster);
    }
    guard.finish().await.map_err(|_| MaintenanceError::Database)
}

async fn connect_restored_database(
    url: &str,
    keys: Arc<dyn KeyProvider>,
) -> Result<PgDatabase, MaintenanceError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(database) =
                PgDatabase::connect_for_restored_cluster(url, 2, Arc::clone(&keys)).await
            {
                return Ok(database);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| MaintenanceError::RecoveryDatabaseConnection)?
}

async fn recover_and_finalize(
    root: &Path,
    pgdata: &Path,
    bin: &Path,
    connection: &Connection,
    keys: Arc<LocalKeyProvider>,
    mode: Finalization<'_>,
) -> Result<(), MaintenanceError> {
    let verifier = checked_binary(bin, "pg_verifybackup").await?;
    checked_binary(bin, "pg_waldump").await?;
    let pg_ctl = checked_binary(bin, "pg_ctl").await?;
    checked_binary(bin, "postgres").await?;
    let status = tokio::time::timeout(
        Duration::from_secs(3600),
        pg_command(&verifier)
            .args(["--exit-on-error", "--quiet"])
            .arg(launcher_path(pgdata)?)
            .status(),
    )
    .await
    .map_err(|_| MaintenanceError::PostgresVerification)?
    .map_err(|_| MaintenanceError::PostgresVerification)?;
    if !status.success() {
        return Err(MaintenanceError::PostgresVerification);
    }
    let read_only = matches!(mode, Finalization::VerifyOnly);
    let pitr = match mode {
        Finalization::Renew { pitr, .. } => pitr,
        Finalization::VerifyOnly => None,
    };
    let config = recovery_config(root, pgdata, read_only, pitr).await?;
    // Reject an occupied port before starting; a failed child never connects to
    // an existing cluster. The recovered data_directory is checked before writes.
    let port: u16 = connection
        .port
        .parse()
        .map_err(|_| MaintenanceError::Configuration)?;
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .map_err(|_| MaintenanceError::Configuration)?;
    drop(listener);
    let options = format!(
        "-c config_file=\"{}\" -c port={}",
        launcher_path(&config)?,
        port
    );
    // Registration is synchronous and precedes the first startup spawn. A signal
    // already queued wins before the operation can start a postmaster.
    let mut signals = RecoverySignals::install()?;
    let mut lifecycle = RecoveryLifecycle {
        pg_ctl: pg_ctl.clone(),
        pgdata: launcher_path(pgdata)?,
        armed: false,
    };
    let result = {
        let operation = async {
            lifecycle.armed = true;
            let mut startup = pg_command(&pg_ctl);
            startup
                .kill_on_drop(false)
                .args(["start", "-D"])
                .arg(&lifecycle.pgdata)
                .args(["-w", "-t", "60", "-s", "-l"])
                .arg(launcher_path(&root.join("restore-postgres.log"))?)
                .arg("-o")
                .arg(&options)
                .stdout(Stdio::null());
            if let Some(pitr) = pitr {
                startup.env("PXEL_KEYRING_FILE", &pitr.keyring_path);
            }
            let status = tokio::time::timeout(Duration::from_secs(70), startup.status())
                .await
                .map_err(|_| MaintenanceError::Postgres)?
                .map_err(|_| MaintenanceError::Postgres)?;
            if !status.success() {
                return Err(MaintenanceError::Postgres);
            }
            let database = connect_restored_database(&connection.url, keys).await?;
            let result = async {
                inspect_cluster(&database, pgdata).await?;
                if let Finalization::Renew { to, .. } = mode {
                    database
                        .readiness()
                        .await
                        .map_err(|_| MaintenanceError::Database)?;
                    database
                        .renew_write_generations_after_restore(to)
                        .await
                        .map_err(|_| MaintenanceError::Encryption)?;
                    database
                        .encryption_readiness()
                        .await
                        .map_err(|_| MaintenanceError::Encryption)?;
                }
                Ok::<_, MaintenanceError>(())
            }
            .await;
            database.close().await;
            result
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = signals.wait() => Err(MaintenanceError::Io),
            result = &mut operation => result,
        }
    };
    // pg_ctl wait targets the launcher process, never the postmaster descendants.
    // Even a timed-out startup can continue in the background and must be stopped.
    if lifecycle.armed {
        stop_cluster(&pg_ctl, pgdata)
            .await
            .map_err(|_| MaintenanceError::RecoveryStopFailed)?;
        lifecycle.armed = false;
    }
    result?;
    write_control_file(
        &root.join(if read_only {
            "VERIFIED_ONLY"
        } else {
            "COMPLETE"
        }),
        b"verified private staging; service publication requires explicit configuration\n",
    )
    .await
}

async fn restore_stage(
    archive: &Path,
    new_root: &Path,
    connection: &Connection,
    mode: Finalization<'_>,
) -> Result<(), MaintenanceError> {
    validate_connection(connection)?;
    let binary = pg_binary().await?;
    let bin = binary.parent().ok_or(MaintenanceError::Configuration)?;
    launcher_path(bin)?;
    let absolute =
        std::path::absolute(new_root).map_err(|_| MaintenanceError::UnsafeDestination)?;
    launcher_path(&absolute)?;
    let parent_path = absolute
        .parent()
        .ok_or(MaintenanceError::UnsafeDestination)?
        .to_owned();
    let parent = tokio::task::spawn_blocking(move || ProtectedDirectory::acquire(&parent_path))
        .await
        .map_err(|_| MaintenanceError::UnsafeDestination)?
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    let destination = parent.path.join(
        absolute
            .file_name()
            .ok_or(MaintenanceError::UnsafeDestination)?,
    );
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(MaintenanceError::UnsafeDestination);
    }
    let keys = Arc::new(
        LocalKeyProvider::from_environment().map_err(|_| MaintenanceError::Configuration)?,
    );
    if let Finalization::Renew { to, .. } = mode {
        keys.physical_key_id(to)
            .map_err(|_| MaintenanceError::Configuration)?;
    }
    let mut secret_guards = Vec::new();
    for variable in ["PXEL_KEYRING_FILE", "PXEL_RESTORE_DATABASE_URL_FILE"] {
        if let Some(path) = std::env::var_os(variable) {
            secret_guards.push(excluded_secret_path(Path::new(&path), &[&parent.path])?);
        }
    }
    if let Finalization::Renew {
        pitr: Some(pitr), ..
    } = mode
    {
        if std::env::var_os("PXEL_KEYRING_JSON").is_some()
            || std::env::var_os("PXEL_KEYRING_FILE").is_none()
            || pitr.archive_directory.starts_with(&destination)
            || destination.starts_with(&pitr.archive_directory)
        {
            return Err(MaintenanceError::UnsafeSource);
        }
        let _keyring = stable_regular_file(&pitr.keyring_path)?;
        let archive_directory = ProtectedDirectory::acquire(&pitr.archive_directory)?;
        if archive_directory.path == destination
            || archive_directory.path.starts_with(&destination)
            || destination.starts_with(&archive_directory.path)
        {
            return Err(MaintenanceError::UnsafeSource);
        }
    }
    let original = stable_regular_file(archive)?;
    let identity =
        same_file::Handle::from_file(original.try_clone().map_err(|_| MaintenanceError::Io)?)
            .map_err(|_| MaintenanceError::Io)?;
    let mut original = File::from_std(original);
    let mut input = copy_ciphertext(&mut original, &parent).await?;
    require_identity(archive, &identity)?;
    // A mutable source cannot change this owned ciphertext after authentication.
    // Both passes read only the copy; decrypted PG bytes never use a temp archive.
    let manifest = Box::pin(verify_reader(&keys, input.reader()?)).await?;
    require_identity(archive, &identity)?;
    parent
        .check()
        .map_err(|_| MaintenanceError::UnsafeDestination)?;
    let stage = new_private_root(&destination).await?;
    write_control_file(
        &stage.path.join("PENDING"),
        b"unpublished private restore; automatic retry to this path is refused\n",
    )
    .await?;
    let pgdata = new_private_root(&stage.path.join("pgdata")).await?;
    let blobs = new_private_root(&stage.path.join("blobs")).await?;
    input
        .reader()?
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| MaintenanceError::Io)?;
    Box::pin(decrypt_and_extract(
        &keys,
        input.reader()?,
        &manifest,
        &pgdata.path,
        &blobs.path,
        matches!(mode, Finalization::Renew { .. }),
    ))
    .await?;
    input.check()?;
    stage.check()?;
    pgdata.check()?;
    blobs.check()?;
    Box::pin(recover_and_finalize(
        &stage.path,
        &pgdata.path,
        bin,
        connection,
        keys,
        mode,
    ))
    .await?;
    // All artifacts remain private. The archive identity remains live through
    // final verification; this function never changes a server configuration.
    stage.check()?;
    drop(secret_guards);
    Ok(())
}

pub(super) async fn verify_physical_recovery(
    archive: &Path,
    new_root: &Path,
    connection: &Connection,
) -> Result<(), MaintenanceError> {
    Box::pin(restore_stage(
        archive,
        new_root,
        connection,
        Finalization::VerifyOnly,
    ))
    .await
}

pub(super) async fn run(arguments: &[OsString]) -> Result<(), MaintenanceError> {
    let (archive, new_root, target, pitr) = match arguments {
        [command, action, archive, root, option, target, confirmed]
            if command == "backup"
                && action == "restore"
                && option == "--to"
                && confirmed == "--maintenance-confirmed" =>
        {
            let target = target
                .to_str()
                .filter(|value| !value.is_empty())
                .ok_or(MaintenanceError::Arguments)?;
            (Path::new(archive), Path::new(root), Some(target), None)
        }
        [
            command,
            action,
            archive,
            root,
            option,
            target,
            target_option,
            target_name,
            archive_option,
            archive_directory,
            confirmed,
        ] if command == "backup"
            && action == "restore"
            && option == "--to"
            && target_option == "--target-name"
            && archive_option == "--wal-archive-dir"
            && confirmed == "--maintenance-confirmed" =>
        {
            let target = target
                .to_str()
                .filter(|value| !value.is_empty())
                .ok_or(MaintenanceError::Arguments)?;
            let target_name = target_name
                .to_str()
                .filter(|value| valid_restore_target_name(value))
                .ok_or(MaintenanceError::Arguments)?;
            if archive_directory.is_empty() || archive_directory.to_string_lossy().starts_with('-')
            {
                return Err(MaintenanceError::Arguments);
            }
            if std::env::var_os("PXEL_KEYRING_JSON").is_some() {
                return Err(MaintenanceError::Configuration);
            }
            let keyring = std::env::var_os("PXEL_KEYRING_FILE")
                .map(PathBuf::from)
                .ok_or(MaintenanceError::Configuration)?;
            let archive_directory = std::path::absolute(archive_directory)
                .map_err(|_| MaintenanceError::UnsafeSource)?;
            let keyring_path =
                std::path::absolute(keyring).map_err(|_| MaintenanceError::Configuration)?;
            let recovery = PitrRecovery {
                archive_directory,
                target_name: target_name.to_owned(),
                keyring_path,
            };
            (
                Path::new(archive),
                Path::new(root),
                Some(target),
                Some(recovery),
            )
        }
        [command, action, archive, root, confirmed]
            if command == "backup"
                && action == "recovery-check"
                && confirmed == "--maintenance-confirmed" =>
        {
            (Path::new(archive), Path::new(root), None, None)
        }
        _ => return Err(MaintenanceError::Arguments),
    };
    if archive.as_os_str().is_empty()
        || new_root.as_os_str().is_empty()
        || archive.as_os_str().to_string_lossy().starts_with('-')
        || new_root.as_os_str().to_string_lossy().starts_with('-')
        || std::env::var_os("PXEL_RESTORE_DATABASE_URL").is_none()
            && std::env::var_os("PXEL_RESTORE_DATABASE_URL_FILE").is_none()
    {
        return Err(MaintenanceError::Arguments);
    }
    let connection = connection_from_sources(
        "PXEL_RESTORE_DATABASE_URL",
        "PXEL_RESTORE_DATABASE_URL_FILE",
        ".local/restore-database-url",
    )?;
    match target {
        Some(to) => {
            Box::pin(restore_stage(
                archive,
                new_root,
                &connection,
                Finalization::Renew {
                    to,
                    pitr: pitr.as_ref(),
                },
            ))
            .await?;
            println!("private restore recovered and write generations renewed; cluster stopped");
        }
        None => {
            verify_physical_recovery(archive, new_root, &connection).await?;
            println!("private backup recovery verified; cluster stopped and unpublished");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::storage_maintenance::BlobEntry;
    use spreadsheet_application::{BlobSource, BlobStore, BlobStoreError};
    use spreadsheet_domain::{BlobKey, Checksum};
    use spreadsheet_storage::LocalFilesystemBlobStore;

    fn header(name: &str, kind: u8, length: u64) -> [u8; 512] {
        let mut value = [0; 512];
        value[..name.len()].copy_from_slice(name.as_bytes());
        value[124..136].copy_from_slice(format!("{length:011o}\0").as_bytes());
        value[156] = kind;
        value[257..263].copy_from_slice(b"ustar\0");
        value[263..265].copy_from_slice(b"00");
        value[148..156].fill(b' ');
        let checksum: u64 = value.iter().map(|byte| u64::from(*byte)).sum();
        value[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        value
    }

    #[test]
    fn strict_tar_paths_reject_traversal_absolute_aliases_and_devices() {
        for name in [
            "/absolute",
            "../outside",
            "base/../../outside",
            "base//file",
            "base/./file",
            "C:/outside",
            "base\\file",
            "base/file:stream",
            "NUL",
            "base/COM1.txt",
            "base/file.",
            "base/file ",
            "base/\nfile",
            "pg_tblspc/42",
            "PG_TBLSPC/42",
            "postmaster.pid",
            "POSTMASTER.PID",
        ] {
            assert!(tar_path(name, false).is_err(), "unsafe fixture path");
        }
        assert_eq!(
            tar_path("base/123/456", false).expect("safe PG path"),
            PathBuf::from("base/123/456")
        );
        assert!(tar_path("pg_wal/", true).is_ok());
        assert_eq!(
            tar_path("./pg_wal/archive_status/", true).expect("PostgreSQL synthetic directory"),
            PathBuf::from("pg_wal/archive_status")
        );
        assert!(tar_path("./../outside", false).is_err());
    }

    #[test]
    fn strict_tar_headers_require_ustar_checksum_regular_files_or_directories() {
        assert!(tar_header(&header("PG_VERSION", b'0', 3)).is_ok());
        assert!(tar_header(&header("base/", b'5', 0)).is_ok());
        assert!(tar_header(&header("./", b'5', 0)).is_ok());
        assert!(tar_header(&header("./", b'0', 0)).is_err());
        for kind in *b"123467xgSL" {
            assert!(tar_header(&header("unsafe", kind, 0)).is_err());
        }
        assert!(tar_header(&header("base/", b'5', 1)).is_err());
        assert!(tar_header(&header("tablespace_map", b'0', 1)).is_err());
        assert!(tar_header(&header("TABLESPACE_MAP", b'0', 1)).is_err());
        assert!(tar_header(&header("tablespace_map", b'0', 0)).is_ok());
        let mut invalid = header("PG_VERSION", b'0', 3);
        invalid[0] ^= 1;
        assert!(tar_header(&invalid).is_err());
        invalid = header("PG_VERSION", b'0', 3);
        invalid[124] = 0x80;
        assert!(tar_header(&invalid).is_err());
    }

    fn tar_fixture() -> Vec<u8> {
        let mut bytes = header("PG_VERSION", b'0', 3).to_vec();
        bytes.extend_from_slice(b"18\n");
        bytes.resize(1024, 0);
        bytes.resize(2048, 0);
        bytes
    }

    #[test]
    fn wal_restore_destination_aliases_are_platform_and_object_scoped() {
        let segment = "000000010000000000000001";
        let history = "00000002.history";
        let backup = "000000010000000000000002.00000028.backup";
        assert!(wal_destination_matches(segment, segment));
        assert!(wal_destination_matches(history, history));
        assert!(wal_destination_matches(backup, backup));
        #[cfg(windows)]
        {
            assert!(wal_destination_matches(segment, "RECOVERYXLOG"));
            assert!(!wal_destination_matches(history, "RECOVERYXLOG"));
            assert!(!wal_destination_matches(backup, "RECOVERYXLOG"));
            assert!(wal_destination_matches(history, "RECOVERYHISTORY"));
            assert!(wal_destination_matches(backup, "RECOVERYHISTORY"));
            assert!(!wal_destination_matches(segment, "RECOVERYHISTORY"));
        }
        #[cfg(not(windows))]
        {
            assert!(!wal_destination_matches(segment, "RECOVERYXLOG"));
            assert!(!wal_destination_matches(history, "RECOVERYHISTORY"));
        }
    }

    #[tokio::test]
    async fn extraction_requires_complete_end_blocks_padding_and_unique_paths() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let root = temporary.path().join("valid");
        std::fs::create_dir(&root).expect("fixture root");
        let valid = tar_fixture();
        assert!(extract_tar(&mut valid.as_slice(), &root).await.is_ok());
        assert_eq!(
            std::fs::read(root.join("PG_VERSION")).expect("extracted fixture"),
            b"18\n"
        );
        let mut duplicate = valid[..1024].to_vec();
        duplicate.extend_from_slice(&valid);
        let mut case_alias = valid[..1024].to_vec();
        case_alias.extend_from_slice(&header("pg_version", b'0', 3));
        case_alias.extend_from_slice(b"18\n");
        case_alias.resize(2048, 0);
        case_alias.resize(3072, 0);
        let mut nonzero_padding = valid.clone();
        nonzero_padding[600] = 1;
        let nonzero_root = temporary.path().join("nonzero-padding");
        std::fs::create_dir(&nonzero_root).expect("fixture root");
        assert!(
            extract_tar(&mut nonzero_padding.as_slice(), &nonzero_root)
                .await
                .is_ok()
        );
        assert_eq!(
            std::fs::read(nonzero_root.join("PG_VERSION")).expect("nonzero padding fixture"),
            b"18\n"
        );
        let mut trailing = valid.clone();
        trailing.push(0);
        for (index, bytes) in [
            valid[..1023].to_vec(),
            valid[..1536].to_vec(),
            duplicate,
            case_alias,
            trailing,
        ]
        .into_iter()
        .enumerate()
        {
            let root = temporary.path().join(format!("invalid-{index}"));
            std::fs::create_dir(&root).expect("fixture root");
            assert!(extract_tar(&mut bytes.as_slice(), &root).await.is_err());
        }
        assert_eq!(
            std::fs::read(temporary.path().join("valid/PG_VERSION"))
                .expect("other fixture unchanged"),
            b"18\n"
        );
    }

    fn bundle_fixture(key: Uuid, physical: &[u8]) -> (Vec<u8>, BundleManifest) {
        let tar = tar_fixture();
        let mut bundle = BUNDLE_MAGIC.to_vec();
        bundle.extend_from_slice(&(tar.len() as u32).to_be_bytes());
        bundle.extend_from_slice(&tar);
        bundle.extend_from_slice(&0_u32.to_be_bytes());
        bundle.extend_from_slice(&(tar.len() as u64).to_be_bytes());
        bundle.extend_from_slice(&Sha256::digest(&tar));
        bundle.extend_from_slice(&1_u32.to_be_bytes());
        bundle.extend_from_slice(key.as_bytes());
        bundle.extend_from_slice(&(physical.len() as u64).to_be_bytes());
        bundle.extend_from_slice(physical);
        bundle.extend_from_slice(BUNDLE_END);
        let manifest = BundleManifest {
            format_version: 1,
            postgres_version: "18.6".to_owned(),
            blobs: vec![BlobEntry {
                key: key.to_string(),
                byte_length: physical.len() as u64,
                checksum: Sha256::digest(physical).into(),
            }],
        };
        (bundle, manifest)
    }

    #[tokio::test]
    async fn encrypted_blob_accepts_real_aead_and_physical_headers_in_their_respective_byte_orders()
    {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let source_root = temporary.path().join("source");
        let store = LocalFilesystemBlobStore::new(&source_root, 1024)
            .await
            .expect("fixture blob store");
        let keys = LocalKeyProvider::from_keyring_json(&format!(
            r#"{{"schema_version":1,"active_kek_id":"fixture","keys":[{{"id":"fixture","key_hex":"{}"}}]}}"#,
            "11".repeat(32)
        ))
        .expect("synthetic fixture key");
        let nonce = [9; 24];
        let mut envelope = b"PXELENC\0".to_vec();
        envelope.extend_from_slice(&1_u16.to_be_bytes());
        envelope.extend_from_slice(&1_u16.to_be_bytes());
        envelope.extend_from_slice(&128_u32.to_be_bytes());
        envelope.extend_from_slice(&nonce);
        let wrapped = keys
            .wrap_key("fixture", &nonce, &envelope, &[7; 32])
            .expect("real fixture AEAD encryption");
        assert_eq!(
            *keys
                .unwrap_key("fixture", &nonce, &envelope, &wrapped)
                .expect("fixture authentication"),
            [7; 32]
        );
        envelope.extend_from_slice(&wrapped);
        let mut swapped_versions = envelope.clone();
        swapped_versions[8..12].copy_from_slice(&[1, 0, 1, 0]);
        let mut swapped_generation = envelope.clone();
        swapped_generation[12..16].copy_from_slice(&128_u32.to_le_bytes());
        let mut zero_generation = envelope.clone();
        zero_generation[12..16].fill(0);
        let mut oversized_generation = envelope.clone();
        oversized_generation[12..16].copy_from_slice(&(i32::MAX as u32 + 1).to_be_bytes());

        for (index, payload) in [
            envelope,
            swapped_versions,
            swapped_generation,
            zero_generation,
            oversized_generation,
        ]
        .into_iter()
        .enumerate()
        {
            let id = Uuid::new_v4();
            let key = BlobKey::parse(&id.to_string()).expect("fixture identity");
            let checksum = Checksum::from_bytes(Sha256::digest(&payload).into());
            let source: BlobSource = Box::pin(futures_util::stream::once(async move {
                Ok::<_, BlobStoreError>(payload.into())
            }));
            store
                .put(&key, source, checksum)
                .await
                .expect("production physical blob envelope");
            let physical = std::fs::read(source_root.join(key.as_str())).expect("physical fixture");
            let (bundle, manifest) = bundle_fixture(id, &physical);
            let output = temporary.path().join(format!("output-{index}"));
            std::fs::create_dir(&output).expect("fixture output");
            let pgdata = output.join("pgdata");
            let blobs = output.join("blobs");
            std::fs::create_dir(&pgdata).expect("fixture PG data");
            std::fs::create_dir(&blobs).expect("fixture blobs");
            let (mut tar_output, mut tar_input) = tokio::io::duplex(CHUNK_BYTES);
            let mut input = bundle.as_slice();
            let result = tokio::try_join!(
                extract_bundle(&mut input, &mut tar_output, &blobs, &manifest, true),
                extract_tar(&mut tar_input, &pgdata),
            );
            if index == 0 {
                assert!(result.is_ok(), "valid encrypted physical blob rejected");
                assert_eq!(
                    std::fs::read(blobs.join(key.as_str())).expect("restored ciphertext"),
                    physical
                );
            } else {
                assert!(result.is_err(), "invalid crypto header accepted");
            }
        }
    }

    #[tokio::test]
    async fn retained_private_ciphertext_copy_survives_changes_to_the_original_inode() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let parent = new_private_root(&temporary.path().join("private-parent"))
            .await
            .expect("protected fixture parent");
        let source = temporary.path().join("original.fixture");
        let ciphertext = b"synthetic ciphertext fixture";
        std::fs::write(&source, ciphertext).expect("source fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o666))
                .expect("adversarial writable source");
        }
        let source_file = stable_regular_file(&source).expect("stable original handle");
        let identity = same_file::Handle::from_file(source_file.try_clone().expect("source clone"))
            .expect("source identity");
        let mut original = File::from_std(source_file);
        let mut copy = copy_ciphertext(&mut original, &parent)
            .await
            .expect("private staging copy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&source, b"changed original inode").expect("same-inode mutation");
            assert!(require_identity(&source, &identity).is_ok());
            assert_eq!(
                std::fs::metadata(&copy.path)
                    .expect("copy mode")
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        #[cfg(windows)]
        {
            assert!(std::fs::write(&source, b"blocked original mutation").is_err());
            assert!(std::fs::write(&copy.path, b"blocked copy mutation").is_err());
            assert!(std::fs::remove_file(&copy.path).is_err());
        }
        drop(original);
        drop(identity);
        std::fs::write(&source, b"replacement original bytes").expect("later source mutation");
        let mut actual = Vec::new();
        copy.reader()
            .expect("retained copy")
            .read_to_end(&mut actual)
            .await
            .expect("copy contents");
        assert_eq!(actual, ciphertext);
        assert!(copy.check().is_ok());
        let copied_path = copy.path.clone();
        drop(copy);
        assert!(!copied_path.exists());
        assert_eq!(
            std::fs::read(&source).expect("original remains"),
            b"replacement original bytes"
        );
        drop(parent);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_recovery_future_invokes_bounded_stop_guard() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().expect("fixture directory");
        let pgdata = temporary.path().join("pgdata");
        std::fs::create_dir(&pgdata).expect("fake PG data");
        let pg_ctl = temporary.path().join("fake-pg-ctl");
        std::fs::write(
            &pg_ctl,
            b"#!/bin/sh\n[ \"$1\" = stop ] && [ \"$2\" = -D ] && [ \"$4\" = -m ] && [ \"$5\" = immediate ] || exit 1\nprintf stopped > \"$3/stop-called\"\n",
        )
        .expect("owned fixture executable");
        std::fs::set_permissions(&pg_ctl, std::fs::Permissions::from_mode(0o700))
            .expect("fixture executable mode");
        let (ready, ready_rx) = tokio::sync::oneshot::channel();
        let pgdata_text = launcher_path(&pgdata).expect("safe fixture path");
        let task = tokio::spawn(async move {
            let _lifecycle = RecoveryLifecycle {
                pg_ctl,
                pgdata: pgdata_text,
                armed: true,
            };
            let _ = ready.send(());
            std::future::pending::<()>().await;
        });
        ready_rx.await.expect("guard armed before cancellation");
        let started = std::time::Instant::now();
        task.abort();
        assert!(task.await.expect_err("cancelled task").is_cancelled());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            std::fs::read(pgdata.join("stop-called")).expect("cancel fallback executed"),
            b"stopped"
        );
    }

    #[test]
    fn pg_ctl_launcher_paths_reject_shell_metacharacters() {
        for value in [
            "path\"quote",
            "path'quote",
            "path$HOME",
            "path`command",
            "path&command",
            "path;command",
            "path%VARIABLE%",
            "path\ncommand",
        ] {
            assert!(launcher_path(Path::new(value)).is_err());
        }
        assert!(launcher_path(Path::new("safe restore-root/path.conf")).is_ok());
        #[cfg(windows)]
        {
            assert_eq!(
                launcher_path(Path::new(r"\\?\C:\private\pgdata")).expect("local extended path"),
                "C:/private/pgdata"
            );
            assert!(launcher_path(Path::new(r"\\?\UNC\server\share")).is_err());
            assert!(launcher_path(Path::new(r"\\server\share")).is_err());
        }
    }

    #[tokio::test]
    async fn recovery_quarantines_auto_config_while_data_directory_is_protected() {
        let temporary = tempfile::tempdir().expect("fixture directory");
        let root = new_private_root(&temporary.path().join("restore"))
            .await
            .expect("private restore root");
        let pgdata = new_private_root(&root.path.join("pgdata"))
            .await
            .expect("private PostgreSQL data directory");
        let auto = pgdata.path.join("postgresql.auto.conf");
        let original = b"archive_command = 'unexpected command'\ninclude = 'private.conf'\n";
        std::fs::write(&auto, original).expect("untrusted restored auto config");

        quarantine_auto_config(&root.path, &auto)
            .await
            .expect("quarantine while guard is held");

        assert_eq!(
            std::fs::read(root.path.join("archived-postgresql-auto.conf"))
                .expect("preserved private original"),
            original
        );
        assert_eq!(
            std::fs::read(&auto).expect("sanitized auto config"),
            b"# original settings archived for isolated recovery\n"
        );
    }

    #[tokio::test]
    async fn malformed_restore_arguments_reject_before_configuration_or_database() {
        for arguments in [
            vec!["backup", "restore"],
            vec!["backup", "restore", "archive", "root", "--to", "fresh"],
            vec![
                "backup",
                "restore",
                "archive",
                "root",
                "--unknown",
                "fresh",
                "--maintenance-confirmed",
            ],
            vec!["backup", "recovery-check", "archive", "root"],
            vec![
                "backup",
                "recovery-check",
                "archive",
                "root",
                "--maintenance-confirmed",
                "extra",
            ],
        ] {
            let arguments: Vec<_> = arguments.into_iter().map(OsString::from).collect();
            assert!(matches!(
                run(&arguments).await,
                Err(MaintenanceError::Arguments)
            ));
        }
    }
}
