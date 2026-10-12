use super::{
    CryptoError, keep_crypto_fence_alive, physical_key_id, random_bytes, random_key,
    reserve_nonce_with, wrapping_aad,
};
use crate::PgDatabase;
use futures_util::TryStreamExt;
use spreadsheet_domain::workbook::{DocumentId, WorkspaceId};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

const RESTORE_LOCK: i64 = 1_347_962_188;

#[derive(Debug, Eq, PartialEq)]
pub struct RestoreReport {
    pub run_id: Uuid,
    pub created_generations: u64,
    pub verified_generations: u64,
    pub active_kek_id: String,
}

#[derive(FromRow)]
struct RestoreRun {
    run_id: Uuid,
    target_kek_id: String,
    target_physical_key_id: Vec<u8>,
    state: String,
}

#[derive(FromRow)]
struct RestoreDocument {
    document_id: Uuid,
    workspace_id: Uuid,
    source_max_generation: i32,
    new_generation: i32,
}

#[derive(FromRow)]
struct SourceDocument {
    document_id: Uuid,
    workspace_id: Uuid,
    head_version: i64,
    source_max_generation: i32,
}

const SOURCE_DOCUMENTS: &str = "SELECT d.document_id,d.workspace_id,d.head_version,COALESCE((SELECT max(k.dek_generation) FROM document_crypto_keys k WHERE k.document_id=d.document_id),0) AS source_max_generation FROM documents d UNION ALL SELECT k.document_id,k.workspace_id,0::bigint AS head_version,max(k.dek_generation) AS source_max_generation FROM document_crypto_keys k WHERE NOT EXISTS(SELECT 1 FROM documents d WHERE d.document_id=k.document_id) GROUP BY k.document_id,k.workspace_id ORDER BY document_id";

impl PgDatabase {
    pub(crate) async fn check_write_lineage(&self) -> Result<(), CryptoError> {
        let pending = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM crypto_restore_state s JOIN crypto_restore_runs r ON r.run_id=s.current_run_id WHERE r.state='PENDING') OR EXISTS(SELECT 1 FROM storage_migration_state WHERE state='PENDING')",
        )
        .fetch_one(&self.crypto_pool)
        .await?;
        if pending {
            Err(CryptoError::RestorePending)
        } else {
            Ok(())
        }
    }

    /// Only an unpublished, restored cluster may enter this maintenance flow.
    /// Every write generation is replaced; existing ciphertext stays readable.
    pub async fn renew_write_generations_after_restore(
        &self,
        to: &str,
    ) -> Result<RestoreReport, CryptoError> {
        self.renew_write_generations_inner(
            to,
            #[cfg(test)]
            None,
        )
        .await
    }

    async fn renew_write_generations_inner(
        &self,
        to: &str,
        #[cfg(test)] interrupt_after: Option<u64>,
    ) -> Result<RestoreReport, CryptoError> {
        let target = self.keys.physical_key_id(to)?;
        let mut fence = self.restore_fence().await?;
        let (run, initialized) = self.restore_run(&mut fence, to, &target).await?;
        if initialized {
            // The marker becomes durable in the transaction that owns the
            // exclusive lock. Writers can enter only after its commit and
            // will then observe PENDING before the next fence is acquired.
            fence.commit().await?;
            fence = self.restore_fence().await?;
            let (current, initialized_again) = self.restore_run(&mut fence, to, &target).await?;
            if initialized_again || current != run {
                return Err(CryptoError::KeyConflict);
            }
        }
        let report = keep_crypto_fence_alive(
            &mut fence,
            self.renew_write_generations_locked(
                run,
                to,
                &target,
                #[cfg(test)]
                interrupt_after,
            ),
        )
        .await?;
        fence.commit().await?;
        Ok(report)
    }

    async fn restore_fence(&self) -> Result<Transaction<'_, Postgres>, CryptoError> {
        let mut fence = self.pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *fence)
            .await?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
            .bind(RESTORE_LOCK)
            .fetch_one(&mut *fence)
            .await?;
        if !acquired {
            return Err(CryptoError::KeyConflict);
        }
        let migration_pending = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM storage_migration_state WHERE state='PENDING')",
        )
        .fetch_one(&mut *fence)
        .await?;
        if migration_pending {
            return Err(CryptoError::RestorePending);
        }
        Ok(fence)
    }

    async fn restore_run(
        &self,
        fence: &mut Transaction<'_, Postgres>,
        to: &str,
        target: &[u8; 32],
    ) -> Result<(Uuid, bool), CryptoError> {
        let current = sqlx::query_as::<_, RestoreRun>(
            "SELECT r.run_id,r.target_kek_id,r.target_physical_key_id,r.state FROM crypto_restore_state s JOIN crypto_restore_runs r ON r.run_id=s.current_run_id WHERE s.singleton=true",
        )
        .fetch_optional(&mut **fence)
        .await?;
        if let Some(current) = current
            && current.state == "PENDING"
        {
            if current.target_kek_id != to
                || current.target_physical_key_id.as_slice() != target.as_slice()
            {
                return Err(CryptoError::KeyConflict);
            }
            return Ok((current.run_id, false));
        }
        let used = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM document_crypto_keys WHERE physical_key_id=$1) OR EXISTS(SELECT 1 FROM crypto_nonce_reservations WHERE physical_key_id=$1) OR EXISTS(SELECT 1 FROM crypto_restore_runs WHERE target_physical_key_id=$1)",
        )
        .bind(target.as_slice())
        .fetch_one(&mut **fence)
        .await?;
        if used {
            return Err(CryptoError::KeyConflict);
        }
        let old_ids = sqlx::query_scalar::<_, String>(
            "SELECT kek_id FROM document_crypto_keys UNION SELECT COALESCE((SELECT active_kek_id FROM crypto_key_state WHERE singleton=true),$1)",
        )
        .bind(self.keys.active_kek_id())
        .fetch_all(&mut **fence)
        .await?;
        for id in old_ids {
            if self.keys.physical_key_id(&id)? == *target {
                return Err(CryptoError::KeyConflict);
            }
        }
        let run = Uuid::now_v7();
        sqlx::query("INSERT INTO crypto_restore_runs(run_id,target_kek_id,target_physical_key_id) VALUES($1,$2,$3)")
            .bind(run).bind(to).bind(target.as_slice()).execute(&mut **fence).await?;
        sqlx::query("INSERT INTO crypto_restore_state(singleton,current_run_id) VALUES(true,$1) ON CONFLICT(singleton) DO UPDATE SET current_run_id=EXCLUDED.current_run_id")
            .bind(run).execute(&mut **fence).await?;
        Ok((run, true))
    }

    async fn plan_restore_documents(&self, run: Uuid) -> Result<(), CryptoError> {
        let planned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM crypto_restore_documents WHERE run_id=$1)",
        )
        .bind(run)
        .fetch_one(&self.crypto_pool)
        .await?;
        if planned {
            return Ok(());
        }
        let conflict = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM document_crypto_keys k JOIN documents d ON d.document_id=k.document_id WHERE k.workspace_id<>d.workspace_id) OR EXISTS(SELECT 1 FROM document_crypto_keys GROUP BY document_id HAVING count(DISTINCT workspace_id)<>1 OR count(*) FILTER(WHERE active)<>1)",
        )
        .fetch_one(&self.crypto_pool)
        .await?;
        if conflict {
            return Err(CryptoError::KeyConflict);
        }
        let mut tx = self.crypto_pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let mut sources =
            sqlx::query_as::<_, SourceDocument>(SOURCE_DOCUMENTS).fetch(&self.crypto_pool);
        while let Some(source) = sources.try_next().await? {
            if source.source_max_generation == 0 && source.head_version > 0 {
                return Err(CryptoError::MissingDocumentKey);
            }
            let generation = source
                .source_max_generation
                .checked_add(1)
                .filter(|value| *value > 0)
                .ok_or(CryptoError::KeyConflict)?;
            sqlx::query("INSERT INTO crypto_restore_documents(run_id,document_id,workspace_id,source_max_generation,new_generation) VALUES($1,$2,$3,$4,$5)")
                .bind(run).bind(source.document_id).bind(source.workspace_id).bind(source.source_max_generation).bind(generation).execute(&mut *tx).await?;
        }
        drop(sources);
        tx.commit().await?;
        Ok(())
    }

    async fn fresh_restore_dek(
        &self,
        target: &[u8; 32],
    ) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
        for _ in 0..8 {
            let material = random_key()?;
            let physical = physical_key_id(&material);
            if physical == *target {
                continue;
            }
            let used = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM document_crypto_keys WHERE physical_key_id=$1) OR EXISTS(SELECT 1 FROM crypto_nonce_reservations WHERE physical_key_id=$1)",
            )
            .bind(physical.as_slice())
            .fetch_one(&self.crypto_pool)
            .await?;
            if !used {
                return Ok(material);
            }
        }
        Err(CryptoError::KeyConflict)
    }

    async fn install_restore_generation(
        &self,
        run: Uuid,
        row: &RestoreDocument,
        to: &str,
        target: &[u8; 32],
    ) -> Result<(), CryptoError> {
        let document = DocumentId::from_bytes(*row.document_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let workspace = WorkspaceId::from_bytes(*row.workspace_id.as_bytes())
            .map_err(|_| CryptoError::InvalidContext)?;
        let generation =
            u32::try_from(row.new_generation).map_err(|_| CryptoError::InvalidEnvelope)?;
        let material = self.fresh_restore_dek(target).await?;
        let physical = physical_key_id(&material);
        let nonce = reserve_nonce_with(&self.crypto_pool, target, random_bytes::<24>).await?;
        let aad = wrapping_aad(workspace, document, generation, &physical);
        let wrapped = self.keys.wrap_key(to, &nonce, &aad, &material)?;
        if *self.keys.unwrap_key(to, &nonce, &aad, &wrapped)? != *material {
            return Err(CryptoError::AuthenticationFailed);
        }
        let mut tx = self.crypto_pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let current = sqlx::query_scalar::<_, i32>(
            "SELECT COALESCE(max(dek_generation),0) FROM document_crypto_keys WHERE document_id=$1",
        )
        .bind(row.document_id)
        .fetch_one(&mut *tx)
        .await?;
        if current != row.source_max_generation {
            return Err(CryptoError::KeyConflict);
        }
        sqlx::query("UPDATE document_crypto_keys SET active=false WHERE document_id=$1 AND active")
            .bind(row.document_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,$3,$4,$5,$6,$7,1)")
            .bind(row.document_id).bind(row.workspace_id).bind(row.new_generation).bind(physical.as_slice()).bind(to).bind(&wrapped).bind(nonce.as_slice()).execute(&mut *tx).await?;
        let changed = sqlx::query("UPDATE crypto_restore_documents SET complete=true,completed_at=clock_timestamp() WHERE run_id=$1 AND document_id=$2 AND NOT complete AND source_max_generation=$3 AND new_generation=$4")
            .bind(run).bind(row.document_id).bind(row.source_max_generation).bind(row.new_generation).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(CryptoError::KeyConflict);
        }
        tx.commit().await?;
        let persisted = self.document_key(document, Some(generation)).await?;
        if persisted.workspace != workspace || *persisted.material != *material {
            return Err(CryptoError::AuthenticationFailed);
        }
        Ok(())
    }

    async fn renew_write_generations_locked(
        &self,
        run: Uuid,
        to: &str,
        target: &[u8; 32],
        #[cfg(test)] interrupt_after: Option<u64>,
    ) -> Result<RestoreReport, CryptoError> {
        self.plan_restore_documents(run).await?;
        #[cfg(test)]
        if interrupt_after == Some(0) {
            return Err(CryptoError::RestorePending);
        }
        let mut report = RestoreReport {
            run_id: run,
            created_generations: 0,
            verified_generations: 0,
            active_kek_id: to.to_owned(),
        };
        loop {
            let next = sqlx::query_as::<_, RestoreDocument>(
                "SELECT document_id,workspace_id,source_max_generation,new_generation FROM crypto_restore_documents WHERE run_id=$1 AND NOT complete ORDER BY document_id LIMIT 1",
            )
            .bind(run)
            .fetch_optional(&self.crypto_pool)
            .await?;
            let Some(row) = next else { break };
            self.install_restore_generation(run, &row, to, target)
                .await?;
            report.created_generations = report
                .created_generations
                .checked_add(1)
                .ok_or(CryptoError::KeyConflict)?;
            #[cfg(test)]
            if interrupt_after == Some(report.created_generations) {
                return Err(CryptoError::RestorePending);
            }
        }
        let invalid = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM crypto_restore_documents p LEFT JOIN document_crypto_keys k ON k.document_id=p.document_id AND k.dek_generation=p.new_generation WHERE p.run_id=$1 AND (NOT p.complete OR k.document_id IS NULL OR NOT k.active OR k.workspace_id<>p.workspace_id OR k.kek_id<>$2)) OR EXISTS(SELECT 1 FROM documents d WHERE NOT EXISTS(SELECT 1 FROM crypto_restore_documents p WHERE p.run_id=$1 AND p.document_id=d.document_id)) OR EXISTS(SELECT 1 FROM document_crypto_keys k WHERE NOT EXISTS(SELECT 1 FROM crypto_restore_documents p WHERE p.run_id=$1 AND p.document_id=k.document_id))",
        )
        .bind(run)
        .bind(to)
        .fetch_one(&self.crypto_pool)
        .await?;
        if invalid {
            return Err(CryptoError::KeyConflict);
        }
        let mut all_keys = sqlx::query_as::<_, (Uuid, i32)>(
            "SELECT document_id,dek_generation FROM document_crypto_keys ORDER BY document_id,dek_generation",
        )
        .fetch(&self.crypto_pool);
        while let Some((document, generation)) = all_keys.try_next().await? {
            self.document_key(
                DocumentId::from_bytes(*document.as_bytes())
                    .map_err(|_| CryptoError::InvalidContext)?,
                Some(u32::try_from(generation).map_err(|_| CryptoError::InvalidEnvelope)?),
            )
            .await?;
            report.verified_generations = report
                .verified_generations
                .checked_add(1)
                .ok_or(CryptoError::KeyConflict)?;
        }
        drop(all_keys);
        let mut tx = self.crypto_pool.begin().await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *tx)
            .await?;
        let restored_documents = sqlx::query_scalar::<_, Uuid>(
            "SELECT document_id FROM crypto_restore_documents WHERE run_id=$1 ORDER BY document_id",
        )
        .bind(run)
        .fetch_all(&mut *tx)
        .await?;
        for document in restored_documents {
            let document = DocumentId::from_bytes(*document.as_bytes())
                .map_err(|_| CryptoError::InvalidContext)?;
            crate::shares::invalidate_document_shares_after_restore(&mut tx, document)
                .await
                .map_err(|_error| CryptoError::KeyConflict)?;
        }
        // A backup can contain sessions revoked after its snapshot. Require a
        // fresh login before publishing the restored instance, atomically with
        // activation of its new write lineage and invalidation of guest shares.
        sqlx::query("DELETE FROM account_sessions")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO crypto_key_state(singleton,active_kek_id,rotated_at) VALUES(true,$1,clock_timestamp()) ON CONFLICT(singleton) DO UPDATE SET active_kek_id=EXCLUDED.active_kek_id,rotated_at=EXCLUDED.rotated_at")
            .bind(to).execute(&mut *tx).await?;
        let changed = sqlx::query("UPDATE crypto_restore_runs SET state='COMPLETE',completed_at=clock_timestamp() WHERE run_id=$1 AND target_kek_id=$2 AND target_physical_key_id=$3 AND state='PENDING'")
            .bind(run).bind(to).bind(target.as_slice()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(CryptoError::KeyConflict);
        }
        tx.commit().await?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalKeyProvider;
    use spreadsheet_application::{
        journal::{AuthenticatedActor, OperationJournal},
        keys::KeyProviderError,
    };
    use spreadsheet_domain::workbook::{
        ActorId, ColumnId, CommittedEvent, DocumentVersion, EventPrincipal, Operation, OperationId,
        RowId, SheetId, SheetSeed, Workbook,
    };
    use std::sync::Arc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type JournalBytes = (Uuid, i64, Vec<u8>, Vec<u8>, Vec<u8>);

    fn provider() -> Result<LocalKeyProvider, KeyProviderError> {
        let key = |byte: &str| byte.repeat(32);
        let json = format!(
            r#"{{"schema_version":1,"active_kek_id":"old","keys":[{{"id":"old","key_hex":"{}"}},{{"id":"old-alias","key_hex":"{}"}},{{"id":"fresh","key_hex":"{}"}},{{"id":"fresh-alias","key_hex":"{}"}},{{"id":"alternate","key_hex":"{}"}},{{"id":"historical","key_hex":"{}"}}]}}"#,
            key("01"),
            key("01"),
            key("02"),
            key("02"),
            key("03"),
            key("04"),
        );
        LocalKeyProvider::from_keyring_json(&json)
    }

    async fn database(pool: sqlx::PgPool) -> Result<PgDatabase, Box<dyn std::error::Error>> {
        let mut db = PgDatabase::for_test(pool)?;
        db.keys = Arc::new(provider()?);
        db.migrate().await?;
        Ok(db)
    }

    fn document(value: u8) -> Result<DocumentId, spreadsheet_domain::workbook::DomainError> {
        DocumentId::from_bytes([value; 16])
    }

    async fn create_documents(
        db: &PgDatabase,
    ) -> Result<(AuthenticatedActor, [DocumentId; 2], DocumentId), Box<dyn std::error::Error>> {
        let identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes([1; 16])?,
            workspace_id: WorkspaceId::from_bytes([2; 16])?,
        };
        db.issue_development_session([9; 32], identity.actor_id, identity.workspace_id)
            .await?;
        let documents = [document(3)?, document(4)?];
        for (index, document) in documents.into_iter().enumerate() {
            let offset = u8::try_from(index)?;
            let operation = Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: SheetId::from_bytes([20 + offset; 16])?,
                    name: "Retained restore fixture".into(),
                    rows: vec![RowId::from_bytes([30 + offset; 16])?],
                    columns: vec![ColumnId::from_bytes([40 + offset; 16])?],
                },
            };
            let workbook =
                Workbook::empty(document).candidate(&operation, DocumentVersion::new(1)?)?;
            db.create(
                identity,
                CommittedEvent {
                    document_id: document,
                    operation_id: OperationId::from_bytes([50 + offset; 16])?,
                    principal: EventPrincipal::User(identity.actor_id),
                    version: workbook.version(),
                    operation,
                    state_hash: workbook.hash(),
                },
            )
            .await?;
        }
        add_retained_generation(db, identity.workspace_id, documents[0], 2).await?;
        let orphan = document(5)?;
        db.ensure_document_key(identity.workspace_id, orphan)
            .await?;
        Ok((identity, documents, orphan))
    }

    async fn add_retained_generation(
        db: &PgDatabase,
        workspace: WorkspaceId,
        document: DocumentId,
        generation: i32,
    ) -> Result<(), CryptoError> {
        let old = db.document_key(document, None).await?;
        let nonce = reserve_nonce_with(
            &db.crypto_pool,
            &db.keys.physical_key_id("old")?,
            random_bytes::<24>,
        )
        .await?;
        let wrapped = db.keys.wrap_key(
            "old",
            &nonce,
            &wrapping_aad(
                workspace,
                document,
                u32::try_from(generation).map_err(|_| CryptoError::InvalidEnvelope)?,
                &old.physical_id,
            ),
            &old.material,
        )?;
        let mut tx = db.crypto_pool.begin().await?;
        sqlx::query("UPDATE document_crypto_keys SET active=false WHERE document_id=$1 AND active")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,$3,$4,'old',$5,$6,1)")
            .bind(Uuid::from_bytes(*document.as_bytes())).bind(Uuid::from_bytes(*workspace.as_bytes())).bind(generation).bind(old.physical_id.as_slice()).bind(&wrapped).bind(nonce.as_slice()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn journal_bytes(db: &PgDatabase) -> Result<Vec<JournalBytes>, sqlx::Error> {
        sqlx::query_as("SELECT document_id,committed_version,payload,command_fingerprint,canonical_event_hash FROM document_operations ORDER BY document_id,committed_version")
            .fetch_all(&db.pool).await
    }

    async fn active_kek(db: &PgDatabase) -> Result<String, sqlx::Error> {
        sqlx::query_scalar("SELECT active_kek_id FROM crypto_key_state WHERE singleton=true")
            .fetch_one(&db.pool)
            .await
    }

    #[sqlx::test(migrations = false)]
    async fn restore_activation_revokes_local_sessions_without_removing_accounts(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let phc = Zeroizing::new("$argon2id$v=19$m=65536,t=3,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned());
        let master = db
            .bootstrap_local_instance("master", phc, [101; 32])
            .await?;
        assert!(db.local_session([101; 32]).await?.is_some());
        db.renew_write_generations_after_restore("fresh").await?;
        assert!(
            db.local_session([101; 32]).await?.is_none(),
            "restored backup must not revive a previously revoked session"
        );
        let credential = db.local_credential("master").await?;
        let logged_in = db
            .complete_local_login(credential.as_ref(), true, [102; 32])
            .await?
            .ok_or("account missing after restore")?;
        assert_eq!(logged_in.user_id, master.user_id);
        assert!(logged_in.is_master);
        assert!(db.local_session([102; 32]).await?.is_some());
        db.encryption_readiness().await?;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn fresh_generations_preserve_journal_retained_ciphertext_and_orphans(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let (identity, documents, orphan) = create_documents(&db).await?;
        let before = journal_bytes(&db).await?;
        let old = db.document_key(documents[0], None).await?;
        let object = Uuid::from_bytes([60; 16]);
        let retained = db
            .seal_record(
                documents[0],
                2,
                "journal",
                object,
                1,
                b"retained generation two",
                64,
            )
            .await?;
        let report = db.renew_write_generations_after_restore("fresh").await?;
        assert_eq!(report.created_generations, 3);
        assert_eq!(report.verified_generations, 7);
        assert_eq!(active_kek(&db).await?, "fresh");
        assert_eq!(before, journal_bytes(&db).await?);
        assert_eq!(
            &**db
                .open_record(documents[0], 2, "journal", object, 1, &retained, 64)
                .await?,
            b"retained generation two"
        );
        let fresh = db.document_key(documents[0], None).await?;
        assert_eq!(fresh.generation, 3);
        assert_ne!(fresh.physical_id, old.physical_id);
        assert_eq!(db.document_key(orphan, None).await?.generation, 2);
        for document in documents {
            let events = db
                .load_events(identity, document, DocumentVersion::ZERO, 10)
                .await?;
            assert_eq!(events.len(), 1);
            let mut book = Workbook::empty(document);
            book.apply_event(&events[0])?;
            assert_eq!(book.hash(), events[0].state_hash);
        }
        let post = db
            .seal_record(documents[0], 3, "journal", object, 1, b"post restore", 64)
            .await?;
        assert_eq!(super::super::parse_envelope(&post, 64)?.0, 3);
        db.encryption_readiness().await?;
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh").await,
            Err(CryptoError::KeyConflict)
        ));
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn old_alias_missing_and_historical_physical_keks_are_rejected(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        create_documents(&db).await?;
        reserve_nonce_with(
            &db.crypto_pool,
            &db.keys.physical_key_id("historical")?,
            random_bytes::<24>,
        )
        .await?;
        let before = journal_bytes(&db).await?;
        let nonces = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
            .fetch_one(&db.pool)
            .await?;
        for target in ["old", "old-alias", "historical"] {
            assert!(matches!(
                db.renew_write_generations_after_restore(target).await,
                Err(CryptoError::KeyConflict)
            ));
        }
        assert!(matches!(
            db.renew_write_generations_after_restore("missing").await,
            Err(CryptoError::KeyProvider(KeyProviderError::KeyUnavailable))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_restore_runs")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?,
            nonces
        );
        assert_eq!(before, journal_bytes(&db).await?);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn interruption_blocks_all_writes_and_resumes_committed_generation_progress(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let (identity, documents, _) = create_documents(&db).await?;
        let before = journal_bytes(&db).await?;
        let object = Uuid::from_bytes([60; 16]);
        let retained = db
            .seal_record(documents[0], 2, "journal", object, 1, b"retained", 64)
            .await?;
        assert!(matches!(
            db.renew_write_generations_inner("fresh", Some(1)).await,
            Err(CryptoError::RestorePending)
        ));
        assert_eq!(active_kek(&db).await?, "old");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM crypto_restore_documents WHERE complete"
            )
            .fetch_one(&db.pool)
            .await?,
            1
        );
        assert!(matches!(
            db.check_write_lineage().await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.encryption_readiness().await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.ensure_document_key(identity.workspace_id, documents[0])
                .await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.seal_record(
                documents[0],
                2,
                "journal",
                Uuid::from_bytes([60; 16]),
                1,
                b"premature",
                64
            )
            .await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.rotate_keys("alternate").await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh-alias")
                .await,
            Err(CryptoError::KeyConflict)
        ));
        assert!(matches!(
            db.renew_write_generations_after_restore("alternate").await,
            Err(CryptoError::KeyConflict)
        ));
        assert_eq!(
            &**db
                .open_record(documents[0], 2, "journal", object, 1, &retained, 64)
                .await?,
            b"retained"
        );
        let run = sqlx::query_scalar::<_, Uuid>("SELECT current_run_id FROM crypto_restore_state")
            .fetch_one(&db.pool)
            .await?;
        let resumed = db.renew_write_generations_after_restore("fresh").await?;
        assert_eq!(resumed.run_id, run);
        assert_eq!(resumed.created_generations, 2);
        assert_eq!(resumed.verified_generations, 7);
        db.check_write_lineage().await?;
        assert_eq!(active_kek(&db).await?, "fresh");
        assert_eq!(before, journal_bytes(&db).await?);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn initialized_barrier_survives_restart_before_first_generation(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        create_documents(&db).await?;
        assert!(matches!(
            db.renew_write_generations_inner("fresh", Some(0)).await,
            Err(CryptoError::RestorePending)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM crypto_restore_documents WHERE complete"
            )
            .fetch_one(&db.pool)
            .await?,
            0
        );
        let reopened = PgDatabase {
            pool: db.pool.clone(),
            crypto_pool: db.crypto_pool.clone(),
            keys: Arc::new(provider()?),
        };
        assert!(matches!(
            reopened.check_write_lineage().await,
            Err(CryptoError::RestorePending)
        ));
        assert_eq!(
            reopened
                .renew_write_generations_after_restore("fresh")
                .await?
                .created_generations,
            3
        );
        reopened.encryption_readiness().await?;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn generation_overflow_keeps_barrier_pending_and_old_ciphertext_readable(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let workspace = WorkspaceId::from_bytes([2; 16])?;
        let document = document(3)?;
        db.ensure_document_key(workspace, document).await?;
        add_retained_generation(&db, workspace, document, i32::MAX).await?;
        let object = Uuid::from_bytes([60; 16]);
        let old = db
            .seal_record(
                document,
                1,
                "journal",
                object,
                1,
                b"last old generation",
                64,
            )
            .await?;
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh").await,
            Err(CryptoError::KeyConflict)
        ));
        assert!(matches!(
            db.check_write_lineage().await,
            Err(CryptoError::RestorePending)
        ));
        assert_eq!(
            db.document_key(document, None).await?.generation,
            i32::MAX as u32
        );
        assert_eq!(
            &**db
                .open_record(document, 1, "journal", object, 1, &old, 64)
                .await?,
            b"last old generation"
        );
        assert_eq!(active_kek(&db).await?, "old");
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn active_writer_prevents_restore_progress_until_maintenance_fence_is_available(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        create_documents(&db).await?;
        let writer = db.begin_guarded().await?;
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh").await,
            Err(CryptoError::KeyConflict)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_restore_runs")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        writer.rollback().await?;
        assert_eq!(
            db.renew_write_generations_after_restore("fresh")
                .await?
                .created_generations,
            3
        );
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn pending_migration_refuses_runtime_writes_until_activation(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let workspace = WorkspaceId::from_bytes([2; 16])?;
        let document = document(3)?;
        db.ensure_document_key(workspace, document).await?;
        sqlx::query("INSERT INTO storage_migration_state(singleton,state) VALUES(true,'PENDING')")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.encryption_readiness().await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.ensure_document_key(workspace, document).await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh").await,
            Err(CryptoError::RestorePending)
        ));
        assert!(matches!(
            db.seal_record(
                document,
                1,
                "journal",
                Uuid::from_bytes([60; 16]),
                1,
                b"premature",
                64
            )
            .await,
            Err(CryptoError::RestorePending)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_restore_runs")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        sqlx::query("UPDATE storage_migration_state SET state='COMPLETE',completed_at=clock_timestamp() WHERE singleton=true")
            .execute(&db.pool)
            .await?;
        db.check_write_lineage().await?;
        db.seal_record(
            document,
            1,
            "journal",
            Uuid::from_bytes([60; 16]),
            1,
            b"activated",
            64,
        )
        .await?;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn empty_restore_rejects_active_alias_and_retains_used_target_identity(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_key_state")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        assert!(matches!(
            db.renew_write_generations_after_restore("old-alias").await,
            Err(CryptoError::KeyConflict)
        ));
        let first = db.renew_write_generations_after_restore("fresh").await?;
        assert_eq!(first.created_generations, 0);
        assert_eq!(first.verified_generations, 0);
        assert_eq!(active_kek(&db).await?, "fresh");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh-alias")
                .await,
            Err(CryptoError::KeyConflict)
        ));
        let next = db
            .renew_write_generations_after_restore("alternate")
            .await?;
        assert_ne!(next.run_id, first.run_id);
        assert_eq!(next.created_generations, 0);
        db.encryption_readiness().await?;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn retained_dek_identity_rejects_target_without_nonce_registry_rows(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        let workspace = WorkspaceId::from_bytes([2; 16])?;
        let document = document(3)?;
        let material = Zeroizing::new([2; 32]);
        let physical = physical_key_id(&material);
        assert_eq!(physical, db.keys.physical_key_id("fresh")?);
        let nonce = [7; 24];
        let wrapped = db.keys.wrap_key(
            "old",
            &nonce,
            &wrapping_aad(workspace, document, 1, &physical),
            &material,
        )?;
        // Model a retained physical key even when its old reservation is absent.
        sqlx::query("INSERT INTO document_crypto_keys(document_id,workspace_id,dek_generation,physical_key_id,kek_id,wrapped_dek,wrapping_nonce,wrap_schema_version) VALUES($1,$2,1,$3,'old',$4,$5,1)")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .bind(Uuid::from_bytes(*workspace.as_bytes()))
            .bind(physical.as_slice()).bind(&wrapped).bind(nonce.as_slice())
            .execute(&db.pool).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        assert!(matches!(
            db.renew_write_generations_after_restore("fresh").await,
            Err(CryptoError::KeyConflict)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_restore_runs")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        assert_eq!(db.document_key(document, None).await?.physical_id, physical);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn initialization_marker_rolls_back_with_terminated_exclusive_owner(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let db = database(pool).await?;
        create_documents(&db).await?;
        let before = journal_bytes(&db).await?;
        let mut fence = db.restore_fence().await?;
        let backend = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
            .fetch_one(&mut *fence)
            .await?;
        let target = db.keys.physical_key_id("fresh")?;
        let (_, initialized) = db.restore_run(&mut fence, "fresh", &target).await?;
        assert!(initialized);
        db.check_write_lineage().await?;
        assert!(
            !sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock_shared($1)")
                .bind(RESTORE_LOCK)
                .fetch_one(&db.crypto_pool)
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT pg_terminate_backend($1)")
                .bind(backend)
                .fetch_one(&db.crypto_pool)
                .await?
        );
        assert!(fence.commit().await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_restore_runs")
                .fetch_one(&db.pool)
                .await?,
            0
        );
        let writer = db.begin_guarded().await?;
        writer.rollback().await?;
        assert_eq!(before, journal_bytes(&db).await?);
        assert_eq!(
            db.renew_write_generations_after_restore("fresh")
                .await?
                .created_generations,
            3
        );
        Ok(())
    }
}
