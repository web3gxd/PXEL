//! Application ports with production consumers. No database or transport DTOs.

use bytes::Bytes;
use futures_util::Stream;
use spreadsheet_domain::{BlobKey, BlobMetadata, Checksum, StoredBlob};
use std::{future::Future, pin::Pin};
use thiserror::Error;

pub type BlobStream = Pin<Box<dyn Stream<Item = Result<Bytes, BlobStoreError>> + Send>>;
pub type BlobSource = BlobStream;

pub struct BlobDownload {
    pub metadata: BlobMetadata,
    pub stream: BlobStream,
}

#[derive(Debug, Error)]
pub enum BlobStoreError {
    #[error("blob does not exist")]
    NotFound,
    #[error("blob body exceeds the configured limit of {limit} bytes")]
    LimitExceeded { limit: u64 },
    #[error("blob body does not match the expected checksum")]
    ChecksumMismatch,
    #[error("blob key already identifies different content")]
    KeyConflict,
    #[error("blob source failed")]
    SourceRead,
    #[error("blob envelope or content is corrupt")]
    CorruptBlob,
    #[error("blob envelope schema is not supported")]
    UnsupportedSchema,
    #[error("blob storage path is unsafe")]
    UnsafePath,
    #[error("blob storage I/O failed")]
    Io(#[from] std::io::Error),
    #[error("blob storage blocking task failed")]
    TaskFailed,
    #[error("blob storage configuration is invalid")]
    InvalidConfiguration,
    #[error("remote blob storage request failed")]
    Remote,
}

/// Immutable blobs. Successful put is atomic and verifies actual streamed bytes.
/// Reusing a key for identical content is idempotent; different content conflicts.
/// A missing delete is successful. Sources/downloads never require whole-body buffers.
pub trait BlobStore: Send + Sync {
    fn put(
        &self,
        key: &BlobKey,
        source: BlobSource,
        expected_checksum: Checksum,
    ) -> impl Future<Output = Result<StoredBlob, BlobStoreError>> + Send;

    fn get(
        &self,
        key: &BlobKey,
    ) -> impl Future<Output = Result<BlobDownload, BlobStoreError>> + Send;

    fn head(
        &self,
        key: &BlobKey,
    ) -> impl Future<Output = Result<BlobMetadata, BlobStoreError>> + Send;

    fn delete(&self, key: &BlobKey) -> impl Future<Output = Result<(), BlobStoreError>> + Send;
}

pub mod journal;
