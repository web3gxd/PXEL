use super::{CryptoError, physical_key_id, random_bytes, reserve_nonce_with, wrapping_aad};
use crate::PgDatabase;
use spreadsheet_domain::workbook::{DocumentId, WorkspaceId};
use sqlx::{FromRow, Postgres, Transaction};
use std::{future::Future, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

const ROTATION_LOCK: i64 = 1_347_962_188;

#[derive(Debug, Eq, PartialEq)]
pub struct RotationReport {
    pub rewrapped: u64,
    pub verified: u64,
    pub active_kek_id: String,
}

#[derive(FromRow)]
struct RotationKey {
    document_id: Uuid,
    workspace_id: Uuid,
    dek_generation: i32,
    physical_key_id: Vec<u8>,
    kek_id: String,
    wrapped_dek: Vec<u8>,
    wrapping_nonce: Vec<u8>,
    wrap_schema_version: i32,
    wrap_revision: i64,
}

impl RotationKey {
    fn aad(&self) -> Result<Vec<u8>, CryptoError> {
        if self.wrap_schema_version != 1 {
            return Err(CryptoError::UnsupportedVersion);
        }
        let workspace = WorkspaceId::from_bytes(*self.workspace_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let document = DocumentId::from_bytes(*self.document_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let generation = u32::try_from(self.dek_generation)
            .ok()
            .filter(|value| *value != 0)
            .ok_or(CryptoError::InvalidEnvelope)?;
        let physical_id = self
            .physical_key_id
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        if self.wrap_revision <= 0 {
            return Err(CryptoError::InvalidEnvelope);
        }
        Ok(wrapping_aad(workspace, document, generation, physical_id))
    }

    fn material(&self, db: &PgDatabase) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
        let nonce = self
            .wrapping_nonce
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        let material = db
            .keys
            .unwrap_key(&self.kek_id, nonce, &self.aad()?, &self.wrapped_dek)?;
        if physical_key_id(&material).as_slice() != self.physical_key_id {
            return Err(CryptoError::AuthenticationFailed);
        }
        Ok(material)
    }
}

// SQLx rolls a dropped transaction back, including when its connection returns
// to the pool. A transaction advisory lock therefore cannot leak into a pooled
// session after a cancelled rotation. Keep its inherited idle timeout active.
pub(super) async fn keep_rotation_alive<T>(
    lock: &mut Transaction<'_, Postgres>,
    work: impl Future<Output = Result<T, CryptoError>>,
) -> Result<T, CryptoError> {
    tokio::pin!(work);
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = timer.tick() => {
                sqlx::query("SELECT 1").execute(&mut **lock).await?;
            }
        }
    }
}

impl PgDatabase {
    /// Rewraps every retained DEK generation without changing data ciphertext.
    /// Each verified wrapper commits independently; retry resumes those commits.
    /// Writers resume with the new durable KEK only after all wrappers validate.
    pub async fn rotate_keys(&self, to: &str) -> Result<RotationReport, CryptoError> {
        self.rotate_keys_inner(
            to,
            #[cfg(test)]
            None,
        )
        .await
    }

    async fn rotate_keys_inner(
        &self,
        to: &str,
        #[cfg(test)] interrupt_after: Option<u64>,
    ) -> Result<RotationReport, CryptoError> {
        let target_physical_id = self.keys.physical_key_id(to)?;
        let mut lock = self.pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit = on")
            .execute(&mut *lock)
            .await?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
            .bind(ROTATION_LOCK)
            .fetch_one(&mut *lock)
            .await?;
        if !acquired {
            lock.rollback().await?;
            return Err(CryptoError::KeyConflict);
        }
        self.check_write_lineage().await?;
        let result = self
            .rotate_keys_locked(
                &mut lock,
                to,
                &target_physical_id,
                #[cfg(test)]
                interrupt_after,
            )
            .await;
        match result {
            Ok(report) => {
                lock.commit().await?;
                Ok(report)
            }
            Err(error) => {
                lock.rollback().await?;
                Err(error)
            }
        }
    }

    async fn rotate_keys_locked(
        &self,
        lock: &mut Transaction<'_, Postgres>,
        to: &str,
        target_physical_id: &[u8; 32],
        #[cfg(test)] interrupt_after: Option<u64>,
    ) -> Result<RotationReport, CryptoError> {
        let mut cursor_document = None::<Uuid>;
        let mut cursor_generation = None::<i32>;
        let mut report = RotationReport {
            rewrapped: 0,
            verified: 0,
            active_kek_id: to.to_owned(),
        };
        loop {
            let row = sqlx::query_as::<_, RotationKey>(
                "SELECT document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version,wrap_revision FROM document_crypto_keys WHERE ($1::uuid IS NULL OR (document_id,dek_generation)>($1,$2)) ORDER BY document_id,dek_generation LIMIT 1",
            )
            .bind(cursor_document)
            .bind(cursor_generation)
            .fetch_optional(&mut **lock)
            .await?;
            let Some(row) = row else {
                break;
            };
            row.material(self)?;
            if row.kek_id != to {
                keep_rotation_alive(lock, self.rewrap_key(&row, to, target_physical_id)).await?;
                let persisted = sqlx::query_as::<_, RotationKey>(
                    "SELECT document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version,wrap_revision FROM document_crypto_keys WHERE document_id=$1 AND dek_generation=$2",
                )
                .bind(row.document_id)
                .bind(row.dek_generation)
                .fetch_one(&mut **lock)
                .await?;
                if persisted.kek_id != to
                    || persisted.wrap_revision
                        != row
                            .wrap_revision
                            .checked_add(1)
                            .ok_or(CryptoError::KeyConflict)?
                {
                    return Err(CryptoError::KeyConflict);
                }
                persisted.material(self)?;
                report.rewrapped = report
                    .rewrapped
                    .checked_add(1)
                    .ok_or(CryptoError::KeyConflict)?;
                #[cfg(test)]
                if interrupt_after == Some(report.rewrapped) {
                    return Err(CryptoError::KeyConflict);
                }
            }
            report.verified = report
                .verified
                .checked_add(1)
                .ok_or(CryptoError::KeyConflict)?;
            cursor_document = Some(row.document_id);
            cursor_generation = Some(row.dek_generation);
        }
        let remaining = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM document_crypto_keys WHERE kek_id<>$1)",
        )
        .bind(to)
        .fetch_one(&mut **lock)
        .await?;
        if remaining {
            return Err(CryptoError::KeyConflict);
        }
        sqlx::query(
            "INSERT INTO crypto_key_state(singleton,active_kek_id,rotated_at) VALUES(true,$1,clock_timestamp()) ON CONFLICT(singleton) DO UPDATE SET active_kek_id=EXCLUDED.active_kek_id,rotated_at=EXCLUDED.rotated_at WHERE crypto_key_state.active_kek_id<>EXCLUDED.active_kek_id",
        ).bind(to).execute(&mut **lock).await?;
        Ok(report)
    }

    async fn rewrap_key(
        &self,
        row: &RotationKey,
        to: &str,
        target_physical_id: &[u8; 32],
    ) -> Result<(), CryptoError> {
        let material = row.material(self)?;
        let aad = row.aad()?;
        let nonce =
            reserve_nonce_with(&self.crypto_pool, target_physical_id, random_bytes::<24>).await?;
        let wrapped = self.keys.wrap_key(to, &nonce, &aad, &material)?;
        let verified = self.keys.unwrap_key(to, &nonce, &aad, &wrapped)?;
        if *verified != *material {
            return Err(CryptoError::AuthenticationFailed);
        }
        let mut tx = self.crypto_pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await?;
        let changed = sqlx::query(
            "UPDATE document_crypto_keys SET kek_id=$3,wrapped_dek=$4,wrapping_nonce=$5,wrap_revision=wrap_revision+1 WHERE document_id=$1 AND dek_generation=$2 AND wrap_revision=$6",
        )
        .bind(row.document_id)
        .bind(row.dek_generation)
        .bind(to)
        .bind(&wrapped)
        .bind(nonce.as_slice())
        .bind(row.wrap_revision)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(CryptoError::KeyConflict);
        }
        let persisted = sqlx::query_as::<_, RotationKey>(
            "SELECT document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version,wrap_revision FROM document_crypto_keys WHERE document_id=$1 AND dek_generation=$2",
        )
        .bind(row.document_id)
        .bind(row.dek_generation)
        .fetch_one(&mut *tx)
        .await?;
        if persisted.kek_id != to || *persisted.material(self)? != *material {
            return Err(CryptoError::AuthenticationFailed);
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalKeyProvider;
    use spreadsheet_application::journal::{AuthenticatedActor, OperationJournal};
    use spreadsheet_application::keys::KeyProviderError;
    use spreadsheet_domain::workbook::{
        ActorId, ColumnId, CommittedEvent, DocumentVersion, EventPrincipal, Operation, OperationId,
        RowId, SheetId, SheetSeed, Workbook,
    };
    use std::sync::Arc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type JournalBytes = (Uuid, i64, Vec<u8>, Vec<u8>, Vec<u8>);

    fn provider() -> Result<LocalKeyProvider, KeyProviderError> {
        LocalKeyProvider::from_keyring_json(
            r#"{"schema_version":1,"active_kek_id":"old","keys":[{"id":"old","key_hex":"0101010101010101010101010101010101010101010101010101010101010101"},{"id":"new","key_hex":"0202020202020202020202020202020202020202020202020202020202020202"}]}"#,
        )
    }

    async fn database(pool: sqlx::PgPool) -> Result<PgDatabase, Box<dyn std::error::Error>> {
        let mut db = PgDatabase::for_test(pool)?;
        db.keys = Arc::new(provider()?);
        db.migrate().await?;
        Ok(db)
    }

    async fn documents(
        db: &PgDatabase,
    ) -> Result<(AuthenticatedActor, [DocumentId; 2]), Box<dyn std::error::Error>> {
        let identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes([1; 16])?,
            workspace_id: WorkspaceId::from_bytes([2; 16])?,
        };
        db.issue_development_session([9; 32], identity.actor_id, identity.workspace_id)
            .await?;
        let documents = [
            DocumentId::from_bytes([3; 16])?,
            DocumentId::from_bytes([4; 16])?,
        ];
        for (index, document) in documents.into_iter().enumerate() {
            let seed = u8::try_from(index)?;
            let operation = Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: SheetId::from_bytes([20 + seed; 16])?,
                    name: "Rotation fixture".into(),
                    rows: vec![RowId::from_bytes([30 + seed; 16])?],
                    columns: vec![ColumnId::from_bytes([40 + seed; 16])?],
                },
            };
            let workbook =
                Workbook::empty(document).candidate(&operation, DocumentVersion::new(1)?)?;
            db.create(
                identity,
                CommittedEvent {
                    document_id: document,
                    operation_id: OperationId::from_bytes([50 + seed; 16])?,
                    principal: EventPrincipal::User(identity.actor_id),
                    version: workbook.version(),
                    operation,
                    state_hash: workbook.hash(),
                },
            )
            .await?;
        }
        add_retained_generation(db, identity.workspace_id, documents[0]).await?;
        Ok((identity, documents))
    }

    async fn add_retained_generation(
        db: &PgDatabase,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> TestResult {
        let material = Zeroizing::new(random_bytes::<32>()?);
        let physical_id = physical_key_id(&material);
        let nonce = reserve_nonce_with(
            &db.crypto_pool,
            &db.keys.physical_key_id("old")?,
            random_bytes::<24>,
        )
        .await?;
        let wrapped = db.keys.wrap_key(
            "old",
            &nonce,
            &wrapping_aad(workspace, document, 2, &physical_id),
            &material,
        )?;
        sqlx::query("INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,active,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,2,$3,false,'old',$4,$5,1)")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .bind(physical_id.as_slice())
            .bind(wrapped)
            .bind(nonce.as_slice())
            .execute(&db.pool)
            .await?;
        Ok(())
    }

    async fn journal_bytes(db: &PgDatabase) -> Result<Vec<JournalBytes>, sqlx::Error> {
        sqlx::query_as("SELECT o.document_id,d.head_version,o.payload,o.command_fingerprint,o.canonical_event_hash FROM document_operations o JOIN documents d ON d.document_id=o.document_id ORDER BY o.document_id,o.committed_version")
            .fetch_all(&db.pool).await
    }

    async fn active_kek(db: &PgDatabase) -> Result<String, sqlx::Error> {
        sqlx::query_scalar("SELECT active_kek_id FROM crypto_key_state WHERE singleton")
            .fetch_one(&db.pool)
            .await
    }

    #[sqlx::test(migrations = false)]
    async fn rotation_keeps_journal_bytes_and_rewraps_retained_generations(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let mut db = database(pool).await?;
        let (identity, documents) = documents(&db).await?;
        let before = journal_bytes(&db).await?;
        let retained = db.document_key(documents[0], Some(2)).await?;
        let report = db.rotate_keys("new").await?;
        assert_eq!(
            report,
            RotationReport {
                rewrapped: 3,
                verified: 3,
                active_kek_id: "new".into()
            }
        );
        assert!(before == journal_bytes(&db).await?);
        assert_eq!(active_kek(&db).await?, "new");
        assert_eq!(db.keys.active_kek_id(), "old");
        let another = DocumentId::from_bytes([5; 16])?;
        db.ensure_document_key(identity.workspace_id, another)
            .await?;
        let new_document_kek: String =
            sqlx::query_scalar("SELECT kek_id FROM document_crypto_keys WHERE document_id=$1")
                .bind(Uuid::from_bytes(*another.as_bytes()))
                .fetch_one(&db.pool)
                .await?;
        assert_eq!(new_document_kek, "new");
        db.keys = Arc::new(LocalKeyProvider::from_keyring_json(
            r#"{"schema_version":1,"active_kek_id":"new","keys":[{"id":"new","key_hex":"0202020202020202020202020202020202020202020202020202020202020202"}]}"#,
        )?);
        db.encryption_readiness().await?;
        let recovered_retained = db.document_key(documents[0], Some(2)).await?;
        assert!(*recovered_retained.material == *retained.material);
        for document in documents {
            let events = db
                .load_events(identity, document, DocumentVersion::ZERO, 10)
                .await?;
            assert_eq!(events.len(), 1);
            let mut workbook = Workbook::empty(document);
            workbook.apply_event(&events[0])?;
            assert_eq!(workbook.hash(), events[0].state_hash);
        }
        assert_eq!(db.rotate_keys("new").await?.rewrapped, 0);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn interrupted_rotation_resumes_committed_wrappers_before_switching_active_kek(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        documents(&db).await?;
        let before = journal_bytes(&db).await?;
        assert!(matches!(
            db.rotate_keys_inner("new", Some(1)).await,
            Err(CryptoError::KeyConflict)
        ));
        assert_eq!(active_kek(&db).await?, "old");
        let committed: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_crypto_keys WHERE kek_id='new'")
                .fetch_one(&db.pool)
                .await?;
        assert_eq!(committed, 1);
        let report = db.rotate_keys("new").await?;
        assert_eq!(report.rewrapped, 2);
        assert_eq!(report.verified, 3);
        assert_eq!(active_kek(&db).await?, "new");
        assert!(before == journal_bytes(&db).await?);
        let revisions: Vec<i64> = sqlx::query_scalar(
            "SELECT wrap_revision FROM document_crypto_keys ORDER BY document_id,dek_generation",
        )
        .fetch_all(&db.pool)
        .await?;
        assert_eq!(revisions, vec![2, 2, 2]);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn missing_target_key_changes_neither_wrappers_nor_active_state(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        documents(&db).await?;
        let before = journal_bytes(&db).await?;
        let nonces: i64 = sqlx::query_scalar("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        assert!(matches!(
            db.rotate_keys("missing").await,
            Err(CryptoError::KeyProvider(KeyProviderError::KeyUnavailable))
        ));
        assert_eq!(active_kek(&db).await?, "old");
        assert!(before == journal_bytes(&db).await?);
        let changed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_crypto_keys WHERE kek_id<>'old' OR wrap_revision<>1)")
            .fetch_one(&db.pool).await?;
        assert!(!changed);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?,
            nonces
        );
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn rotation_refuses_an_active_writer_and_releases_lock_after_failure(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        documents(&db).await?;
        let mut writer = db.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
            .bind(ROTATION_LOCK)
            .execute(&mut *writer)
            .await?;
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), db.rotate_keys("new")).await?,
            Err(CryptoError::KeyConflict)
        ));
        writer.rollback().await?;
        assert_eq!(db.rotate_keys("new").await?.rewrapped, 3);
        Ok(())
    }
}
