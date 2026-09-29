use aws_config::{BehaviorVersion, Region, timeout::TimeoutConfig};
use aws_sdk_s3::{Client, config::Builder, primitives::ByteStream};
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use spreadsheet_application::{BlobDownload, BlobSource, BlobStore, BlobStoreError};
use spreadsheet_domain::{BlobKey, BlobMetadata, Checksum, StoredBlob};
use std::time::Duration;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};
use url::Url;

#[derive(Clone)]
pub struct S3BlobStore {
    client: Client,
    bucket: String,
    max_blob_bytes: u64,
}

impl S3BlobStore {
    /// Uses the AWS SDK credential chain; no credentials are persisted here.
    /// Custom endpoints are path-style and must use TLS outside loopback.
    pub async fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        max_blob_bytes: u64,
    ) -> Result<Self, BlobStoreError> {
        validate_configuration(endpoint, bucket, region, max_blob_bytes)?;
        let shared = aws_config::defaults(BehaviorVersion::v2026_01_12())
            .region(Region::new(region.to_owned()))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(30))
                    .operation_attempt_timeout(Duration::from_secs(10))
                    .build(),
            )
            .load()
            .await;
        let config = Builder::from(&shared)
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(config),
            bucket: bucket.to_owned(),
            max_blob_bytes,
        })
    }

    pub async fn readiness(&self) -> Result<(), BlobStoreError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|_| BlobStoreError::Remote)?;
        Ok(())
    }

    async fn verified_download(
        &self,
        key: &BlobKey,
    ) -> Result<(tempfile::NamedTempFile, BlobMetadata), BlobStoreError> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| {
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404)
                {
                    BlobStoreError::NotFound
                } else {
                    BlobStoreError::Remote
                }
            })?;
        let declared = output
            .content_length()
            .and_then(|value| u64::try_from(value).ok());
        if declared.is_some_and(|length| length > self.max_blob_bytes) {
            return Err(BlobStoreError::LimitExceeded {
                limit: self.max_blob_bytes,
            });
        }
        let declared_checksum = output
            .metadata()
            .and_then(|metadata| metadata.get("pxel-sha256"))
            .ok_or(BlobStoreError::CorruptBlob)?
            .to_owned();
        let temporary = tempfile::NamedTempFile::new()?;
        let mut file = File::from_std(temporary.reopen()?);
        let mut source = output.body.into_async_read();
        let mut buffer = [0_u8; 64 * 1024];
        let mut hash = Sha256::new();
        let mut length = 0_u64;
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|_| BlobStoreError::Remote)?;
                if read == 0 {
                    break;
                }
                length = length
                    .checked_add(read as u64)
                    .filter(|size| *size <= self.max_blob_bytes)
                    .ok_or(BlobStoreError::LimitExceeded {
                        limit: self.max_blob_bytes,
                    })?;
                hash.update(&buffer[..read]);
                file.write_all(&buffer[..read]).await?;
            }
            Ok::<(), BlobStoreError>(())
        })
        .await
        .map_err(|_| BlobStoreError::Remote)??;
        if declared.is_some_and(|expected| expected != length) {
            return Err(BlobStoreError::CorruptBlob);
        }
        let checksum = Checksum::from_bytes(hash.finalize().into());
        if checksum_hex(checksum) != declared_checksum {
            return Err(BlobStoreError::ChecksumMismatch);
        }
        file.flush().await?;
        Ok((
            temporary,
            BlobMetadata {
                size_bytes: length,
                checksum,
            },
        ))
    }
}

fn checksum_hex(checksum: Checksum) -> String {
    let mut output = String::with_capacity(64);
    for byte in checksum.as_bytes() {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

impl BlobStore for S3BlobStore {
    async fn put(
        &self,
        key: &BlobKey,
        mut source: BlobSource,
        expected_checksum: Checksum,
    ) -> Result<StoredBlob, BlobStoreError> {
        let temporary = tempfile::NamedTempFile::new()?;
        let mut file = File::from_std(temporary.reopen()?);
        let mut hash = Sha256::new();
        let mut length = 0_u64;
        while let Some(chunk) = source.next().await {
            let chunk = chunk?;
            length = length
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
        file.flush().await?;
        drop(file);
        let metadata = BlobMetadata {
            size_bytes: length,
            checksum: expected_checksum,
        };
        for attempt in 0..3_u32 {
            let body = ByteStream::from_path(temporary.path()).await.map_err(|_| {
                BlobStoreError::Io(std::io::Error::other("cannot read staged blob"))
            })?;
            let result = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(key.as_str())
                .body(body)
                .metadata("pxel-sha256", checksum_hex(expected_checksum))
                .if_none_match("*")
                .send()
                .await;
            match result {
                Ok(_) => {
                    return Ok(StoredBlob {
                        key: key.clone(),
                        metadata,
                    });
                }
                Err(error)
                    if error
                        .raw_response()
                        .is_some_and(|response| response.status().as_u16() == 412) =>
                {
                    let existing = self.verified_download(key).await?.1;
                    return if existing == metadata {
                        Ok(StoredBlob {
                            key: key.clone(),
                            metadata,
                        })
                    } else {
                        Err(BlobStoreError::KeyConflict)
                    };
                }
                Err(error)
                    if attempt < 2
                        && error
                            .raw_response()
                            .is_some_and(|response| response.status().as_u16() == 409) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(25 * (attempt + 1) as u64))
                        .await;
                }
                Err(_) => return Err(BlobStoreError::Remote),
            }
        }
        Err(BlobStoreError::Remote)
    }

    async fn get(&self, key: &BlobKey) -> Result<BlobDownload, BlobStoreError> {
        let (temporary, metadata) = self.verified_download(key).await?;
        let file = File::from_std(temporary.reopen()?);
        let body = stream::try_unfold(
            (temporary, file, metadata.size_bytes),
            |(temporary, mut file, remaining)| async move {
                if remaining == 0 {
                    return Ok(None);
                }
                let mut buffer = vec![0_u8; remaining.min(64 * 1024) as usize];
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    return Err(BlobStoreError::CorruptBlob);
                }
                buffer.truncate(read);
                Ok(Some((
                    Bytes::from(buffer),
                    (temporary, file, remaining - read as u64),
                )))
            },
        );
        Ok(BlobDownload {
            metadata,
            stream: Box::pin(body),
        })
    }

    async fn head(&self, key: &BlobKey) -> Result<BlobMetadata, BlobStoreError> {
        Ok(self.verified_download(key).await?.1)
    }

    async fn delete(&self, key: &BlobKey) -> Result<(), BlobStoreError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .send()
            .await
            .map_err(|_| BlobStoreError::Remote)?;
        Ok(())
    }
}

fn validate_configuration(
    endpoint: &str,
    bucket: &str,
    region: &str,
    max_blob_bytes: u64,
) -> Result<(), BlobStoreError> {
    let url = Url::parse(endpoint).map_err(|_| BlobStoreError::InvalidConfiguration)?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !(3..=63).contains(&bucket.len())
        || !bucket.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
        || bucket.starts_with(['-', '.'])
        || bucket.ends_with(['-', '.'])
        || bucket.contains("..")
        || region.is_empty()
        || region.len() > 64
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || max_blob_bytes == 0
    {
        return Err(BlobStoreError::InvalidConfiguration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_configuration;

    #[test]
    fn rejects_insecure_or_ambiguous_s3_configuration() {
        assert!(
            validate_configuration("http://127.0.0.1:3900/", "pxel-test", "us-east-1", 1024)
                .is_ok()
        );
        for endpoint in [
            "http://storage.example.com/",
            "https://user:pass@storage.example.com/",
            "https://storage.example.com/path",
            "https://storage.example.com/?x=1",
        ] {
            assert!(validate_configuration(endpoint, "pxel-test", "us-east-1", 1024).is_err());
        }
        assert!(
            validate_configuration("https://storage.example.com/", "../bad", "us-east-1", 1024)
                .is_err()
        );
        assert!(
            validate_configuration("https://storage.example.com/", "pxel-test", "", 1024).is_err()
        );
        assert!(
            validate_configuration("https://storage.example.com/", "pxel-test", "us-east-1", 0)
                .is_err()
        );
    }
}
