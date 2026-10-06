use super::*;
use spreadsheet_domain::MergeChange;
use std::sync::Arc;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn prepared(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u8,
    operation: Operation,
    payload: CommandPayload,
) -> TestResult<CommitRequest> {
    let version = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, version)?;
    let operation_id = OperationId::from_bytes([id; 16])?;
    Ok(CommitRequest {
        identity,
        expected_head: book.version(),
        expected_access: db.access(identity, book.document_id()).await?,
        inverse_projection: Some(Arc::new(book.clone())),
        command: Command {
            document_id: book.document_id(),
            operation_id,
            actor_id: identity.actor_id,
            base_version: book.version(),
            client_sequence: u64::from(id),
            operation: payload,
        },
        event: CommittedEvent {
            document_id: book.document_id(),
            operation_id,
            principal: EventPrincipal::User(identity.actor_id),
            version,
            operation,
            state_hash: candidate.hash(),
        },
    })
}

async fn commit(db: &PgDatabase, book: &mut Workbook, request: &CommitRequest) -> TestResult {
    db.commit(request).await.map_err(|error| {
        format!(
            "durable commit failed: {error:?}; op={:?}",
            request.event.operation
        )
    })?;
    book.apply_event(&request.event)?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn merge_history_commits_undo_redo_and_replays(pool: sqlx::PgPool) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let sheet = book.sheets()[0].id();
    let added_row = RowId::from_bytes([31; 16])?;
    let extend = prepared(
        &db,
        identity,
        &book,
        32,
        Operation::ExtendRows {
            sheet,
            rows: vec![added_row],
        },
        CommandPayload::ExtendRows {
            sheet,
            rows: vec![added_row],
        },
    )
    .await?;
    commit(&db, &mut book, &extend).await?;
    let range = spreadsheet_domain::MergeRange::new(
        book.sheets()[0].rows()[0],
        added_row,
        book.sheets()[0].columns()[0],
        book.sheets()[0].columns()[0],
    );
    let merge = Operation::MergeWrite {
        sheet,
        change: MergeChange {
            range,
            merged: true,
            previous: false,
        },
        undoes: None,
        undoes_version: None,
    };
    let target = prepared(
        &db,
        identity,
        &book,
        33,
        merge,
        CommandPayload::SetMergeRange {
            sheet,
            range,
            merged: true,
        },
    )
    .await?;
    commit(&db, &mut book, &target).await?;
    assert!(book.sheets()[0].merges().contains(&range));

    let inverse = |target: &CommittedEvent, merged: bool| -> TestResult<Operation> {
        let Operation::MergeWrite { sheet, change, .. } = &target.operation else {
            return Err("target is not a merge event".into());
        };
        Ok(Operation::MergeWrite {
            sheet: *sheet,
            change: MergeChange {
                range: change.range,
                merged,
                previous: change.merged,
            },
            undoes: Some(target.operation_id),
            undoes_version: Some(target.version),
        })
    };
    let undo_operation = inverse(&target.event, false)?;
    let undo = prepared(
        &db,
        identity,
        &book,
        34,
        undo_operation,
        CommandPayload::UndoCell {
            target_operation_id: target.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &undo).await?;
    assert!(!book.sheets()[0].merges().contains(&range));

    let redo_operation = inverse(&undo.event, true)?;
    let redo = prepared(
        &db,
        identity,
        &book,
        35,
        redo_operation,
        CommandPayload::UndoCell {
            target_operation_id: undo.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &redo).await?;
    assert!(book.sheets()[0].merges().contains(&range));
    assert!(matches!(
        db.commit(&undo).await?,
        CommitOutcome::AlreadyCommitted(_)
    ));

    let mut replay = Workbook::empty(book.document_id());
    loop {
        let events = db
            .load_events(identity, book.document_id(), replay.version(), 16)
            .await?;
        if events.is_empty() {
            break;
        }
        for event in events {
            replay.apply_event(&event)?;
        }
    }
    assert_eq!(replay.hash(), book.hash());
    Ok(())
}
