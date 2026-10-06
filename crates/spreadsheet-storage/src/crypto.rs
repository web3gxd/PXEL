//! Independently versioned authenticated storage envelope and key adapters.

mod backup;
mod local_keys;
pub use backup::{
    BackupError, BackupReport, MAX_BACKUP_MANIFEST_BYTES, decrypt_backup, verify_backup,
};
mod rotation;
pub use rotation::RotationReport;
mod restore;
pub use restore::RestoreReport;
#[cfg(feature = "legacy-migration")]
mod legacy_migration;
#[cfg(feature = "legacy-migration")]
pub use legacy_migration::{LegacyMigrationError, LegacyMigrationReport};
#[cfg(test)]
mod tests;

pub use local_keys::LocalKeyProvider;

use crate::PgDatabase;
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload},
};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use spreadsheet_application::keys::KeyProviderError;
use spreadsheet_domain::workbook::{DocumentId, WorkspaceId};
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"PXELENC\0";
const ENVELOPE_VERSION: u16 = 1;
const ALGORITHM: u16 = 1;
const HEADER_BYTES: usize = 40;
const NONCE_ATTEMPTS: usize = 8;
pub const CRYPTO_ENVELOPE_OVERHEAD: usize = HEADER_BYTES + 16;
pub const ENVELOPE_OVERHEAD: usize = CRYPTO_ENVELOPE_OVERHEAD;

pub fn ciphertext_limit(plaintext_limit: usize) -> Result<usize, CryptoError> {
    plaintext_limit
        .checked_add(CRYPTO_ENVELOPE_OVERHEAD)
        .ok_or(CryptoError::LimitExceeded {
            limit: plaintext_limit,
        })
}

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption context is invalid")]
    InvalidContext,
    #[error("encrypted storage envelope is malformed")]
    InvalidEnvelope,
    #[error("encrypted storage envelope version is unsupported")]
    UnsupportedVersion,
    #[error("encrypted storage authentication failed")]
    AuthenticationFailed,
    #[error("plaintext exceeds the configured limit of {limit} bytes")]
    LimitExceeded { limit: usize },
    #[error("document encryption key is unavailable")]
    MissingDocumentKey,
    #[error("document encryption key context conflicts")]
    KeyConflict,
    #[error("restored storage write generations require activation")]
    RestorePending,
    #[error("unique encryption nonce allocation failed")]
    NonceExhausted,
    #[error("key provider failed")]
    KeyProvider(#[from] KeyProviderError),
    #[error("encryption storage query failed")]
    Database(#[from] sqlx::Error),
}

#[derive(FromRow)]
struct KeyRecord {
    document_id: Uuid,
    workspace_id: Uuid,
    dek_generation: i32,
    physical_key_id: Vec<u8>,
    kek_id: String,
    wrapped_dek: Vec<u8>,
    wrapping_nonce: Vec<u8>,
    wrap_schema_version: i32,
    durable_workspace_id: Option<Uuid>,
}

struct DocumentKey {
    workspace: WorkspaceId,
    generation: u32,
    physical_id: [u8; 32],
    material: Zeroizing<[u8; 32]>,
}

/// Short-lived cache scoped to a single authenticated history read. DEKs remain
/// zeroizing values and are dropped when the scan returns.
#[derive(Default)]
pub(crate) struct ReadKeyCache {
    keys: HashMap<u32, DocumentKey>,
}

/// Stable identity for equal AEAD key bytes, independent of configured aliases.
pub(super) fn physical_key_id(material: &[u8; 32]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"PXEL physical AEAD key v1\0");
    digest.update(material);
    digest.finalize().into()
}

fn random_bytes<const N: usize>() -> Result<[u8; N], KeyProviderError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| KeyProviderError::RandomnessUnavailable)?;
    Ok(bytes)
}

fn random_key() -> Result<Zeroizing<[u8; 32]>, KeyProviderError> {
    let mut material = Zeroizing::new([0; 32]);
    getrandom::fill(&mut *material).map_err(|_| KeyProviderError::RandomnessUnavailable)?;
    Ok(material)
}

fn wrapping_aad(
    workspace: WorkspaceId,
    document: DocumentId,
    generation: u32,
    physical_id: &[u8; 32],
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(96);
    aad.extend_from_slice(b"PXEL DEK wrap v1\0");
    aad.extend_from_slice(workspace.as_bytes());
    aad.extend_from_slice(document.as_bytes());
    aad.extend_from_slice(&generation.to_be_bytes());
    aad.extend_from_slice(physical_id);
    aad
}

fn record_aad(
    key: &DocumentKey,
    document: DocumentId,
    version: u64,
    purpose: &str,
    object_id: Uuid,
    codec_version: u32,
) -> Result<Vec<u8>, CryptoError> {
    if version == 0
        || version > i64::MAX as u64
        || codec_version == 0
        || purpose.is_empty()
        || purpose.len() > 64
        || !purpose
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || object_id.is_nil()
    {
        return Err(CryptoError::InvalidContext);
    }
    let mut aad = Vec::with_capacity(128 + purpose.len());
    aad.extend_from_slice(b"PXEL encrypted record v1\0");
    aad.extend_from_slice(&ENVELOPE_VERSION.to_be_bytes());
    aad.extend_from_slice(&ALGORITHM.to_be_bytes());
    aad.extend_from_slice(key.workspace.as_bytes());
    aad.extend_from_slice(document.as_bytes());
    aad.extend_from_slice(&version.to_be_bytes());
    aad.push(u8::try_from(purpose.len()).map_err(|_| CryptoError::InvalidContext)?);
    aad.extend_from_slice(purpose.as_bytes());
    aad.extend_from_slice(object_id.as_bytes());
    aad.extend_from_slice(&key.generation.to_be_bytes());
    aad.extend_from_slice(&codec_version.to_be_bytes());
    Ok(aad)
}

async fn reserve_nonce_with(
    pool: &PgPool,
    physical_id: &[u8; 32],
    mut random: impl FnMut() -> Result<[u8; 24], KeyProviderError>,
) -> Result<[u8; 24], CryptoError> {
    for _ in 0..NONCE_ATTEMPTS {
        let nonce = random()?;
        // This transaction commits independently of any document write. A failed
        // or cancelled writer must never release an already consumed nonce.
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await?;
        let inserted = sqlx::query(
            "INSERT INTO crypto_nonce_reservations(physical_key_id,nonce) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(physical_id.as_slice())
        .bind(nonce.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        if inserted == 1 {
            return Ok(nonce);
        }
    }
    Err(CryptoError::NonceExhausted)
}

pub(crate) async fn keep_crypto_fence_alive<T>(
    fence: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    work: impl std::future::Future<Output = Result<T, CryptoError>>,
) -> Result<T, CryptoError> {
    rotation::keep_rotation_alive(fence, work).await
}

impl PgDatabase {
    /// Startup gate: plaintext datasets need an explicit offline migration.
    /// Every retained generation must be readable under the configured KEKs.
    pub async fn encryption_readiness(&self) -> Result<(), CryptoError> {
        self.check_write_lineage().await?;
        let active = sqlx::query_scalar::<_, String>(
            "SELECT active_kek_id FROM crypto_key_state WHERE singleton",
        )
        .fetch_optional(&self.crypto_pool)
        .await?;
        self.keys
            .physical_key_id(active.as_deref().unwrap_or(self.keys.active_kek_id()))?;
        let plaintext = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM document_operations WHERE NOT is_encrypted_storage_envelope(payload) OR NOT is_encrypted_storage_envelope(command_fingerprint) OR NOT is_encrypted_storage_envelope(canonical_event_hash)) OR EXISTS(SELECT 1 FROM document_snapshots WHERE NOT is_encrypted_storage_envelope(state_hash)) OR EXISTS(SELECT 1 FROM export_audit_events WHERE NOT is_encrypted_storage_envelope(payload_checksum) OR payload_size <> 0)",
        )
        .fetch_one(&self.crypto_pool)
        .await?;
        if plaintext {
            return Err(CryptoError::InvalidEnvelope);
        }
        let mut generations = sqlx::query_as::<_, KeyRecord>(
            "SELECT k.document_id,k.workspace_id,k.dek_generation,k.physical_key_id,k.kek_id,k.wrapped_dek,k.wrapping_nonce,k.wrap_schema_version,d.workspace_id AS durable_workspace_id FROM document_crypto_keys k LEFT JOIN documents d ON d.document_id=k.document_id ORDER BY k.document_id,k.dek_generation",
        )
        .fetch(&self.crypto_pool);
        while let Some(record) = generations.try_next().await? {
            self.decode_key_record(record)?;
        }
        Ok(())
    }

    /// Creates the durable DEK before document genesis. Races retain the winner;
    /// a reused document identity may never silently change its workspace.
    pub async fn ensure_document_key(
        &self,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<(), CryptoError> {
        self.check_write_lineage().await?;
        // The fence occupies the document pool, leaving both crypto connections
        // free for committed reservations while a rotation waits exclusively.
        let mut fence = self.begin_guarded().await?;
        keep_crypto_fence_alive(
            &mut fence,
            self.ensure_document_key_guarded(workspace, document),
        )
        .await?;
        fence.commit().await?;
        Ok(())
    }

    /// The caller holds a main-pool shared rotation fence for the complete
    /// transaction. Avoid taking a second document connection during genesis.
    pub(crate) async fn ensure_document_key_guarded(
        &self,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<(), CryptoError> {
        match self.document_key(document, None).await {
            Ok(key) if key.workspace == workspace => return Ok(()),
            Ok(_) => return Err(CryptoError::KeyConflict),
            Err(CryptoError::MissingDocumentKey) => {}
            Err(error) => return Err(error),
        }
        let existing_workspace = sqlx::query_scalar::<_, Uuid>(
            "SELECT workspace_id FROM documents WHERE document_id=$1",
        )
        .bind(Uuid::from_bytes(*document.as_bytes()))
        .fetch_optional(&self.crypto_pool)
        .await?;
        if existing_workspace
            .is_some_and(|current| current != Uuid::from_bytes(*workspace.as_bytes()))
        {
            return Err(CryptoError::KeyConflict);
        }
        let material = random_key()?;
        let physical_id = physical_key_id(&material);
        sqlx::query(
            "INSERT INTO crypto_key_state(singleton,active_kek_id) VALUES(true,$1) ON CONFLICT(singleton) DO NOTHING",
        )
        .bind(self.keys.active_kek_id())
        .execute(&self.crypto_pool)
        .await?;
        let kek_id = sqlx::query_scalar::<_, String>(
            "SELECT active_kek_id FROM crypto_key_state WHERE singleton=true",
        )
        .fetch_one(&self.crypto_pool)
        .await?;
        let nonce = reserve_nonce_with(
            &self.crypto_pool,
            &self.keys.physical_key_id(&kek_id)?,
            random_bytes::<24>,
        )
        .await?;
        let wrapped = self.keys.wrap_key(
            &kek_id,
            &nonce,
            &wrapping_aad(workspace, document, 1, &physical_id),
            &material,
        )?;
        let mut tx = self.crypto_pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await?;
        // First creation races on the generation PK and the partial active-key
        // index. Arbitrate both, then authenticate the durable winner below.
        sqlx::query(
            "INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,1,$3,$4,$5,$6,1) ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::from_bytes(*document.as_bytes()))
        .bind(Uuid::from_bytes(*workspace.as_bytes()))
        .bind(physical_id.as_slice())
        .bind(&kek_id)
        .bind(&wrapped)
        .bind(nonce.as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let key = self.document_key(document, None).await?;
        if key.workspace != workspace {
            return Err(CryptoError::KeyConflict);
        }
        Ok(())
    }

    async fn document_key(
        &self,
        document: DocumentId,
        generation: Option<u32>,
    ) -> Result<DocumentKey, CryptoError> {
        let generation = generation
            .map(i32::try_from)
            .transpose()
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        let record = sqlx::query_as::<_, KeyRecord>(
            "SELECT k.document_id,k.workspace_id,k.dek_generation,k.physical_key_id,k.kek_id,k.wrapped_dek,k.wrapping_nonce,k.wrap_schema_version,d.workspace_id AS durable_workspace_id FROM document_crypto_keys k LEFT JOIN documents d ON d.document_id=k.document_id WHERE k.document_id=$1 AND (($2::integer IS NULL AND k.active) OR k.dek_generation=$2) ORDER BY k.dek_generation DESC LIMIT 1",
        )
        .bind(Uuid::from_bytes(*document.as_bytes()))
        .bind(generation)
        .fetch_optional(&self.crypto_pool)
        .await?
        .ok_or(CryptoError::MissingDocumentKey)?;
        self.decode_key_record(record)
    }

    fn decode_key_record(&self, record: KeyRecord) -> Result<DocumentKey, CryptoError> {
        if record
            .durable_workspace_id
            .is_some_and(|workspace| workspace != record.workspace_id)
        {
            return Err(CryptoError::KeyConflict);
        }
        if record.wrap_schema_version != 1 {
            return Err(CryptoError::UnsupportedVersion);
        }
        let workspace = WorkspaceId::from_bytes(*record.workspace_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let document = DocumentId::from_bytes(*record.document_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let generation = u32::try_from(record.dek_generation)
            .ok()
            .filter(|value| *value != 0)
            .ok_or(CryptoError::InvalidEnvelope)?;
        let physical_id: [u8; 32] = record
            .physical_key_id
            .try_into()
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        let nonce: [u8; 24] = record
            .wrapping_nonce
            .try_into()
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        let material = self.keys.unwrap_key(
            &record.kek_id,
            &nonce,
            &wrapping_aad(workspace, document, generation, &physical_id),
            &record.wrapped_dek,
        )?;
        if physical_key_id(&material) != physical_id {
            return Err(CryptoError::AuthenticationFailed);
        }
        Ok(DocumentKey {
            workspace,
            generation,
            physical_id,
            material,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn seal_record(
        &self,
        document: DocumentId,
        version: u64,
        purpose: &str,
        object_id: Uuid,
        codec_version: u32,
        plaintext: &[u8],
        plaintext_limit: usize,
    ) -> Result<Vec<u8>, CryptoError> {
        self.check_write_lineage().await?;
        let mut fence = self.begin_guarded().await?;
        let result = keep_crypto_fence_alive(
            &mut fence,
            self.seal_record_guarded(
                document,
                version,
                purpose,
                object_id,
                codec_version,
                plaintext,
                plaintext_limit,
            ),
        )
        .await?;
        fence.commit().await?;
        Ok(result)
    }

    /// Internal writers retain their existing shared transaction fence; offline
    /// migration holds the exclusive fence while its activation barrier is pending.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn seal_record_guarded(
        &self,
        document: DocumentId,
        version: u64,
        purpose: &str,
        object_id: Uuid,
        codec_version: u32,
        plaintext: &[u8],
        plaintext_limit: usize,
    ) -> Result<Vec<u8>, CryptoError> {
        if plaintext.len() > plaintext_limit {
            return Err(CryptoError::LimitExceeded {
                limit: plaintext_limit,
            });
        }
        let key = self.document_key(document, None).await?;
        let aad = record_aad(&key, document, version, purpose, object_id, codec_version)?;
        let nonce =
            reserve_nonce_with(&self.crypto_pool, &key.physical_id, random_bytes::<24>).await?;
        seal_with(&key, &nonce, &aad, plaintext)
    }

    #[cfg(feature = "legacy-migration")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn seal_record_for_migration(
        &self,
        document: DocumentId,
        version: u64,
        purpose: &str,
        object_id: Uuid,
        codec_version: u32,
        plaintext: &[u8],
        plaintext_limit: usize,
    ) -> Result<Vec<u8>, CryptoError> {
        self.seal_record_guarded(
            document,
            version,
            purpose,
            object_id,
            codec_version,
            plaintext,
            plaintext_limit,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn open_record(
        &self,
        document: DocumentId,
        version: u64,
        purpose: &str,
        object_id: Uuid,
        codec_version: u32,
        envelope: &[u8],
        plaintext_limit: usize,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        self.open_record_cached(
            &mut ReadKeyCache::default(),
            document,
            version,
            purpose,
            object_id,
            codec_version,
            envelope,
            plaintext_limit,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn open_record_cached(
        &self,
        cache: &mut ReadKeyCache,
        document: DocumentId,
        version: u64,
        purpose: &str,
        object_id: Uuid,
        codec_version: u32,
        envelope: &[u8],
        plaintext_limit: usize,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let (generation, nonce) = parse_envelope(envelope, plaintext_limit)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.keys.entry(generation) {
            entry.insert(self.document_key(document, Some(generation)).await?);
        }
        let key = cache
            .keys
            .get(&generation)
            .ok_or(CryptoError::MissingDocumentKey)?;
        let aad = record_aad(key, document, version, purpose, object_id, codec_version)?;
        open_with(key, &nonce, &aad, envelope)
    }
}

fn parse_envelope(envelope: &[u8], plaintext_limit: usize) -> Result<(u32, [u8; 24]), CryptoError> {
    if envelope.len() < CRYPTO_ENVELOPE_OVERHEAD || envelope.get(..8) != Some(MAGIC.as_slice()) {
        return Err(CryptoError::InvalidEnvelope);
    }
    if envelope.len() - CRYPTO_ENVELOPE_OVERHEAD > plaintext_limit {
        return Err(CryptoError::LimitExceeded {
            limit: plaintext_limit,
        });
    }
    if envelope.get(8..10) != Some(ENVELOPE_VERSION.to_be_bytes().as_slice())
        || envelope.get(10..12) != Some(ALGORITHM.to_be_bytes().as_slice())
    {
        return Err(CryptoError::UnsupportedVersion);
    }
    let generation = u32::from_be_bytes(
        envelope
            .get(12..16)
            .ok_or(CryptoError::InvalidEnvelope)?
            .try_into()
            .map_err(|_| CryptoError::InvalidEnvelope)?,
    );
    if generation == 0 || generation > i32::MAX as u32 {
        return Err(CryptoError::InvalidEnvelope);
    }
    let nonce = envelope
        .get(16..HEADER_BYTES)
        .ok_or(CryptoError::InvalidEnvelope)?
        .try_into()
        .map_err(|_| CryptoError::InvalidEnvelope)?;
    Ok((generation, nonce))
}

fn seal_with(
    key: &DocumentKey,
    nonce: &[u8; 24],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new((&*key.material).into());
    let ciphertext = cipher
        .encrypt(
            nonce.into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::AuthenticationFailed)?;
    let mut envelope = Vec::with_capacity(ciphertext.len() + HEADER_BYTES);
    envelope.extend_from_slice(MAGIC);
    envelope.extend_from_slice(&ENVELOPE_VERSION.to_be_bytes());
    envelope.extend_from_slice(&ALGORITHM.to_be_bytes());
    envelope.extend_from_slice(&key.generation.to_be_bytes());
    envelope.extend_from_slice(nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn open_with(
    key: &DocumentKey,
    nonce: &[u8; 24],
    aad: &[u8],
    envelope: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let cipher = XChaCha20Poly1305::new((&*key.material).into());
    cipher
        .decrypt(
            nonce.into(),
            Payload {
                msg: envelope
                    .get(HEADER_BYTES..)
                    .ok_or(CryptoError::InvalidEnvelope)?,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::AuthenticationFailed)
}
