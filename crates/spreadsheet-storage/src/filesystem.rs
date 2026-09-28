use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use spreadsheet_application::{BlobDownload, BlobSource, BlobStore, BlobStoreError};
use spreadsheet_domain::{BlobKey, BlobMetadata, Checksum, StoredBlob};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

const MAGIC: [u8; 8] = *b"PXELBLB\x01";
const HEADER_SIZE: usize = 48;
const CHUNK_SIZE: usize = 64 * 1024;

/// Immutable files in a root owned exclusively by the service account.
///
/// Symlinks/reparse points are rejected, including root ancestors. This is not a
/// sandbox against a malicious local account able to mutate that root: safe std
/// path operations cannot remove every check/open race. Configure private Windows
/// ACLs externally; Unix creation uses 0700 directories and 0600 temporary files.
/// Linux syncs file data and the publication directory. Windows syncs file data
/// and publishes atomically, but safe std offers no directory flush guarantee.
/// Crash-orphaned dot-prefixed temp files are never returned as blobs; operational
/// cleanup may remove them only while the store has no active writers.
#[derive(Clone)]
pub struct LocalFilesystemBlobStore {
    root: PathBuf,
    max_blob_bytes: u64,
}

impl LocalFilesystemBlobStore {
    pub async fn new(
        root: impl Into<PathBuf>,
        max_blob_bytes: u64,
    ) -> Result<Self, BlobStoreError> {
        let root = std::path::absolute(root.into())?;
        let setup_root = root.clone();
        tokio::task::spawn_blocking(move || {
            verify_path(&setup_root, true)?;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&setup_root)?;
            verify_root(&setup_root)
        })
        .await
        .map_err(|_| BlobStoreError::TaskFailed)??;
        Ok(Self {
            root,
            max_blob_bytes,
        })
    }

    /// Validate the configured root without exposing a blob API to HTTP callers.
    pub async fn readiness(&self) -> Result<(), BlobStoreError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || verify_root(&root))
            .await
            .map_err(|_| BlobStoreError::TaskFailed)?
    }

    async fn open_verified(&self, key: &BlobKey) -> Result<(File, BlobMetadata), BlobStoreError> {
        self.readiness().await?;
        let path = self.root.join(key.as_str());
        let stat = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(map_read_error)?;
        if is_link(&stat) || !stat.is_file() {
            return Err(BlobStoreError::UnsafePath);
        }
        let mut file = File::open(path).await.map_err(map_read_error)?;
        let mut header = [0; HEADER_SIZE];
        match file.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(BlobStoreError::CorruptBlob);
            }
            Err(error) => return Err(error.into()),
        }
        if header[..7] != MAGIC[..7] {
            return Err(BlobStoreError::CorruptBlob);
        }
        if header[7] != MAGIC[7] {
            return Err(BlobStoreError::UnsupportedSchema);
        }
        let mut length_bytes = [0; 8];
        length_bytes.copy_from_slice(&header[8..16]);
        let length = u64::from_le_bytes(length_bytes);
        if length > self.max_blob_bytes {
            return Err(BlobStoreError::LimitExceeded {
                limit: self.max_blob_bytes,
            });
        }
        if length.checked_add(HEADER_SIZE as u64) != Some(file.metadata().await?.len()) {
            return Err(BlobStoreError::CorruptBlob);
        }
        let mut expected = [0; 32];
        expected.copy_from_slice(&header[16..]);
        let mut hash = Sha256::new();
        let mut buffer = [0; CHUNK_SIZE];
        let mut size = 0_u64;
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            size = size
                .checked_add(read as u64)
                .ok_or(BlobStoreError::CorruptBlob)?;
            if size > length {
                return Err(BlobStoreError::CorruptBlob);
            }
            hash.update(&buffer[..read]);
        }
        if size != length || hash.finalize().as_slice() != expected {
            return Err(BlobStoreError::ChecksumMismatch);
        }
        file.seek(io::SeekFrom::Start(HEADER_SIZE as u64)).await?;
        Ok((
            file,
            BlobMetadata {
                size_bytes: length,
                checksum: Checksum::from_bytes(expected),
            },
        ))
    }

    async fn sync_directory(&self) -> Result<(), BlobStoreError> {
        #[cfg(unix)]
        {
            let root = self.root.clone();
            tokio::task::spawn_blocking(move || fs::File::open(root)?.sync_all())
                .await
                .map_err(|_| BlobStoreError::TaskFailed)??;
        }
        Ok(())
    }
}

impl BlobStore for LocalFilesystemBlobStore {
    async fn put(
        &self,
        key: &BlobKey,
        mut source: BlobSource,
        expected_checksum: Checksum,
    ) -> Result<StoredBlob, BlobStoreError> {
        self.readiness().await?;
        let root = self.root.clone();
        let temporary = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix(".pending-")
                .tempfile_in(root)
        })
        .await
        .map_err(|_| BlobStoreError::TaskFailed)??;
        // The owning NamedTempFile removes the uncommitted path when cancelled.
        let mut file = File::from_std(temporary.reopen()?);
        file.write_all(&[0; HEADER_SIZE]).await?;
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        while let Some(chunk) = source.next().await {
            let chunk = chunk?;
            size = size
                .checked_add(chunk.len() as u64)
                .filter(|size| *size <= self.max_blob_bytes)
                .ok_or(BlobStoreError::LimitExceeded {
                    limit: self.max_blob_bytes,
                })?;
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        if hash.finalize().as_slice() != expected_checksum.as_bytes() {
            return Err(BlobStoreError::ChecksumMismatch);
        }
        let mut header = [0; HEADER_SIZE];
        header[..8].copy_from_slice(&MAGIC);
        header[8..16].copy_from_slice(&size.to_le_bytes());
        header[16..].copy_from_slice(expected_checksum.as_bytes());
        file.seek(io::SeekFrom::Start(0)).await?;
        file.write_all(&header).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let metadata = BlobMetadata {
            size_bytes: size,
            checksum: expected_checksum,
        };
        // Unlike rename, hard_link never overwrites an existing destination.
        match tokio::fs::hard_link(temporary.path(), self.root.join(key.as_str())).await {
            Ok(()) => self.sync_directory().await?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if self.head(key).await? != metadata {
                    return Err(BlobStoreError::KeyConflict);
                }
                self.sync_directory().await?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(StoredBlob {
            key: key.clone(),
            metadata,
        })
    }

    async fn get(&self, key: &BlobKey) -> Result<BlobDownload, BlobStoreError> {
        let (file, metadata) = self.open_verified(key).await?;
        let remaining = metadata.size_bytes;
        let body = stream::try_unfold((file, remaining), |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            let capacity = remaining.min(CHUNK_SIZE as u64) as usize;
            let mut bytes = vec![0; capacity];
            let read = file.read(&mut bytes).await?;
            if read == 0 {
                return Err(BlobStoreError::CorruptBlob);
            }
            bytes.truncate(read);
            Ok(Some((Bytes::from(bytes), (file, remaining - read as u64))))
        });
        Ok(BlobDownload {
            metadata,
            stream: Box::pin(body),
        })
    }

    async fn head(&self, key: &BlobKey) -> Result<BlobMetadata, BlobStoreError> {
        let (_, metadata) = self.open_verified(key).await?;
        Ok(metadata)
    }

    async fn delete(&self, key: &BlobKey) -> Result<(), BlobStoreError> {
        self.readiness().await?;
        let path = self.root.join(key.as_str());
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if is_link(&metadata) || !metadata.is_file() => {
                return Err(BlobStoreError::UnsafePath);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        match tokio::fs::remove_file(path).await {
            Ok(()) => self.sync_directory().await,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn map_read_error(error: io::Error) -> BlobStoreError {
    if error.kind() == io::ErrorKind::NotFound {
        BlobStoreError::NotFound
    } else {
        BlobStoreError::Io(error)
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn verify_path(path: &Path, allow_missing: bool) -> Result<(), BlobStoreError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(BlobStoreError::UnsafePath);
        }
        current.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link(&metadata) || !metadata.is_dir() => {
                return Err(BlobStoreError::UnsafePath);
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn verify_root(root: &Path) -> Result<(), BlobStoreError> {
    verify_path(root, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(root)?.permissions().mode() & 0o077 != 0 {
            return Err(BlobStoreError::UnsafePath);
        }
    }
    Ok(())
}
