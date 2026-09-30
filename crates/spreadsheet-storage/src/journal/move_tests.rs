use super::*;
use spreadsheet_formula::relocation::{MoveRequest, plan_move};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn prepare(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    identifier: u8,
    operation: Operation,
) -> TestResult<CommitRequest> {
    let payload = match &operation {
        Operation::InsertColumnsRebased {
            sheet,
            before,
            columns,
        } => CommandPayload::InsertColumns {
            sheet: *sheet,
            before: *before,
            columns: columns.clone(),
        },
        Operation::SetCellValue { address, value } => CommandPayload::SetCellValue {
            address: *address,
            value: value.clone(),
        },
        Operation::MoveRange {
            source,
            destination,
            rows,
            columns,
            ..
        } => CommandPayload::MoveRange {
            source: *source,
            destination: *destination,
            rows: *rows,
            columns: *columns,
        },
        _ => return Err("unexpected move fixture operation".into()),
    };
    let next = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, next)?;
    let operation_id = OperationId::from_bytes([identifier; 16])?;
    Ok(CommitRequest {
        identity,
        expected_head: book.version(),
        expected_access: db.access(identity, book.document_id()).await?,
        command: Command {
            operation_id,
            document_id: book.document_id(),
            actor_id: identity.actor_id,
            base_version: book.version(),
            client_sequence: u64::from(identifier),
            operation: payload,
        },
        event: CommittedEvent {
            document_id: book.document_id(),
            operation_id,
            principal: EventPrincipal::User(identity.actor_id),
            version: next,
            operation,
            state_hash: candidate.hash(),
        },
    })
}

async fn commit(db: &PgDatabase, book: &mut Workbook, request: &CommitRequest) -> TestResult {
    assert!(matches!(
        db.commit(request).await?,
        CommitOutcome::Committed(_)
    ));
    book.apply_event(&request.event)?;
    Ok(())
}

async fn fixture(
    pool: sqlx::PgPool,
) -> TestResult<(PgDatabase, AuthenticatedActor, Workbook, [CellAddress; 3])> {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let sheet = book.sheets().first().ok_or("missing sheet")?;
    let before = *sheet.columns().first().ok_or("missing column")?;
    let extra = [
        ColumnId::from_bytes([20; 16])?,
        ColumnId::from_bytes([21; 16])?,
    ];
    let addresses = [extra[0], extra[1], before].map(|column| CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[0],
        column,
    });
    let inserted = prepare(
        &db,
        identity,
        &book,
        30,
        Operation::InsertColumnsRebased {
            sheet: sheet.id(),
            before,
            columns: extra.to_vec(),
        },
    )
    .await?;
    commit(&db, &mut book, &inserted).await?;
    let source = prepare(
        &db,
        identity,
        &book,
        31,
        Operation::SetCellValue {
            address: addresses[0],
            value: CellValue::Number(42.0),
        },
    )
    .await?;
    commit(&db, &mut book, &source).await?;
    Ok((db, identity, book, addresses))
}

fn moved(book: &Workbook, source: CellAddress, destination: CellAddress) -> TestResult<Operation> {
    let plan = plan_move(
        book,
        MoveRequest {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
    )?;
    assert_eq!(plan.input_version, book.version());
    assert_eq!(plan.input_hash, book.hash());
    Ok(Operation::MoveRange {
        source,
        destination,
        rows: 1,
        columns: 1,
        writes: plan.writes,
    })
}

#[sqlx::test(migrations = false)]
async fn move_schema19_late_retry_and_paged_replay_use_frozen_event(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, addresses) = fixture(pool).await?;
    let request = prepare(
        &db,
        identity,
        &book,
        32,
        moved(&book, addresses[0], addresses[1])?,
    )
    .await?;
    commit(&db, &mut book, &request).await?;
    assert!(book.cell_content(addresses[0])?.is_none());
    assert_eq!(
        book.cell_content(addresses[1])?,
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    let schema: i32 =
        sqlx::query_scalar("SELECT schema_version FROM document_operations WHERE operation_id=$1")
            .bind(uuid(request.event.operation_id.as_bytes()))
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(schema, 19);
    let later = prepare(
        &db,
        identity,
        &book,
        33,
        Operation::SetCellValue {
            address: addresses[0],
            value: CellValue::Number(99.0),
        },
    )
    .await?;
    commit(&db, &mut book, &later).await?;
    let CommitOutcome::AlreadyCommitted(receipt) = db.commit(&request).await? else {
        return Err("late retry was not deduplicated".into());
    };
    assert_eq!(receipt.committed_version, request.event.version);
    assert_eq!(receipt.current_head_version, book.version());
    assert_eq!(
        receipt.canonical_event_hash,
        codec::event_hash(&request.event)?
    );
    let mut replay = Workbook::empty(book.document_id());
    let mut count = 0;
    while replay.version() < book.version() {
        let events = db
            .load_events(identity, book.document_id(), replay.version(), 2)
            .await?;
        assert!(!events.is_empty() && events.len() <= 2);
        for event in events {
            replay.apply_event(&event)?;
            count += 1;
        }
    }
    assert_eq!(count, book.version().get());
    assert_eq!(replay.hash(), book.hash());
    assert_eq!(
        replay.cell_content(addresses[1])?,
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    let mut reused = request.command.clone();
    if let CommandPayload::MoveRange { destination, .. } = &mut reused.operation {
        *destination = addresses[2];
    }
    assert!(matches!(
        db.receipt(identity, &reused).await,
        Err(JournalError::IdempotencyConflict)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn stale_move_base_and_system_principal_are_rejected_without_events(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, book, addresses) = fixture(pool).await?;
    let mut stale = prepare(
        &db,
        identity,
        &book,
        40,
        moved(&book, addresses[0], addresses[1])?,
    )
    .await?;
    stale.command.base_version = DocumentVersion::new(book.version().get() - 1)?;
    assert_eq!(stale.expected_head, book.version());
    assert!(matches!(
        db.commit(&stale).await,
        Err(JournalError::Conflict)
    ));
    assert!(db.receipt(identity, &stale.command).await?.is_none());
    let mut event = stale.event.clone();
    event.principal = EventPrincipal::System;
    let system = SystemCommitRequest {
        identity,
        expected_access: stale.expected_access,
        expected_head: book.version(),
        event,
    };
    assert!(matches!(
        db.commit_system(&system).await,
        Err(JournalError::Corrupt)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(
        db.load_events(identity, book.document_id(), book.version(), 10)
            .await?
            .is_empty()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_operations WHERE document_id=$1")
            .bind(uuid(book.document_id().as_bytes()))
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(count as u64, book.version().get());
    Ok(())
}
