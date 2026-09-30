//! Release-only measurements through the real PostgreSQL journal and actor queue.
use super::*;
use spreadsheet_collab::{
    DocumentSession, InProcessSessionDirectory, SessionDirectory, SessionError,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{sync::Barrier, task::JoinSet};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn populated_book(
    database: &PgDatabase,
    identity: AuthenticatedActor,
    heavy: bool,
) -> TestResult<Workbook> {
    let seed = SheetSeed {
        id: SheetId::from_bytes([50; 16])?,
        name: "Benchmark".into(),
        rows: (10_000..11_000u128)
            .map(|n| RowId::from_bytes(n.to_be_bytes()))
            .collect::<Result<Vec<_>, _>>()?,
        columns: vec![ColumnId::from_bytes([51; 16])?],
    };
    let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
    let create = Operation::CreateWorkbook {
        sheet: seed.clone(),
    };
    let mut book = Workbook::empty(document).candidate(&create, DocumentVersion::new(1)?)?;
    database
        .create(
            identity,
            CommittedEvent {
                document_id: document,
                operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
                principal: EventPrincipal::User(identity.actor_id),
                version: book.version(),
                operation: create,
                state_hash: book.hash(),
            },
        )
        .await?;
    // Setup writes use actual candidate validation, matching command/event and durable CAS.
    // No in-memory-only projection is injected into the session directory.
    for (index, row) in seed.rows.iter().enumerate() {
        let address = CellAddress {
            sheet: seed.id,
            row: *row,
            column: seed.columns[0],
        };
        let (payload, operation) = if heavy || index == 0 {
            let source = if heavy {
                "=NOW()+SUM(1,2,3,4)"
            } else {
                "=NOW()"
            };
            (
                CommandPayload::SetCellFormula {
                    address,
                    source: spreadsheet_domain::formula::FormulaSource::new(source.into())?,
                },
                Operation::SetCellFormula {
                    address,
                    formula: spreadsheet_formula::binder::bind(source, &book, address)?,
                },
            )
        } else {
            (
                CommandPayload::SetCellValue {
                    address,
                    value: CellValue::Number(1.0),
                },
                Operation::SetCellValue {
                    address,
                    value: CellValue::Number(1.0),
                },
            )
        };
        let version = DocumentVersion::new(book.version().get() + 1)?;
        let candidate = book.candidate(&operation, version)?;
        let id = OperationId::from_bytes(*Uuid::now_v7().as_bytes())?;
        assert!(matches!(
            database
                .commit(&CommitRequest {
                    move_inverse_projection: None,
                    identity,
                    expected_head: book.version(),
                    expected_access: database.access(identity, document).await?,
                    command: Command {
                        operation_id: id,
                        document_id: document,
                        actor_id: identity.actor_id,
                        base_version: book.version(),
                        client_sequence: index as u64 + 1,
                        operation: payload
                    },
                    event: CommittedEvent {
                        document_id: document,
                        operation_id: id,
                        principal: EventPrincipal::User(identity.actor_id),
                        version,
                        operation,
                        state_hash: candidate.hash()
                    },
                })
                .await?,
            CommitOutcome::Committed(_)
        ));
        book = candidate;
    }
    assert_eq!(book.sheets()[0].cells().len(), 1000);
    Ok(book)
}

struct Burst {
    accepted: usize,
    full: usize,
    probe_retries: usize,
    enqueue_us: Vec<f64>,
    drain_ms: f64,
}

async fn burst(
    session: &DocumentSession,
    identity: AuthenticatedActor,
    producers: usize,
) -> TestResult<Burst> {
    let barrier = Arc::new(Barrier::new(producers + 1));
    let mut tasks = JoinSet::new();
    for _ in 0..producers {
        let session = session.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            let mut accepted = 0;
            let mut full = 0;
            let mut samples = Vec::new();
            for _ in 0..4 {
                let start = Instant::now();
                let outcome = session.request_timer(identity);
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
                match outcome {
                    Ok(()) => accepted += 1,
                    Err(SessionError::QueueFull) => full += 1,
                    Err(error) => return Err(error),
                }
            }
            Ok((accepted, full, samples))
        });
    }
    let start = Instant::now();
    barrier.wait().await;
    let mut result = Burst {
        accepted: 0,
        full: 0,
        probe_retries: 0,
        enqueue_us: Vec::new(),
        drain_ms: 0.0,
    };
    while let Some(task) = tasks.join_next().await {
        let (accepted, full, samples) = task??;
        result.accepted += accepted;
        result.full += full;
        result.enqueue_us.extend(samples);
    }
    let mut drained = false;
    for _ in 0..1000 {
        match session.head(identity).await {
            Ok(_) => {
                drained = true;
                break;
            }
            Err(SessionError::QueueFull) => {
                result.probe_retries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if !drained {
        return Err("actor drain barrier exhausted retry budget".into());
    }
    result.drain_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(result.accepted + result.full, producers * 4);
    Ok(result)
}

#[sqlx::test(migrations = false)]
#[ignore = "release PostgreSQL actor admission timing harness; not WebSocket latency"]
async fn timer_admission_benchmark(pool: sqlx::PgPool) -> TestResult {
    let (database, identity, _) = super::tests::fixture(pool).await?;
    let settings: (String, String, String, String) = sqlx::query_as(
        "SELECT current_setting('server_version'), current_setting('synchronous_commit'), current_setting('fsync'), current_setting('full_page_writes')"
    ).fetch_one(&database.pool).await?;
    assert_eq!(
        (&settings.1, &settings.2, &settings.3),
        (&"on".to_string(), &"on".to_string(), &"on".to_string())
    );
    println!(
        "timer_admission postgres={} synchronous_commit={} fsync={} full_page_writes={} cells=1000 attempts_per_producer=4 samples=20 warmups=2",
        settings.0, settings.1, settings.2, settings.3
    );
    for heavy in [false, true] {
        let book = populated_book(&database, identity, heavy).await?;
        let clock = Arc::new(AtomicI64::new(0));
        let sampled_clock = clock.clone();
        let counter = Arc::new(AtomicU64::new(0));
        let sampled_counter = counter.clone();
        let directory =
            InProcessSessionDirectory::new(database.clone()).with_context_sampler(move |reason| {
                let count = sampled_counter.fetch_add(1, Ordering::SeqCst);
                let timestamp = sampled_clock.load(Ordering::SeqCst);
                let mut seed = [0; 32];
                seed[..8].copy_from_slice(&count.to_be_bytes());
                Ok((
                    OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
                    CalculationContext::new(
                        CalculationContextId::from_bytes(*Uuid::now_v7().as_bytes())?,
                        1,
                        timestamp,
                        0,
                        i32::try_from(timestamp / 86_400_000)
                            .map_err(|_| SessionError::InvalidCommand)?,
                        seed,
                        spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                        spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                        reason,
                    )?,
                ))
            });
        let session = directory.session(identity, book.document_id()).await?;
        let mut current = session.read(identity).await?;
        // Wait for initial calculation outside timing. Later epoch calculation may compete for CPU.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(state) = session.calculated(identity).await? {
                    if !matches!(
                        state.outcome,
                        spreadsheet_collab::calculation::CalculationOutcome::Complete { .. }
                    ) {
                        return Err(SessionError::InvalidCommand);
                    }
                    return Ok::<_, SessionError>(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await??;
        for producers in [1, 8, 32] {
            for due in [false, true] {
                let mut drain = Vec::new();
                let mut enqueue = Vec::new();
                let mut accepted = 0;
                let mut full = 0;
                let mut retries = 0;
                for round in 0..22 {
                    let previous = current.calculation_context().ok_or("missing context")?;
                    let timestamp =
                        previous.effective_timestamp_utc_ms() + if due { 60_000 } else { 59_999 };
                    clock.store(timestamp, Ordering::SeqCst);
                    let result = burst(&session, identity, producers).await?;
                    assert!(result.accepted > 0);
                    let next = session.read(identity).await?;
                    let events = database
                        .load_events(identity, book.document_id(), current.version(), 128)
                        .await?;
                    assert_eq!(events.len(), usize::from(due));
                    assert_eq!(
                        next.version().get(),
                        current.version().get() + u64::from(due)
                    );
                    assert_eq!(
                        next.calculation_context().ok_or("missing context")?.epoch(),
                        previous.epoch() + u64::from(due)
                    );
                    if due {
                        let Operation::AdvanceCalculationEpoch { context } = &events[0].operation
                        else {
                            return Err("unexpected timer event".into());
                        };
                        assert_eq!(context.reason(), CalculationReason::Timer);
                        assert_eq!(context.effective_timestamp_utc_ms(), timestamp);
                        assert_eq!(next.calculation_context(), Some(context));
                    }
                    // Same-clock duplicate burst must not create another durable epoch.
                    burst(&session, identity, producers).await?;
                    assert_eq!(session.read(identity).await?.hash(), next.hash());
                    assert!(
                        database
                            .load_events(identity, book.document_id(), next.version(), 128)
                            .await?
                            .is_empty()
                    );
                    if round >= 2 {
                        drain.push(result.drain_ms);
                        enqueue.extend(result.enqueue_us);
                        accepted += result.accepted;
                        full += result.full;
                        retries += result.probe_retries;
                    }
                    current = next;
                }
                drain.sort_by(f64::total_cmp);
                enqueue.sort_by(f64::total_cmp);
                println!(
                    "timer_admission profile={} producers={producers} due={due} attempts={} accepted={accepted} full={full} barrier_retries={retries} drain_p50_ms={:.3} drain_p95_ms={:.3} drain_max_ms={:.3} enqueue_p50_us={:.3} enqueue_p95_us={:.3}",
                    if heavy { "1000_formulas" } else { "one_now" },
                    producers * 4 * 20,
                    drain[9],
                    drain[18],
                    drain[19],
                    enqueue[(enqueue.len() - 1) / 2],
                    enqueue[(enqueue.len() * 95).div_ceil(100) - 1]
                );
            }
        }
    }
    Ok(())
}
