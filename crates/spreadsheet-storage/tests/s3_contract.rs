#![cfg(feature = "s3")]

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{Client, config::Builder, primitives::ByteStream};
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use spreadsheet_application::{BlobSource, BlobStore, BlobStoreError};
use spreadsheet_domain::{BlobKey, Checksum};
use spreadsheet_storage::S3BlobStore;
use std::error::Error;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

fn checksum(bytes: &[u8]) -> Checksum {
    Checksum::from_bytes(Sha256::digest(bytes).into())
}

fn source(bytes: &'static [u8]) -> BlobSource {
    Box::pin(stream::once(async move { Ok(Bytes::from_static(bytes)) }))
}

fn key() -> Result<BlobKey, Box<dyn Error>> {
    Ok(BlobKey::parse(&Uuid::now_v7().to_string())?)
}

async fn make_store(limit: u64) -> Result<S3BlobStore, Box<dyn Error>> {
    let endpoint = std::env::var("PXEL_S3_TEST_ENDPOINT")?;
    let bucket = std::env::var("PXEL_S3_TEST_BUCKET")?;
    Ok(S3BlobStore::new(&endpoint, &bucket, "us-east-1", limit).await?)
}

async fn raw_client() -> Result<(Client, String), Box<dyn Error>> {
    let endpoint = std::env::var("PXEL_S3_TEST_ENDPOINT")?;
    let bucket = std::env::var("PXEL_S3_TEST_BUCKET")?;
    let shared = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let client = Client::from_conf(
        Builder::from(&shared)
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build(),
    );
    Ok((client, bucket))
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_constructor_and_missing_object() -> TestResult {
    let store = make_store(1024).await?;
    store.readiness().await?;
    assert!(matches!(
        store.head(&key()?).await,
        Err(BlobStoreError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_single_put_and_delete() -> TestResult {
    let store = make_store(1024).await?;
    let object = key()?;
    Box::pin(store.put(&object, source(b"one"), checksum(b"one"))).await?;
    store.delete(&object).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_large_stream_roundtrip() -> TestResult {
    Box::pin(s3_large_stream_scenario()).await
}

async fn s3_large_stream_scenario() -> TestResult {
    let store = make_store(200_000).await?;
    let object = key()?;
    assert!(matches!(
        store.get(&object).await,
        Err(BlobStoreError::NotFound)
    ));
    assert!(matches!(
        store.head(&object).await,
        Err(BlobStoreError::NotFound)
    ));
    let content = vec![37_u8; 150_000];
    let expected = checksum(&content);
    let staged: BlobSource = Box::pin(stream::iter([
        Ok(Bytes::from(content[..70_000].to_vec())),
        Ok(Bytes::from(content[70_000..].to_vec())),
    ]));
    let receipt = store.put(&object, staged, expected).await?;
    assert_eq!(receipt.metadata.size_bytes, 150_000);
    assert_eq!(store.head(&object).await?, receipt.metadata);
    let mut downloaded = store.get(&object).await?;
    assert_eq!(downloaded.metadata, receipt.metadata);
    let mut actual = Vec::new();
    while let Some(chunk) = downloaded.stream.next().await {
        let chunk = chunk?;
        assert!(chunk.len() <= 64 * 1024);
        actual.extend_from_slice(&chunk);
    }
    assert_eq!(actual, content);
    store.delete(&object).await?;
    store.delete(&object).await?;
    assert!(matches!(
        store.get(&object).await,
        Err(BlobStoreError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_idempotent_retry_and_conflict() -> TestResult {
    let store = make_store(1024).await?;
    let object = key()?;
    let first = store
        .put(&object, source(b"first"), checksum(b"first"))
        .await?;
    assert_eq!(
        store
            .put(&object, source(b"first"), checksum(b"first"))
            .await?,
        first
    );
    assert!(matches!(
        store
            .put(&object, source(b"other"), checksum(b"other"))
            .await,
        Err(BlobStoreError::KeyConflict)
    ));
    assert_eq!(store.head(&object).await?, first.metadata);
    store.delete(&object).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_rejects_bad_source_without_publication() -> TestResult {
    let store = make_store(1024).await?;
    let bad = key()?;
    assert!(matches!(
        store
            .put(&bad, source(b"wrong"), checksum(b"expected"))
            .await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    assert!(matches!(
        store.head(&bad).await,
        Err(BlobStoreError::NotFound)
    ));
    let small = make_store(4).await?;
    let too_large: BlobSource = Box::pin(stream::iter([
        Ok(Bytes::from_static(b"123")),
        Ok(Bytes::from_static(b"45")),
    ]));
    assert!(matches!(
        small.put(&bad, too_large, checksum(b"12345")).await,
        Err(BlobStoreError::LimitExceeded { limit: 4 })
    ));
    assert!(matches!(
        store.head(&bad).await,
        Err(BlobStoreError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_rejects_object_without_checksum_metadata() -> TestResult {
    let store = make_store(1024).await?;
    let (client, bucket) = raw_client().await?;
    let object = key()?;
    client
        .put_object()
        .bucket(&bucket)
        .key(object.as_str())
        .body(ByteStream::from_static(b"foreign"))
        .send()
        .await?;
    assert!(matches!(
        store.get(&object).await,
        Err(BlobStoreError::CorruptBlob)
    ));
    store.delete(&object).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_rejects_body_changed_after_metadata_was_written() -> TestResult {
    let store = make_store(1024).await?;
    let (client, bucket) = raw_client().await?;
    let object = key()?;
    client
        .put_object()
        .bucket(&bucket)
        .key(object.as_str())
        .body(ByteStream::from_static(b"tampered"))
        .metadata(
            "pxel-sha256",
            "0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5",
        )
        .send()
        .await?;
    assert!(matches!(
        store.head(&object).await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    assert!(matches!(
        store.get(&object).await,
        Err(BlobStoreError::ChecksumMismatch)
    ));
    store.delete(&object).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PXEL_S3_TEST_ENDPOINT, PXEL_S3_TEST_BUCKET and credentials"]
async fn s3_conditional_writes_have_one_winner() -> TestResult {
    Box::pin(s3_conditional_writes_scenario()).await
}

async fn s3_conditional_writes_scenario() -> TestResult {
    let store = make_store(1024).await?;
    let object = key()?;
    let (left, right) = tokio::join!(
        store.put(&object, source(b"left"), checksum(b"left")),
        store.put(&object, source(b"right"), checksum(b"right")),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(
        matches!(left, Err(BlobStoreError::KeyConflict))
            || matches!(right, Err(BlobStoreError::KeyConflict))
    );
    store.delete(&object).await?;
    Ok(())
}
