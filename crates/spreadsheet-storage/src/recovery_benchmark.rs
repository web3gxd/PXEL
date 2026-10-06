use crate::{LocalFilesystemBlobStore, PgDatabase};
use spreadsheet_application::journal::{
    AuthenticatedActor, CommitOutcome, CommitRequest, OperationJournal,
};
use spreadsheet_domain::workbook::{
    ActorId, CellAddress, CellValue, ColumnId, Command, CommandPayload, CommittedEvent, DocumentId,
    DocumentVersion, EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed, SnapshotId,
    Workbook, WorkspaceId,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

const PREFIX_EVENTS: u64 = 10_000;
const TAIL_EVENTS: u64 = 10_000;
const SAMPLES: usize = 20;
type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

#[sqlx::test(migrations = false)]
#[ignore = "20,000 durable PostgreSQL commits; run with just bench-recovery-storage"]
async fn persisted_snapshot_plus_ten_thousand_tail_events(pool: sqlx::PgPool) -> BenchResult<()> {
    Box::pin(run_benchmark(pool)).await
}

async fn run_benchmark(pool: sqlx::PgPool) -> BenchResult<()> {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    db.readiness().await?;
    let identity = AuthenticatedActor {
        actor_id: ActorId::from_bytes(*Uuid::now_v7().as_bytes())?,
        workspace_id: WorkspaceId::from_bytes(*Uuid::now_v7().as_bytes())?,
    };
    db.issue_development_session([7; 32], identity.actor_id, identity.workspace_id)
        .await?;
    let document_id = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
    let address = CellAddress {
        sheet: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
        row: RowId::from_bytes(*Uuid::now_v7().as_bytes())?,
        column: ColumnId::from_bytes(*Uuid::now_v7().as_bytes())?,
    };
    let genesis_operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: address.sheet,
            name: "Recovery benchmark".into(),
            rows: vec![address.row],
            columns: vec![address.column],
        },
    };
    let mut workbook =
        Workbook::empty(document_id).candidate(&genesis_operation, DocumentVersion::new(1)?)?;
    db.create(
        identity,
        CommittedEvent {
            document_id,
            operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
            principal: EventPrincipal::User(identity.actor_id),
            version: workbook.version(),
            operation: genesis_operation,
            state_hash: workbook.hash(),
        },
    )
    .await?;
    let access = db.access(identity, document_id).await?;
    let temp = tempfile::tempdir()?;
    let blobs = LocalFilesystemBlobStore::new(temp.path().join("blobs"), 64 * 1024 * 1024).await?;
    let setup_started = Instant::now();
    for number in 1..=PREFIX_EVENTS + TAIL_EVENTS {
        let value = CellValue::Number(number as f64);
        let operation = Operation::SetCellValue {
            address,
            value: value.clone(),
        };
        let candidate = workbook.candidate(&operation, DocumentVersion::new(number + 1)?)?;
        let operation_id = OperationId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let request = CommitRequest {
            inverse_projection: None,
            identity,
            expected_head: workbook.version(),
            expected_access: access,
            command: Command {
                operation_id,
                document_id,
                actor_id: identity.actor_id,
                base_version: workbook.version(),
                client_sequence: number,
                operation: CommandPayload::SetCellValue { address, value },
            },
            event: CommittedEvent {
                document_id,
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: candidate.version(),
                operation,
                state_hash: candidate.hash(),
            },
        };
        if !matches!(db.commit(&request).await?, CommitOutcome::Committed(_)) {
            return Err("benchmark fixture did not append exactly once".into());
        }
        workbook = candidate;
        if number == PREFIX_EVENTS {
            db.publish_snapshot(
                &blobs,
                SnapshotId::from_bytes(*Uuid::now_v7().as_bytes())?,
                Arc::new(workbook.clone()),
                "benchmark-engine-v1",
                "pxel-v1",
            )
            .await?;
        }
    }
    let setup_seconds = setup_started.elapsed().as_secs_f64();
    let expected_hash = workbook.hash();
    for _ in 0..2 {
        let recovered = db.recover_document(&blobs, identity, document_id).await?;
        verify_recovered(&recovered, expected_hash)?;
        let full = replay_full(&db, identity, document_id, workbook.version()).await?;
        if full.hash() != expected_hash {
            return Err("full replay hash differs from committed head".into());
        }
    }
    let mut snapshot_times = Vec::with_capacity(SAMPLES);
    let mut full_times = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        if sample % 2 == 0 {
            snapshot_times
                .push(time_snapshot(&db, &blobs, identity, document_id, expected_hash).await?);
            full_times.push(
                time_full(
                    &db,
                    identity,
                    document_id,
                    workbook.version(),
                    expected_hash,
                )
                .await?,
            );
        } else {
            full_times.push(
                time_full(
                    &db,
                    identity,
                    document_id,
                    workbook.version(),
                    expected_hash,
                )
                .await?,
            );
            snapshot_times
                .push(time_snapshot(&db, &blobs, identity, document_id, expected_hash).await?);
        }
    }
    let snapshot = summarize(&snapshot_times)?;
    let full = summarize(&full_times)?;
    println!(
        "persisted recovery benchmark: prefix={PREFIX_EVENTS}, tail={TAIL_EVENTS}, samples={SAMPLES}, setup_seconds={setup_seconds:.3}"
    );
    println!(
        "snapshot + tail ms: min={:.3}, p50={:.3}, p95={:.3}, max={:.3}",
        snapshot.0, snapshot.1, snapshot.2, snapshot.3
    );
    println!(
        "full journal replay ms: min={:.3}, p50={:.3}, p95={:.3}, max={:.3}",
        full.0, full.1, full.2, full.3
    );
    Ok(())
}

fn verify_recovered(recovered: &crate::RecoveryResult, expected_hash: [u8; 32]) -> BenchResult<()> {
    if recovered.snapshot_version != Some(DocumentVersion::new(PREFIX_EVENTS + 1)?)
        || recovered.replayed_events != TAIL_EVENTS
        || recovered.workbook.hash() != expected_hash
    {
        return Err("snapshot recovery missed its checkpoint, tail or hash".into());
    }
    Ok(())
}

async fn time_snapshot(
    db: &PgDatabase,
    blobs: &LocalFilesystemBlobStore,
    identity: AuthenticatedActor,
    document_id: DocumentId,
    expected_hash: [u8; 32],
) -> BenchResult<Duration> {
    let started = Instant::now();
    let recovered = db.recover_document(blobs, identity, document_id).await?;
    let elapsed = started.elapsed();
    verify_recovered(&recovered, expected_hash)?;
    Ok(elapsed)
}

async fn time_full(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    document_id: DocumentId,
    target: DocumentVersion,
    expected_hash: [u8; 32],
) -> BenchResult<Duration> {
    let started = Instant::now();
    let full = replay_full(db, identity, document_id, target).await?;
    let elapsed = started.elapsed();
    if full.hash() != expected_hash {
        return Err("full replay hash differs from committed head".into());
    }
    Ok(elapsed)
}

async fn replay_full(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    document_id: DocumentId,
    target: DocumentVersion,
) -> BenchResult<Workbook> {
    let mut workbook = Workbook::empty(document_id);
    while workbook.version() < target {
        let remaining = target.get() - workbook.version().get();
        let events = db
            .load_events(
                identity,
                document_id,
                workbook.version(),
                remaining.min(1000) as u32,
            )
            .await?;
        if events.is_empty() {
            return Err("journal ended before the committed head".into());
        }
        for event in events {
            workbook.apply_event(&event)?;
        }
    }
    Ok(workbook)
}

fn summarize(samples: &[Duration]) -> BenchResult<(f64, f64, f64, f64)> {
    if samples.len() != SAMPLES {
        return Err("incomplete recovery sample set".into());
    }
    let mut measured: Vec<f64> = samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1000.0)
        .collect();
    measured.sort_by(f64::total_cmp);
    let percentile = |percent: usize| measured[(measured.len() * percent).div_ceil(100) - 1];
    Ok((
        measured[0],
        percentile(50),
        percentile(95),
        measured[measured.len() - 1],
    ))
}
