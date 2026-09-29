use crate::LocalFilesystemBlobStore;
#[cfg(feature = "s3")]
use crate::S3BlobStore;
use spreadsheet_application::{BlobDownload, BlobSource, BlobStore, BlobStoreError};
use spreadsheet_domain::{BlobKey, BlobMetadata, Checksum, StoredBlob};

#[derive(Clone)]
pub enum ConfiguredBlobStore {
    Local(LocalFilesystemBlobStore),
    #[cfg(feature = "s3")]
    S3(S3BlobStore),
}

impl ConfiguredBlobStore {
    pub async fn readiness(&self) -> Result<(), BlobStoreError> {
        match self {
            Self::Local(store) => store.readiness().await,
            #[cfg(feature = "s3")]
            Self::S3(store) => store.readiness().await,
        }
    }
}

impl BlobStore for ConfiguredBlobStore {
    async fn put(
        &self,
        key: &BlobKey,
        source: BlobSource,
        expected_checksum: Checksum,
    ) -> Result<StoredBlob, BlobStoreError> {
        match self {
            Self::Local(store) => store.put(key, source, expected_checksum).await,
            #[cfg(feature = "s3")]
            Self::S3(store) => store.put(key, source, expected_checksum).await,
        }
    }

    async fn get(&self, key: &BlobKey) -> Result<BlobDownload, BlobStoreError> {
        match self {
            Self::Local(store) => store.get(key).await,
            #[cfg(feature = "s3")]
            Self::S3(store) => store.get(key).await,
        }
    }

    async fn head(&self, key: &BlobKey) -> Result<BlobMetadata, BlobStoreError> {
        match self {
            Self::Local(store) => store.head(key).await,
            #[cfg(feature = "s3")]
            Self::S3(store) => store.head(key).await,
        }
    }

    async fn delete(&self, key: &BlobKey) -> Result<(), BlobStoreError> {
        match self {
            Self::Local(store) => store.delete(key).await,
            #[cfg(feature = "s3")]
            Self::S3(store) => store.delete(key).await,
        }
    }
}
