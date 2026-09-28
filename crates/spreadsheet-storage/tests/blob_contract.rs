use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use spreadsheet_application::{BlobSource, BlobStore, BlobStoreError};
use spreadsheet_domain::{BlobKey, Checksum};
use spreadsheet_storage::LocalFilesystemBlobStore;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn checksum(bytes: &[u8]) -> Checksum {
    Checksum::from_bytes(Sha256::digest(bytes).into())
}

fn source(bytes: &'static [u8]) -> BlobSource {
    Box::pin(stream::iter([Ok(Bytes::from_static(bytes))]))
}

fn key() -> Result<BlobKey, spreadsheet_domain::InvalidBlobKey> {
    BlobKey::parse("01990000-0000-7000-8000-000000000001")
}

#[tokio::test]
async fn large_download_is_streamed_in_bounded_chunks() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 200_000).await?;
    let key = key()?;
    let content = vec![37; 150_000];
    let expected = checksum(&content);
    let source: BlobSource = Box::pin(stream::iter([Ok(Bytes::from(content))]));
    store.put(&key, source, expected).await?;
    let mut download = store.get(&key).await?;
    let mut total = 0;
    let mut hash = Sha256::new();
    while let Some(chunk) = download.stream.next().await {
        let chunk = chunk?;
        assert!(chunk.len() <= 64 * 1024);
        total += chunk.len();
        hash.update(chunk);
    }
    assert_eq!(total, 150_000);
    assert_eq!(Checksum::from_bytes(hash.finalize().into()), expected);
    Ok(())
}

#[tokio::test]
async fn regular_file_cannot_be_used_as_root_and_directory_cannot_be_a_blob() -> TestResult {
    let temp = tempfile::tempdir()?;
    let wrong_root = temp.path().join("root-file");
    std::fs::write(&wrong_root, b"")?;
    assert!(matches!(
        LocalFilesystemBlobStore::new(wrong_root, 1024).await,
        Err(BlobStoreError::UnsafePath)
    ));
    let root = temp.path().join("blobs");
    let store = LocalFilesystemBlobStore::new(root.clone(), 1024).await?;
    let key = key()?;
    std::fs::create_dir(root.join(key.as_str()))?;
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::UnsafePath)
    ));
    assert!(matches!(
        store.delete(&key).await,
        Err(BlobStoreError::UnsafePath)
    ));
    Ok(())
}

#[tokio::test]
async fn streaming_roundtrip_head_and_idempotent_delete() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 1024).await?;
    let key = key()?;
    let chunks: BlobSource = Box::pin(stream::iter([
        Ok(Bytes::from_static(b"hello ")),
        Ok(Bytes::from_static(b"world")),
    ]));
    let stored = store.put(&key, chunks, checksum(b"hello world")).await?;
    assert_eq!(stored.metadata.size_bytes, 11);
    assert_eq!(store.head(&key).await?, stored.metadata);
    let mut downloaded = store.get(&key).await?;
    assert_eq!(downloaded.metadata, stored.metadata);
    let mut bytes = Vec::new();
    while let Some(chunk) = downloaded.stream.next().await {
        bytes.extend_from_slice(&chunk?);
    }
    assert_eq!(bytes, b"hello world");
    store.delete(&key).await?;
    store.delete(&key).await?;
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::NotFound)
    ));
    assert!(matches!(
        store.head(&key).await,
        Err(BlobStoreError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn checksum_mismatch_and_source_failure_never_publish() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 1024).await?;
    let key = key()?;
    assert!(matches!(
        store.put(&key, source(b"bad"), checksum(b"good")).await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    let failed: BlobSource = Box::pin(stream::iter([
        Ok(Bytes::from_static(b"partial")),
        Err(BlobStoreError::SourceRead),
    ]));
    assert!(matches!(
        store.put(&key, failed, checksum(b"partial")).await,
        Err(BlobStoreError::SourceRead)
    ));
    assert!(matches!(
        store.head(&key).await,
        Err(BlobStoreError::NotFound)
    ));
    assert_eq!(std::fs::read_dir(temp.path().join("blobs"))?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn actual_streamed_bytes_are_limited_including_multiple_chunks() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 4).await?;
    let chunks: BlobSource = Box::pin(stream::iter([
        Ok(Bytes::from_static(b"123")),
        Ok(Bytes::from_static(b"45")),
    ]));
    assert!(matches!(
        store.put(&key()?, chunks, checksum(b"12345")).await,
        Err(BlobStoreError::LimitExceeded { limit: 4 })
    ));
    assert_eq!(std::fs::read_dir(temp.path().join("blobs"))?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn immutable_key_retries_are_idempotent_and_conflicts_preserve_original() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 1024).await?;
    let key = key()?;
    let original = store
        .put(&key, source(b"first"), checksum(b"first"))
        .await?;
    assert_eq!(
        store
            .put(&key, source(b"first"), checksum(b"first"))
            .await?,
        original
    );
    assert!(matches!(
        store.put(&key, source(b"other"), checksum(b"other")).await,
        Err(BlobStoreError::KeyConflict)
    ));
    assert_eq!(store.head(&key).await?, original.metadata);
    Ok(())
}

#[tokio::test]
async fn concurrent_same_content_publish_once_and_different_content_has_one_winner() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 1024).await?;
    let key = key()?;
    let (a, b) = tokio::join!(
        store.put(&key, source(b"same"), checksum(b"same")),
        store.put(&key, source(b"same"), checksum(b"same"))
    );
    assert_eq!(a?, b?);
    store.delete(&key).await?;
    let (a, b) = tokio::join!(
        store.put(&key, source(b"left"), checksum(b"left")),
        store.put(&key, source(b"right"), checksum(b"right"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(BlobStoreError::KeyConflict))
            || matches!(b, Err(BlobStoreError::KeyConflict))
    );
    assert_eq!(std::fs::read_dir(temp.path().join("blobs"))?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn cancellation_removes_temporary_file_and_does_not_expose_partial_blob() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 1024).await?;
    let key = key()?;
    let (started, received) = tokio::sync::oneshot::channel();
    let pending: BlobSource = Box::pin(
        stream::once(async move {
            let _ = started.send(());
            Ok(Bytes::from_static(b"first"))
        })
        .chain(stream::pending()),
    );
    let mut put = Box::pin(store.put(&key, pending, checksum(b"first")));
    tokio::select! {
        result = &mut put => { result?; return Err("incomplete source unexpectedly completed".into()); }
        result = received => { result?; }
    }
    assert!(matches!(
        store.head(&key).await,
        Err(BlobStoreError::NotFound)
    ));
    drop(put);
    assert_eq!(std::fs::read_dir(temp.path().join("blobs"))?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn empty_blob_is_supported_and_in_place_corruption_is_not_hidden() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("blobs");
    let store = LocalFilesystemBlobStore::new(root.clone(), 1024).await?;
    let key = key()?;
    store.put(&key, source(b""), checksum(b"")).await?;
    assert_eq!(store.head(&key).await?.size_bytes, 0);
    assert!(store.get(&key).await?.stream.next().await.is_none());
    store.delete(&key).await?;
    store
        .put(&key, source(b"original"), checksum(b"original"))
        .await?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join(key.as_str()))?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::End(-1))?;
    std::io::Write::write_all(&mut file, b"!")?;
    drop(file);
    assert!(matches!(
        store.head(&key).await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn unknown_envelope_schema_is_rejected_without_returning_content() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("blobs");
    let store = LocalFilesystemBlobStore::new(root.clone(), 1024).await?;
    let key = key()?;
    store
        .put(&key, source(b"content"), checksum(b"content"))
        .await?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join(key.as_str()))?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(7))?;
    std::io::Write::write_all(&mut file, &[2])?;
    drop(file);
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::UnsupportedSchema)
    ));
    Ok(())
}

#[tokio::test]
async fn head_and_get_detect_content_corruption() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("blobs");
    let store = LocalFilesystemBlobStore::new(root.clone(), 1024).await?;
    let key = key()?;
    store
        .put(&key, source(b"original"), checksum(b"original"))
        .await?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(key.as_str()))?;
    std::io::Write::write_all(&mut file, b"corruption")?;
    drop(file);
    assert!(matches!(
        store.head(&key).await,
        Err(BlobStoreError::CorruptBlob)
    ));
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::CorruptBlob)
    ));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_root_and_blob_are_rejected() -> TestResult {
    let temp = tempfile::tempdir()?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&outside, &link)?;
    assert!(matches!(
        LocalFilesystemBlobStore::new(link, 1024).await,
        Err(BlobStoreError::UnsafePath)
    ));
    let root = temp.path().join("blobs");
    let store = LocalFilesystemBlobStore::new(root.clone(), 1024).await?;
    let key = key()?;
    std::fs::write(outside.join("secret"), b"secret")?;
    std::os::unix::fs::symlink(outside.join("secret"), root.join(key.as_str()))?;
    assert!(matches!(
        store.get(&key).await,
        Err(BlobStoreError::UnsafePath)
    ));
    assert!(matches!(
        store.delete(&key).await,
        Err(BlobStoreError::UnsafePath)
    ));
    Ok(())
}
