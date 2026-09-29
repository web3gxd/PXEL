use crate::PgDatabase;
use bytes::Bytes;
use futures_util::{TryStreamExt, stream};
use spreadsheet_application::{
    BlobStore, BlobStoreError,
    journal::{AuthenticatedActor, JournalError, OperationJournal},
};
use spreadsheet_domain::{
    BlobKey, Checksum,
    workbook::{DocumentId, DocumentVersion, SnapshotId, Workbook},
};
use spreadsheet_storage_codec as codec;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot source does not match the committed journal")]
    SourceMismatch,
    #[error("snapshot metadata or object is corrupt")]
    Corrupt,
    #[error("snapshot is too large")]
    TooLarge,
    #[error("snapshot codec failed")]
    Codec(#[from] codec::CodecError),
    #[error("snapshot object storage failed")]
    Blob(#[from] BlobStoreError),
    #[error("snapshot metadata storage failed")]
    Database(#[from] sqlx::Error),
    #[error("journal replay failed")]
    Journal(#[from] JournalError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotReceipt {
    pub snapshot_id: SnapshotId,
    pub document_id: DocumentId,
    pub version: DocumentVersion,
}

pub struct RecoveryResult {
    pub workbook: Workbook,
    pub snapshot_version: Option<DocumentVersion>,
    pub replayed_events: u64,
}

fn uuid(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}
fn version(number: i64) -> Result<DocumentVersion, SnapshotError> {
    DocumentVersion::new(u64::try_from(number).map_err(|_| SnapshotError::Corrupt)?)
        .map_err(|_| SnapshotError::Corrupt)
}
fn sql_number(number: u64) -> Result<i64, SnapshotError> {
    i64::try_from(number).map_err(|_| SnapshotError::Corrupt)
}
fn bytes32(bytes: &[u8]) -> Result<[u8; 32], SnapshotError> {
    bytes.try_into().map_err(|_| SnapshotError::Corrupt)
}

async fn read_verified<B: BlobStore>(
    blobs: &B,
    key: &BlobKey,
    expected_len: usize,
    expected_checksum: [u8; 32],
) -> Result<Vec<u8>, SnapshotError> {
    if expected_len > codec::MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::TooLarge);
    }
    let download = blobs.get(key).await?;
    if download.metadata.size_bytes != expected_len as u64
        || download.metadata.checksum != Checksum::from_bytes(expected_checksum)
    {
        return Err(SnapshotError::Corrupt);
    }
    let mut body = Vec::with_capacity(expected_len);
    let mut stream = download.stream;
    while let Some(chunk) = stream.try_next().await? {
        if body.len().saturating_add(chunk.len()) > expected_len {
            return Err(SnapshotError::Corrupt);
        }
        body.extend_from_slice(&chunk);
    }
    if body.len() != expected_len || codec::checksum(&body) != expected_checksum {
        return Err(SnapshotError::Corrupt);
    }
    Ok(body)
}

impl PgDatabase {
    /// Authenticated point-in-time recovery. Each tail page is checked by the
    /// journal adapter, including contiguous versions, checksums and ACL.
    pub async fn recover_document<B: BlobStore>(
        &self,
        blobs: &B,
        identity: AuthenticatedActor,
        document_id: DocumentId,
    ) -> Result<RecoveryResult, SnapshotError> {
        let target = self.access(identity, document_id).await?.head;
        let snapshot = self
            .latest_valid_snapshot(blobs, document_id, target)
            .await?;
        let snapshot_version = snapshot.as_ref().map(Workbook::version);
        let mut workbook = snapshot.unwrap_or_else(|| Workbook::empty(document_id));
        let mut replayed_events = 0_u64;
        while workbook.version() < target {
            let limit = (target.get() - workbook.version().get()).min(1000) as u32;
            let events = self
                .load_events(identity, document_id, workbook.version(), limit)
                .await?;
            if events.is_empty() {
                return Err(SnapshotError::Corrupt);
            }
            for event in events {
                workbook
                    .apply_event(&event)
                    .map_err(|_| SnapshotError::Corrupt)?;
                replayed_events += 1;
            }
        }
        if workbook.version() != target {
            return Err(SnapshotError::Corrupt);
        }
        Ok(RecoveryResult {
            workbook,
            snapshot_version,
            replayed_events,
        })
    }

    /// Internal checkpoint operation. Only a journal-backed projection is eligible.
    /// The object is verified through a fresh read before the short activation transaction.
    pub async fn publish_snapshot<B: BlobStore>(
        &self,
        blobs: &B,
        snapshot_id: SnapshotId,
        workbook: &Workbook,
        engine_version: &str,
        compatibility_profile: &str,
    ) -> Result<SnapshotReceipt, SnapshotError> {
        let document_id = workbook.document_id();
        let source = sqlx::query!(
            "SELECT o.payload, o.payload_checksum, o.canonical_event_hash, d.head_version, d.state FROM document_operations o JOIN documents d ON d.document_id=o.document_id WHERE o.document_id=$1 AND o.committed_version=$2",
            uuid(document_id.as_bytes()), sql_number(workbook.version().get())?
        ).fetch_optional(&self.pool).await?.ok_or(SnapshotError::SourceMismatch)?;
        if source.state != "active"
            || source.head_version < sql_number(workbook.version().get())?
            || codec::checksum(&source.payload).as_slice() != source.payload_checksum
        {
            return Err(SnapshotError::SourceMismatch);
        }
        let event = codec::decode(&source.payload).map_err(|_| SnapshotError::SourceMismatch)?;
        if event.document_id != document_id
            || event.version != workbook.version()
            || event.state_hash != workbook.hash()
            || codec::event_hash(&event)
                .map_err(|_| SnapshotError::SourceMismatch)?
                .as_slice()
                != source.canonical_event_hash
        {
            return Err(SnapshotError::SourceMismatch);
        }
        let body = codec::encode_snapshot(workbook, engine_version, compatibility_profile)?;
        let body_len = body.len();
        let checksum = codec::checksum(&body);
        let key_uuid = uuid(snapshot_id.as_bytes());
        let key = BlobKey::parse(&key_uuid.to_string()).map_err(|_| SnapshotError::Corrupt)?;
        sqlx::query!(
            "INSERT INTO document_snapshots (snapshot_id, document_id, snapshot_version, last_axis_change_version, snapshot_schema_version, object_key, byte_length, payload_checksum, state_hash, writer_engine_version, writer_compatibility_profile) VALUES ($1,$2,$3,$4,1,$5,$6,$7,$8,$9,$10)",
            key_uuid, uuid(document_id.as_bytes()), sql_number(workbook.version().get())?,
            sql_number(workbook.last_axis_change_version().get())?, key_uuid,
            sql_number(body.len() as u64)?, &checksum[..], &workbook.hash()[..], engine_version, compatibility_profile
        ).execute(&self.pool).await?;
        blobs
            .put(
                &key,
                Box::pin(stream::once(async move { Ok(Bytes::from(body)) })),
                Checksum::from_bytes(checksum),
            )
            .await?;
        sqlx::query!("UPDATE document_snapshots SET state='STORED' WHERE snapshot_id=$1 AND state='UPLOADING'", key_uuid).execute(&self.pool).await?;
        let verified = async {
            let verified = read_verified(blobs, &key, body_len, checksum).await?;
            codec::decode_snapshot(
                &verified,
                document_id,
                workbook.version(),
                workbook.last_axis_change_version(),
                workbook.hash(),
                engine_version,
                compatibility_profile,
            )?;
            Ok::<(), SnapshotError>(())
        }
        .await;
        if let Err(error) = verified {
            sqlx::query!("UPDATE document_snapshots SET state='REJECTED' WHERE snapshot_id=$1 AND state='STORED'", key_uuid).execute(&self.pool).await?;
            return Err(error);
        }
        sqlx::query!("UPDATE document_snapshots SET state='VERIFIED', verified_at=now() WHERE snapshot_id=$1 AND state='STORED'", key_uuid).execute(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        let document = sqlx::query!(
            "SELECT head_version, state FROM documents WHERE document_id=$1 FOR UPDATE",
            uuid(document_id.as_bytes())
        )
        .fetch_one(&mut *tx)
        .await?;
        if document.state != "active"
            || document.head_version < sql_number(workbook.version().get())?
        {
            return Err(SnapshotError::SourceMismatch);
        }
        sqlx::query!("UPDATE document_snapshots SET state='ACTIVE', activated_at=now() WHERE snapshot_id=$1 AND state='VERIFIED'", key_uuid).execute(&mut *tx).await?;
        sqlx::query!("UPDATE documents SET preferred_snapshot_id=$2 WHERE document_id=$1 AND (preferred_snapshot_id IS NULL OR (SELECT snapshot_version FROM document_snapshots WHERE snapshot_id=preferred_snapshot_id) <= $3)", uuid(document_id.as_bytes()), key_uuid, sql_number(workbook.version().get())?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(SnapshotReceipt {
            snapshot_id,
            document_id,
            version: workbook.version(),
        })
    }

    /// Returns the newest verified ACTIVE image. A corrupt or missing object is
    /// skipped so replay can continue from an older checkpoint or the genesis log.
    pub async fn latest_valid_snapshot<B: BlobStore>(
        &self,
        blobs: &B,
        document_id: DocumentId,
        through: DocumentVersion,
    ) -> Result<Option<Workbook>, SnapshotError> {
        let rows = sqlx::query!(
            "SELECT snapshot_version, last_axis_change_version, object_key, byte_length, payload_checksum, state_hash, writer_engine_version, writer_compatibility_profile FROM document_snapshots WHERE document_id=$1 AND snapshot_version<=$2 AND state='ACTIVE' ORDER BY snapshot_version DESC, activated_at DESC LIMIT 128",
            uuid(document_id.as_bytes()), sql_number(through.get())?
        ).fetch_all(&self.pool).await?;
        for row in rows {
            let key =
                BlobKey::parse(&row.object_key.to_string()).map_err(|_| SnapshotError::Corrupt)?;
            let expected_checksum = bytes32(&row.payload_checksum)?;
            let expected_hash = bytes32(&row.state_hash)?;
            let len = usize::try_from(row.byte_length).map_err(|_| SnapshotError::Corrupt)?;
            let body = match read_verified(blobs, &key, len, expected_checksum).await {
                Ok(body) => body,
                Err(SnapshotError::Blob(
                    BlobStoreError::NotFound
                    | BlobStoreError::CorruptBlob
                    | BlobStoreError::ChecksumMismatch
                    | BlobStoreError::UnsupportedSchema,
                ))
                | Err(SnapshotError::Corrupt | SnapshotError::TooLarge) => continue,
                Err(error) => return Err(error),
            };
            match codec::decode_snapshot(
                &body,
                document_id,
                version(row.snapshot_version)?,
                version(row.last_axis_change_version)?,
                expected_hash,
                &row.writer_engine_version,
                &row.writer_compatibility_profile,
            ) {
                Ok(workbook) => return Ok(Some(workbook)),
                Err(_) => continue,
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalFilesystemBlobStore;
    use spreadsheet_application::journal::{
        AuthenticatedActor, CommitOutcome, CommitRequest, OperationJournal,
    };
    use spreadsheet_collab::{InProcessSessionDirectory, SessionDirectory};
    use spreadsheet_domain::workbook::{
        ActorId, CellAddress, CellValue, ColumnId, Command, CommandPayload, CommittedEvent,
        EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed, WorkspaceId,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn assert_preferred(
        db: &PgDatabase,
        document_id: DocumentId,
        snapshot_id: SnapshotId,
    ) -> TestResult {
        let active = sqlx::query!(
            "SELECT preferred_snapshot_id FROM documents WHERE document_id=$1",
            uuid(document_id.as_bytes())
        )
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(
            active.preferred_snapshot_id,
            Some(uuid(snapshot_id.as_bytes()))
        );
        Ok(())
    }

    async fn assert_full_replay_fallback(
        db: &PgDatabase,
        blobs: &LocalFilesystemBlobStore,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        through: DocumentVersion,
        older_id: SnapshotId,
    ) -> TestResult {
        let older_key = BlobKey::parse(&uuid(older_id.as_bytes()).to_string())?;
        blobs.delete(&older_key).await?;
        assert!(
            db.latest_valid_snapshot(blobs, document_id, through)
                .await?
                .is_none()
        );
        let recovered = Box::pin(db.recover_document(blobs, identity, document_id)).await?;
        assert_eq!(recovered.snapshot_version, None);
        assert_eq!(recovered.replayed_events, through.get());
        Ok(())
    }

    async fn assert_actor_boots_from_snapshot(
        db: &PgDatabase,
        blobs: &LocalFilesystemBlobStore,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        expected: &Workbook,
    ) -> TestResult {
        let observed_replay = Arc::new(AtomicU64::new(u64::MAX));
        let observed = observed_replay.clone();
        let database = db.clone();
        let blobs = blobs.clone();
        let directory =
            InProcessSessionDirectory::with_recovery(db.clone(), move |actor, document| {
                let database = database.clone();
                let blobs = blobs.clone();
                let observed = observed.clone();
                async move {
                    let result = database
                        .recover_document(&blobs, actor, document)
                        .await
                        .map_err(|_| JournalError::Corrupt)?;
                    observed.store(result.replayed_events, Ordering::SeqCst);
                    Ok(result.workbook)
                }
            });
        let session = directory.session(identity, document_id).await?;
        let opened = session.read(identity).await?;
        assert_eq!(opened.version(), expected.version());
        assert_eq!(opened.hash(), expected.hash());
        assert_eq!(observed_replay.load(Ordering::SeqCst), 0);
        let mut calculated = session.watch_calculation(identity).await?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if calculated
                    .borrow_and_update()
                    .as_ref()
                    .is_some_and(|state| state.input_version == expected.version())
                {
                    return Ok::<(), tokio::sync::watch::error::RecvError>(());
                }
                calculated.changed().await?;
            }
        })
        .await??;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn active_snapshot_round_trip_and_corrupt_newest_falls_back(
        pool: sqlx::PgPool,
    ) -> TestResult {
        Box::pin(snapshot_scenario(pool)).await
    }

    // Keep the SQLx test thread's Windows stack small; the scenario owns several
    // large async state machines, so its future is allocated separately.
    async fn snapshot_scenario(pool: sqlx::PgPool) -> TestResult {
        let db = PgDatabase { pool };
        db.migrate().await?;
        let identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes([1; 16])?,
            workspace_id: WorkspaceId::from_bytes([2; 16])?,
        };
        db.issue_development_session([3; 32], identity.actor_id, identity.workspace_id)
            .await?;
        let document_id = DocumentId::from_bytes([4; 16])?;
        let first = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes([5; 16])?,
                name: "Test".into(),
                rows: vec![RowId::from_bytes([6; 16])?],
                columns: vec![ColumnId::from_bytes([7; 16])?],
            },
        };
        let book1 = Workbook::empty(document_id).candidate(&first, DocumentVersion::new(1)?)?;
        db.create(
            identity,
            CommittedEvent {
                document_id,
                operation_id: OperationId::from_bytes([8; 16])?,
                principal: EventPrincipal::User(identity.actor_id),
                version: book1.version(),
                operation: first,
                state_hash: book1.hash(),
            },
        )
        .await?;
        let temp = tempfile::tempdir()?;
        let blobs = LocalFilesystemBlobStore::new(
            temp.path().join("blobs"),
            codec::MAX_SNAPSHOT_BYTES as u64,
        )
        .await?;
        let older_id = SnapshotId::from_bytes([10; 16])?;
        db.publish_snapshot(&blobs, older_id, &book1, "engine-v1", "pxel-v1")
            .await?;
        assert_eq!(
            db.latest_valid_snapshot(&blobs, document_id, book1.version())
                .await?
                .ok_or("missing snapshot")?
                .hash(),
            book1.hash()
        );

        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let value = CellValue::Text("saved".into());
        let operation = Operation::SetCellValue {
            address,
            value: value.clone(),
        };
        let book2 = book1.candidate(&operation, DocumentVersion::new(2)?)?;
        let operation_id = OperationId::from_bytes([9; 16])?;
        let request = CommitRequest {
            identity,
            expected_head: book1.version(),
            expected_access: db.access(identity, document_id).await?,
            command: Command {
                operation_id,
                document_id,
                actor_id: identity.actor_id,
                base_version: book1.version(),
                client_sequence: 1,
                operation: CommandPayload::SetCellValue { address, value },
            },
            event: CommittedEvent {
                document_id,
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: book2.version(),
                operation,
                state_hash: book2.hash(),
            },
        };
        assert!(matches!(
            db.commit(&request).await?,
            CommitOutcome::Committed(_)
        ));
        let newer_id = SnapshotId::from_bytes([11; 16])?;
        db.publish_snapshot(&blobs, newer_id, &book2, "engine-v1", "pxel-v1")
            .await?;
        assert_preferred(&db, document_id, newer_id).await?;
        assert_eq!(
            db.latest_valid_snapshot(&blobs, document_id, book2.version())
                .await?
                .ok_or("missing newer snapshot")?
                .hash(),
            book2.hash()
        );
        let from_newest = Box::pin(db.recover_document(&blobs, identity, document_id)).await?;
        assert_eq!(from_newest.snapshot_version, Some(book2.version()));
        assert_eq!(from_newest.replayed_events, 0);
        assert_eq!(from_newest.workbook.hash(), book2.hash());
        Box::pin(assert_actor_boots_from_snapshot(
            &db,
            &blobs,
            identity,
            document_id,
            &book2,
        ))
        .await?;
        let newer_key = BlobKey::parse(&uuid(newer_id.as_bytes()).to_string())?;
        let blob_path = temp.path().join("blobs").join(newer_key.as_str());
        let mut damaged = tokio::fs::read(&blob_path).await?;
        let final_byte = damaged.last_mut().ok_or("empty blob")?;
        *final_byte ^= 0x01;
        tokio::fs::write(&blob_path, damaged).await?;
        let fallback = db
            .latest_valid_snapshot(&blobs, document_id, book2.version())
            .await?
            .ok_or("missing fallback snapshot")?;
        assert_eq!(fallback.version(), book1.version());
        assert_eq!(fallback.hash(), book1.hash());
        let recovered = Box::pin(db.recover_document(&blobs, identity, document_id)).await?;
        assert_eq!(recovered.snapshot_version, Some(book1.version()));
        assert_eq!(recovered.replayed_events, 1);
        assert_eq!(recovered.workbook.hash(), book2.hash());
        assert_full_replay_fallback(
            &db,
            &blobs,
            identity,
            document_id,
            book2.version(),
            older_id,
        )
        .await?;
        Ok(())
    }
}
