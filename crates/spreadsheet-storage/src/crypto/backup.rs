use super::{CryptoError, physical_key_id, random_bytes, random_key, reserve_nonce_with};
use crate::PgDatabase;
use aead_stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::XChaCha20Poly1305;
use spreadsheet_application::keys::KeyProvider;
use std::io;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"PXELBKP\0";
const VERSION: u16 = 1;
const ALGORITHM: u16 = 1;
const HEADER_PREFIX_BYTES: usize = 18;
const HEADER_TAIL_BYTES: usize = 24 + 19 + 48;
const DATA_BYTES: usize = 64 * 1024;
const TAG_BYTES: usize = 16;
const MANIFEST: u8 = 0;
const DATA: u8 = 1;
const FINAL: u8 = 2;
pub const MAX_BACKUP_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup encryption failed")]
    Crypto(#[from] CryptoError),
    #[error("backup input or output failed")]
    Io(#[from] io::Error),
    #[error("backup archive is malformed")]
    InvalidArchive,
    #[error("backup archive version is unsupported")]
    UnsupportedVersion,
    #[error("backup manifest is empty or exceeds its limit")]
    InvalidManifest,
    #[error("backup stream exceeds its limit")]
    StreamLimitExceeded,
    #[error("backup archive is truncated")]
    Truncated,
    #[error("backup archive contains trailing data")]
    TrailingData,
    #[error("backup authentication failed")]
    AuthenticationFailed,
}

impl From<sqlx::Error> for BackupError {
    fn from(error: sqlx::Error) -> Self {
        Self::Crypto(CryptoError::Database(error))
    }
}

// No Debug implementation: the manifest may describe private document objects.
pub struct BackupReport {
    pub manifest: Zeroizing<Vec<u8>>,
    pub payload_bytes: u64,
    pub data_chunks: u32,
}

struct Header {
    bytes: Vec<u8>,
    kek_id: String,
    wrapping_nonce: [u8; 24],
    stream_nonce: [u8; 19],
    wrapped_dek: [u8; 48],
}

fn valid_key_id(id: &[u8]) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn wrap_aad(prefix: &[u8]) -> Vec<u8> {
    let mut aad = b"PXEL backup key wrap v1\0".to_vec();
    aad.extend_from_slice(prefix);
    aad
}

fn frame_prefix(kind: u8, len: usize) -> Result<[u8; 5], BackupError> {
    let mut prefix = [0; 5];
    prefix[0] = kind;
    prefix[1..].copy_from_slice(
        &u32::try_from(len)
            .map_err(|_| BackupError::StreamLimitExceeded)?
            .to_be_bytes(),
    );
    Ok(prefix)
}

fn frame_aad(header: &[u8], prefix: &[u8; 5]) -> Vec<u8> {
    let mut aad = header.to_vec();
    aad.extend_from_slice(prefix);
    aad
}

fn plaintext_buffer(bytes: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut buffer = Zeroizing::new(Vec::with_capacity(bytes.len() + TAG_BYTES));
    buffer.extend_from_slice(bytes);
    buffer
}

async fn exact<R: AsyncRead + Unpin>(input: &mut R, output: &mut [u8]) -> Result<(), BackupError> {
    input.read_exact(output).await.map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            BackupError::Truncated
        } else {
            BackupError::Io(error)
        }
    })?;
    Ok(())
}

async fn read_header<R: AsyncRead + Unpin>(input: &mut R) -> Result<Header, BackupError> {
    let mut prefix = [0; HEADER_PREFIX_BYTES];
    exact(input, &mut prefix).await?;
    if prefix[..8] != MAGIC[..] {
        return Err(BackupError::InvalidArchive);
    }
    if prefix[8..10] != VERSION.to_be_bytes() || prefix[10..12] != ALGORITHM.to_be_bytes() {
        return Err(BackupError::UnsupportedVersion);
    }
    if prefix[12..16] != (DATA_BYTES as u32).to_be_bytes() {
        return Err(BackupError::InvalidArchive);
    }
    let id_len = usize::from(u16::from_be_bytes([prefix[16], prefix[17]]));
    if id_len == 0 || id_len > 64 {
        return Err(BackupError::InvalidArchive);
    }
    let mut id = vec![0; id_len];
    exact(input, &mut id).await?;
    if !valid_key_id(&id) {
        return Err(BackupError::InvalidArchive);
    }
    let kek_id = String::from_utf8(id.clone()).map_err(|_| BackupError::InvalidArchive)?;
    let mut tail = [0; HEADER_TAIL_BYTES];
    exact(input, &mut tail).await?;
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&id);
    bytes.extend_from_slice(&tail);
    Ok(Header {
        bytes,
        kek_id,
        wrapping_nonce: tail[..24]
            .try_into()
            .map_err(|_| BackupError::InvalidArchive)?,
        stream_nonce: tail[24..43]
            .try_into()
            .map_err(|_| BackupError::InvalidArchive)?,
        wrapped_dek: tail[43..]
            .try_into()
            .map_err(|_| BackupError::InvalidArchive)?,
    })
}

async fn read_frame<R: AsyncRead + Unpin>(
    input: &mut R,
) -> Result<([u8; 5], Zeroizing<Vec<u8>>), BackupError> {
    let mut prefix = [0; 5];
    exact(input, &mut prefix).await?;
    let len = usize::try_from(u32::from_be_bytes(
        prefix[1..]
            .try_into()
            .map_err(|_| BackupError::InvalidArchive)?,
    ))
    .map_err(|_| BackupError::InvalidArchive)?;
    let valid = match prefix[0] {
        MANIFEST => (TAG_BYTES + 1..=MAX_BACKUP_MANIFEST_BYTES + TAG_BYTES).contains(&len),
        DATA => (TAG_BYTES + 1..=DATA_BYTES + TAG_BYTES).contains(&len),
        FINAL => len == TAG_BYTES,
        _ => false,
    };
    if !valid {
        return Err(BackupError::InvalidArchive);
    }
    let mut body = Zeroizing::new(vec![0; len]);
    exact(input, &mut body).await?;
    Ok((prefix, body))
}

async fn write_frame<W: AsyncWrite + Unpin>(
    output: &mut W,
    prefix: &[u8; 5],
    body: &[u8],
) -> Result<(), BackupError> {
    output.write_all(prefix).await?;
    output.write_all(body).await?;
    Ok(())
}

async fn fill_data<R: AsyncRead + Unpin>(
    input: &mut R,
    buffer: &mut [u8],
) -> Result<usize, BackupError> {
    let mut len = 0;
    while len < buffer.len() {
        match input.read(&mut buffer[len..]).await {
            Ok(0) => break,
            Ok(read) => len += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(BackupError::Io(error)),
        }
    }
    Ok(len)
}

impl PgDatabase {
    /// Encrypts an opaque bounded manifest followed by a streaming backup payload.
    /// The caller coordinates a consistent PostgreSQL/blob bundle and publishes
    /// the ciphertext file only after this succeeds and file durability passes.
    pub async fn encrypt_backup<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &self,
        manifest: &[u8],
        input: &mut R,
        output: &mut W,
    ) -> Result<BackupReport, BackupError> {
        self.check_write_lineage().await?;
        self.with_shared_fence(Box::pin(
            self.encrypt_backup_guarded(manifest, input, output),
        ))
        .await
    }

    pub(crate) async fn encrypt_backup_guarded<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &self,
        manifest: &[u8],
        input: &mut R,
        output: &mut W,
    ) -> Result<BackupReport, BackupError> {
        self.check_write_lineage().await?;
        if manifest.is_empty() || manifest.len() > MAX_BACKUP_MANIFEST_BYTES {
            return Err(BackupError::InvalidManifest);
        }
        let configured = sqlx::query_scalar::<_, String>(
            "SELECT active_kek_id FROM crypto_key_state WHERE singleton",
        )
        .fetch_optional(&self.crypto_pool)
        .await
        .map_err(CryptoError::from)?;
        let kek_id = match configured {
            Some(id) => id,
            None => {
                let has_keys: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_crypto_keys)")
                        .fetch_one(&self.crypto_pool)
                        .await
                        .map_err(CryptoError::from)?;
                if has_keys {
                    return Err(CryptoError::KeyConflict.into());
                }
                self.keys.active_kek_id().to_owned()
            }
        };
        if !valid_key_id(kek_id.as_bytes()) {
            return Err(BackupError::InvalidArchive);
        }
        let target_physical = self
            .keys
            .physical_key_id(&kek_id)
            .map_err(CryptoError::from)?;
        let material = random_key().map_err(CryptoError::from)?;
        let physical_id = physical_key_id(&material);
        let wrapping_nonce =
            reserve_nonce_with(&self.crypto_pool, &target_physical, random_bytes::<24>).await?;
        let stream_nonce = random_bytes::<19>().map_err(CryptoError::from)?;
        let mut header = MAGIC.to_vec();
        header.extend_from_slice(&VERSION.to_be_bytes());
        header.extend_from_slice(&ALGORITHM.to_be_bytes());
        header.extend_from_slice(&(DATA_BYTES as u32).to_be_bytes());
        header.extend_from_slice(
            &u16::try_from(kek_id.len())
                .map_err(|_| BackupError::InvalidArchive)?
                .to_be_bytes(),
        );
        header.extend_from_slice(kek_id.as_bytes());
        header.extend_from_slice(&wrapping_nonce);
        header.extend_from_slice(&stream_nonce);
        let wrapping_context = wrap_aad(&header);
        let wrapped = self
            .keys
            .wrap_key(&kek_id, &wrapping_nonce, &wrapping_context, &material)
            .map_err(CryptoError::from)?;
        let verified = self
            .keys
            .unwrap_key(&kek_id, &wrapping_nonce, &wrapping_context, &wrapped)
            .map_err(CryptoError::from)?;
        if wrapped.len() != 48 || *verified != *material {
            return Err(BackupError::AuthenticationFailed);
        }
        header.extend_from_slice(&wrapped);
        let mut encryptor =
            EncryptorBE32::<XChaCha20Poly1305>::new((&*material).into(), (&stream_nonce).into());
        let mut encoded_manifest = plaintext_buffer(manifest);
        let prefix = frame_prefix(MANIFEST, encoded_manifest.len() + TAG_BYTES)?;
        self.reserve_backup_segment(&physical_id, &stream_nonce, 0, false)
            .await?;
        encryptor
            .encrypt_next_in_place(&frame_aad(&header, &prefix), &mut *encoded_manifest)
            .map_err(|_| BackupError::AuthenticationFailed)?;
        output.write_all(&header).await?;
        write_frame(output, &prefix, &encoded_manifest).await?;
        drop(encoded_manifest);
        let mut payload_bytes = 0_u64;
        let mut data_chunks = 0_u32;
        loop {
            let mut buffer = Zeroizing::new(Vec::with_capacity(DATA_BYTES + TAG_BYTES));
            buffer.resize(DATA_BYTES, 0);
            let len = fill_data(input, &mut buffer).await?;
            if len == 0 {
                break;
            }
            if data_chunks == u32::MAX - 1 {
                return Err(BackupError::StreamLimitExceeded);
            }
            buffer.truncate(len);
            let prefix = frame_prefix(DATA, len + TAG_BYTES)?;
            let counter = data_chunks
                .checked_add(1)
                .ok_or(BackupError::StreamLimitExceeded)?;
            self.reserve_backup_segment(&physical_id, &stream_nonce, counter, false)
                .await?;
            encryptor
                .encrypt_next_in_place(&frame_aad(&header, &prefix), &mut *buffer)
                .map_err(|_| BackupError::AuthenticationFailed)?;
            write_frame(output, &prefix, &buffer).await?;
            payload_bytes = payload_bytes
                .checked_add(len as u64)
                .ok_or(BackupError::StreamLimitExceeded)?;
            data_chunks = counter;
        }
        let final_counter = data_chunks
            .checked_add(1)
            .ok_or(BackupError::StreamLimitExceeded)?;
        self.reserve_backup_segment(&physical_id, &stream_nonce, final_counter, true)
            .await?;
        let prefix = frame_prefix(FINAL, TAG_BYTES)?;
        let mut final_body = plaintext_buffer(&[]);
        encryptor
            .encrypt_last_in_place(&frame_aad(&header, &prefix), &mut *final_body)
            .map_err(|_| BackupError::AuthenticationFailed)?;
        write_frame(output, &prefix, &final_body).await?;
        output.flush().await?;
        Ok(BackupReport {
            manifest: Zeroizing::new(manifest.to_vec()),
            payload_bytes,
            data_chunks,
        })
    }

    async fn reserve_backup_segment(
        &self,
        physical_id: &[u8; 32],
        base_nonce: &[u8; 19],
        counter: u32,
        last: bool,
    ) -> Result<(), BackupError> {
        // STREAMBE32's exact AEAD nonce layout is defined by aead-stream. The
        // ledger records the nonce before the library performs any encryption.
        let mut nonce = [0; 24];
        nonce[..19].copy_from_slice(base_nonce);
        nonce[19..23].copy_from_slice(&counter.to_be_bytes());
        nonce[23] = u8::from(last);
        reserve_nonce_with(&self.crypto_pool, physical_id, || Ok(nonce)).await?;
        Ok(())
    }
}

/// Decrypts authenticated segments into an unpublished sink. Success requires
/// the terminal STREAM tag and exact EOF. A caller must discard partial output
/// on any error; restore should first verify the archive in a separate pass.
pub async fn decrypt_backup<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    keys: &dyn KeyProvider,
    input: &mut R,
    output: &mut W,
) -> Result<BackupReport, BackupError> {
    let header = read_header(input).await?;
    let material = keys
        .unwrap_key(
            &header.kek_id,
            &header.wrapping_nonce,
            &wrap_aad(&header.bytes[..header.bytes.len() - 48]),
            &header.wrapped_dek,
        )
        .map_err(CryptoError::from)?;
    let mut decryptor =
        DecryptorBE32::<XChaCha20Poly1305>::new((&*material).into(), (&header.stream_nonce).into());
    let (prefix, mut manifest) = read_frame(input).await?;
    if prefix[0] != MANIFEST {
        return Err(BackupError::InvalidArchive);
    }
    decryptor
        .decrypt_next_in_place(&frame_aad(&header.bytes, &prefix), &mut *manifest)
        .map_err(|_| BackupError::AuthenticationFailed)?;
    let mut payload_bytes = 0_u64;
    let mut data_chunks = 0_u32;
    loop {
        let (prefix, mut body) = read_frame(input).await?;
        match prefix[0] {
            DATA => {
                if data_chunks == u32::MAX - 1 {
                    return Err(BackupError::StreamLimitExceeded);
                }
                decryptor
                    .decrypt_next_in_place(&frame_aad(&header.bytes, &prefix), &mut *body)
                    .map_err(|_| BackupError::AuthenticationFailed)?;
                payload_bytes = payload_bytes
                    .checked_add(body.len() as u64)
                    .ok_or(BackupError::StreamLimitExceeded)?;
                data_chunks = data_chunks
                    .checked_add(1)
                    .ok_or(BackupError::StreamLimitExceeded)?;
                output.write_all(&body).await?;
            }
            FINAL => {
                decryptor
                    .decrypt_last_in_place(&frame_aad(&header.bytes, &prefix), &mut *body)
                    .map_err(|_| BackupError::AuthenticationFailed)?;
                if !body.is_empty() {
                    return Err(BackupError::InvalidArchive);
                }
                let mut trailing = [0; 1];
                if input.read(&mut trailing).await? != 0 {
                    return Err(BackupError::TrailingData);
                }
                output.flush().await?;
                return Ok(BackupReport {
                    manifest,
                    payload_bytes,
                    data_chunks,
                });
            }
            _ => return Err(BackupError::InvalidArchive),
        }
    }
}

/// Verifies the whole archive without writing plaintext to storage.
pub async fn verify_backup<R: AsyncRead + Unpin>(
    keys: &dyn KeyProvider,
    input: &mut R,
) -> Result<BackupReport, BackupError> {
    decrypt_backup(keys, input, &mut tokio::io::sink()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalKeyProvider;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    async fn encrypt(
        db: &PgDatabase,
        manifest: &[u8],
        payload: &[u8],
    ) -> Result<Vec<u8>, BackupError> {
        let mut archive = Vec::new();
        let mut input = payload;
        db.encrypt_backup(manifest, &mut input, &mut archive)
            .await?;
        Ok(archive)
    }

    async fn frame_ranges(archive: &[u8]) -> Result<Vec<(usize, usize)>, BackupError> {
        let mut input = archive;
        let header = read_header(&mut input).await?;
        let mut cursor = header.bytes.len();
        let mut ranges = Vec::new();
        while cursor < archive.len() {
            let prefix = archive
                .get(cursor..cursor + 5)
                .ok_or(BackupError::Truncated)?;
            let len = usize::try_from(u32::from_be_bytes(
                prefix[1..]
                    .try_into()
                    .map_err(|_| BackupError::InvalidArchive)?,
            ))
            .map_err(|_| BackupError::InvalidArchive)?;
            let end = cursor
                .checked_add(5 + len)
                .ok_or(BackupError::InvalidArchive)?;
            if end > archive.len() {
                return Err(BackupError::Truncated);
            }
            ranges.push((cursor, end));
            cursor = end;
        }
        Ok(ranges)
    }

    #[sqlx::test(migrations = false)]
    async fn backup_round_trip_streams_boundaries_and_reserves_every_nonce(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let manifest = b"private authenticated backup manifest";
        for len in [0, 1, DATA_BYTES, DATA_BYTES + 1, DATA_BYTES * 3 + 7] {
            let payload = payload(len);
            let before: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?;
            let mut archive = Vec::new();
            let encoded = db
                .encrypt_backup(manifest, &mut payload.as_slice(), &mut archive)
                .await?;
            assert_eq!(encoded.payload_bytes, len as u64);
            assert_eq!(
                encoded.data_chunks,
                u32::try_from(len.div_ceil(DATA_BYTES))?
            );
            assert!(
                !archive
                    .windows(manifest.len())
                    .any(|window| window == manifest)
            );
            let report = verify_backup(db.keys.as_ref(), &mut archive.as_slice()).await?;
            assert_eq!(report.manifest.as_slice(), manifest);
            assert_eq!(report.payload_bytes, encoded.payload_bytes);
            assert_eq!(report.data_chunks, encoded.data_chunks);
            let mut restored = Zeroizing::new(Vec::new());
            let decrypted =
                decrypt_backup(db.keys.as_ref(), &mut archive.as_slice(), &mut *restored).await?;
            assert!(restored.as_slice() == payload.as_slice());
            assert_eq!(decrypted.payload_bytes, len as u64);
            let header = read_header(&mut archive.as_slice()).await?;
            let key = db.keys.unwrap_key(
                &header.kek_id,
                &header.wrapping_nonce,
                &wrap_aad(&header.bytes[..header.bytes.len() - 48]),
                &header.wrapped_dek,
            )?;
            let physical_id = physical_key_id(&key);
            let nonces: Vec<Vec<u8>> = sqlx::query_scalar("SELECT nonce FROM crypto_nonce_reservations WHERE physical_key_id=$1 ORDER BY nonce")
                .bind(physical_id.as_slice()).fetch_all(&db.pool).await?;
            assert_eq!(nonces.len(), usize::try_from(encoded.data_chunks)? + 2);
            for (index, nonce) in nonces.iter().enumerate() {
                assert_eq!(&nonce[..19], &header.stream_nonce);
                assert_eq!(&nonce[19..23], &u32::try_from(index)?.to_be_bytes());
                assert_eq!(nonce[23], u8::from(index == nonces.len() - 1));
            }
            let after: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?;
            assert_eq!(after - before, i64::from(encoded.data_chunks) + 3);
        }
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn backup_rejects_tampering_reorder_truncation_trailing_data_and_wrong_keys(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let archive = encrypt(&db, b"authenticated manifest", &payload(DATA_BYTES * 2 + 7)).await?;
        let header = read_header(&mut archive.as_slice()).await?;
        let frames = frame_ranges(&archive).await?;
        assert_eq!(frames.len(), 5);
        let mut rejected = Vec::new();
        for index in [
            8,
            10,
            12,
            16,
            18,
            header.bytes.len() - 49,
            header.bytes.len() - 1,
            frames[0].0 + 5,
            frames[1].0 + 5,
            archive.len() - 1,
        ] {
            let mut tampered = archive.clone();
            tampered[index] ^= 1;
            rejected.push(tampered);
        }
        let mut wrong_length = archive.clone();
        wrong_length[frames[0].0 + 1..frames[0].0 + 5].copy_from_slice(&u32::MAX.to_be_bytes());
        rejected.push(wrong_length);
        let mut wrong_kind = archive.clone();
        wrong_kind[frames[0].0] = DATA;
        rejected.push(wrong_kind);
        let mut reordered = archive.clone();
        let first_len = frames[1].1 - frames[1].0;
        assert_eq!(first_len, frames[2].1 - frames[2].0);
        for index in 0..first_len {
            reordered.swap(frames[1].0 + index, frames[2].0 + index);
        }
        rejected.push(reordered);
        let mut missing_middle = archive.clone();
        missing_middle.drain(frames[2].0..frames[2].1);
        rejected.push(missing_middle);
        for end in [
            0,
            HEADER_PREFIX_BYTES - 1,
            header.bytes.len() - 1,
            frames[0].1 - 1,
            frames[1].1 - 1,
            frames[4].0,
            archive.len() - 1,
        ] {
            rejected.push(archive[..end].to_vec());
        }
        for corrupted in rejected {
            assert!(
                verify_backup(db.keys.as_ref(), &mut corrupted.as_slice())
                    .await
                    .is_err()
            );
        }
        let mut trailing = archive.clone();
        trailing.push(0);
        assert!(matches!(
            verify_backup(db.keys.as_ref(), &mut trailing.as_slice()).await,
            Err(BackupError::TrailingData)
        ));
        let wrong_keys = LocalKeyProvider::for_test()?;
        assert!(
            verify_backup(&wrong_keys, &mut archive.as_slice())
                .await
                .is_err()
        );
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn backup_manifest_limit_is_checked_before_any_nonce_is_reserved(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let manifest = Zeroizing::new(vec![b'x'; MAX_BACKUP_MANIFEST_BYTES]);
        let archive = encrypt(&db, &manifest, &[]).await?;
        let verified = verify_backup(db.keys.as_ref(), &mut archive.as_slice()).await?;
        assert_eq!(verified.manifest.len(), MAX_BACKUP_MANIFEST_BYTES);
        assert!(verified.manifest.as_slice() == manifest.as_slice());
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        for invalid in [Vec::new(), vec![b'x'; MAX_BACKUP_MANIFEST_BYTES + 1]] {
            assert!(matches!(
                encrypt(&db, &invalid, &[]).await,
                Err(BackupError::InvalidManifest)
            ));
        }
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(before, after);
        Ok(())
    }

    struct FailWriter;

    impl AsyncWrite for FailWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::other("synthetic output failure")))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[sqlx::test(migrations = false)]
    async fn failed_backup_keeps_consumed_nonce_reservations(pool: sqlx::PgPool) -> TestResult {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let manifest = b"failure fixture";
        assert!(matches!(
            db.encrypt_backup(manifest, &mut b"input".as_slice(), &mut FailWriter)
                .await,
            Err(BackupError::Io(_))
        ));
        let reserved: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(reserved, 2);
        let archive = encrypt(&db, manifest, b"input").await?;
        verify_backup(db.keys.as_ref(), &mut archive.as_slice()).await?;
        let after_retry: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(after_retry, reserved + 4);
        Ok(())
    }
}
