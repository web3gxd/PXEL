use super::*;
use spreadsheet_collab::{
    DocumentSession, InProcessSessionDirectory, SessionDirectory, Submission,
};
use spreadsheet_domain::{
    sort::{SortKey, SortRange},
    style::{Change, Rgb, StyleChange},
};
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[sqlx::test(migrations = false)]
async fn concurrent_sort_commands_from_separate_actors_commit_only_one_current_range(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, original) = super::tests::fixture(pool).await?;
    let directory = InProcessSessionDirectory::new(db.clone());
    let session = directory.session(identity, original.document_id()).await?;
    let row = RowId::from_bytes([11; 16])?;
    let source = CellAddress {
        sheet: original.sheets()[0].id(),
        row: original.sheets()[0].rows()[0],
        column: original.sheets()[0].columns()[0],
    };
    write(
        &session,
        identity,
        20,
        CommandPayload::ExtendRows {
            sheet: source.sheet,
            rows: vec![row],
        },
    )
    .await?;
    write(
        &session,
        identity,
        21,
        CommandPayload::SetCellValue {
            address: source,
            value: CellValue::Number(2.0),
        },
    )
    .await?;
    write(
        &session,
        identity,
        22,
        CommandPayload::SetCellValue {
            address: CellAddress { row, ..source },
            value: CellValue::Number(1.0),
        },
    )
    .await?;
    let before = session.read(identity).await?;
    let second_directory = InProcessSessionDirectory::new(db.clone());
    let second = second_directory
        .session(identity, original.document_id())
        .await?;
    let range = SortRange {
        source,
        rows: 2,
        columns: 1,
        header_rows: 0,
        keys: vec![SortKey {
            column: source.column,
            descending: false,
        }],
    };
    let asc = command(
        identity,
        &before,
        30,
        CommandPayload::SortRange {
            range: range.clone(),
        },
    )?;
    let mut descending = range;
    descending.keys[0].descending = true;
    let desc = command(
        identity,
        &before,
        31,
        CommandPayload::SortRange { range: descending },
    )?;
    let (a, b) = tokio::join!(session.submit(identity, asc), second.submit(identity, desc));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let refused = if a.is_err() { a } else { b };
    assert!(matches!(
        refused,
        Err(spreadsheet_collab::SessionError::SourceVersionChanged)
    ));
    assert_eq!(
        db.access(identity, original.document_id())
            .await?
            .head
            .get(),
        before.version().get() + 1
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_operations WHERE document_id=$1 AND schema_version=37",
    )
    .bind(uuid(original.document_id().as_bytes()))
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(count, 1);
    Ok(())
}

fn command(
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u8,
    operation: CommandPayload,
) -> TestResult<Command> {
    Ok(Command {
        document_id: book.document_id(),
        actor_id: identity.actor_id,
        operation_id: OperationId::from_bytes([id; 16])?,
        base_version: book.version(),
        client_sequence: u64::from(id),
        operation,
    })
}
async fn write(
    session: &DocumentSession,
    identity: AuthenticatedActor,
    id: u8,
    operation: CommandPayload,
) -> TestResult {
    let book = session.read(identity).await?;
    session
        .submit(identity, command(identity, &book, id, operation)?)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn sort_is_one_encrypted_event_and_undo_redo_restore_exact_content_and_style(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, original) = super::tests::fixture(pool).await?;
    let directory = InProcessSessionDirectory::new(db.clone());
    let session = directory.session(identity, original.document_id()).await?;
    let sheet = &original.sheets()[0];
    let rows = vec![
        RowId::from_bytes([11; 16])?,
        RowId::from_bytes([12; 16])?,
        RowId::from_bytes([13; 16])?,
    ];
    write(
        &session,
        identity,
        20,
        CommandPayload::ExtendRows {
            sheet: sheet.id(),
            rows: rows.clone(),
        },
    )
    .await?;
    let source = CellAddress {
        sheet: sheet.id(),
        row: sheet.rows()[0],
        column: sheet.columns()[0],
    };
    write(
        &session,
        identity,
        21,
        CommandPayload::SetCellValue {
            address: source,
            value: CellValue::Text("header-canary".into()),
        },
    )
    .await?;
    for (offset, value) in [3.0, 1.0, 2.0].into_iter().enumerate() {
        write(
            &session,
            identity,
            22 + offset as u8,
            CommandPayload::SetCellValue {
                address: CellAddress {
                    row: rows[offset],
                    ..source
                },
                value: CellValue::Number(value),
            },
        )
        .await?;
    }
    write(
        &session,
        identity,
        25,
        CommandPayload::SetCellStyle {
            address: CellAddress {
                row: rows[0],
                ..source
            },
            changes: vec![StyleChange::FontBold(Change::Set(true))],
        },
    )
    .await?;
    write(
        &session,
        identity,
        26,
        CommandPayload::SetCellStyle {
            address: CellAddress {
                row: rows[1],
                ..source
            },
            changes: vec![StyleChange::Fill(Change::Set(Rgb::new([10, 20, 30])))],
        },
    )
    .await?;
    write(
        &session,
        identity,
        27,
        CommandPayload::SetCellValue {
            address: CellAddress {
                row: rows[0],
                ..source
            },
            value: CellValue::Text("sort-content-canary".into()),
        },
    )
    .await?;
    let before = session.read(identity).await?;
    let range = SortRange {
        source,
        rows: 4,
        columns: 1,
        header_rows: 1,
        keys: vec![SortKey {
            column: source.column,
            descending: false,
        }],
    };
    let plan =
        spreadsheet_formula::sort::plan(&before, &range, &std::collections::BTreeMap::new())?;
    let mut forged = Operation::CellBatchWrite {
        intent: CellBatchIntent::SortRange {
            range: range.clone(),
            order: plan.order.clone(),
        },
        content_writes: plan.content_writes,
        style_writes: spreadsheet_domain::sort::style_writes(&before, &range, &plan.order)?,
        undoes: None,
        undoes_version: None,
    };
    let Operation::CellBatchWrite { content_writes, .. } = &mut forged else {
        return Err("missing batch".into());
    };
    content_writes[0].content = Some(CellContent::Value(CellValue::Number(999.0)));
    let forged_candidate =
        before.candidate(&forged, DocumentVersion::new(before.version().get() + 1)?)?;
    let forged_command = command(
        identity,
        &before,
        29,
        CommandPayload::SortRange {
            range: range.clone(),
        },
    )?;
    let forged_request = CommitRequest {
        identity,
        command: forged_command.clone(),
        expected_head: before.version(),
        expected_access: db.access(identity, before.document_id()).await?,
        inverse_projection: Some(std::sync::Arc::new(before.clone())),
        event: CommittedEvent {
            document_id: before.document_id(),
            operation_id: forged_command.operation_id,
            principal: EventPrincipal::User(identity.actor_id),
            version: forged_candidate.version(),
            operation: forged,
            state_hash: forged_candidate.hash(),
        },
    };
    assert!(matches!(
        db.commit(&forged_request).await,
        Err(JournalError::UndoConflict)
    ));
    assert_eq!(
        db.access(identity, before.document_id()).await?.head,
        before.version()
    );
    let sort_command = command(identity, &before, 30, CommandPayload::SortRange { range })?;
    let Submission::Committed { event, .. } =
        session.submit(identity, sort_command.clone()).await?
    else {
        return Err("sort not committed".into());
    };
    assert_eq!(codec::schema_version(&event.operation), 37);
    let encrypted: Vec<u8> = sqlx::query_scalar(
        "SELECT payload FROM document_operations WHERE document_id=$1 AND operation_id=$2",
    )
    .bind(uuid(original.document_id().as_bytes()))
    .bind(uuid(event.operation_id.as_bytes()))
    .fetch_one(&db.pool)
    .await?;
    assert!(
        !encrypted
            .windows(b"sort-content-canary".len())
            .any(|part| part == b"sort-content-canary")
    );
    let sorted = session.read(identity).await?;
    assert_eq!(sorted.version().get(), before.version().get() + 1);
    assert_eq!(sorted.cell_content(source)?, before.cell_content(source)?);
    assert_eq!(
        sorted.cell_content(CellAddress {
            row: rows[0],
            ..source
        })?,
        Some(&CellContent::Value(CellValue::Number(1.0)))
    );
    assert_eq!(
        sorted.sheets()[0].styles().get(&CellAddress {
            row: rows[2],
            ..source
        }),
        before.sheets()[0].styles().get(&CellAddress {
            row: rows[0],
            ..source
        })
    );
    assert!(matches!(
        session.submit(identity, sort_command).await?,
        Submission::AlreadyCommitted(_)
    ));
    let history = db.undo_history(identity, original.document_id()).await?;
    assert_eq!(
        history.undo.last().map(|target| target.operation_id),
        Some(event.operation_id)
    );
    let undo = command(
        identity,
        &sorted,
        31,
        CommandPayload::UndoCell {
            target_operation_id: event.operation_id,
        },
    )?;
    let Submission::Committed {
        event: undo_event, ..
    } = session.submit(identity, undo).await?
    else {
        return Err("undo not committed".into());
    };
    assert_eq!(codec::schema_version(&undo_event.operation), 38);
    let restored = session.read(identity).await?;
    assert_eq!(restored.sheets()[0].cells(), before.sheets()[0].cells());
    assert_eq!(restored.sheets()[0].styles(), before.sheets()[0].styles());
    session
        .submit(
            identity,
            command(
                identity,
                &restored,
                32,
                CommandPayload::UndoCell {
                    target_operation_id: undo_event.operation_id,
                },
            )?,
        )
        .await?;
    let replay_directory = InProcessSessionDirectory::new(db.clone());
    let replay = replay_directory
        .session(identity, original.document_id())
        .await?
        .read(identity)
        .await?;
    assert_eq!(replay.sheets()[0].cells(), sorted.sheets()[0].cells());
    assert_eq!(replay.sheets()[0].styles(), sorted.sheets()[0].styles());
    assert_eq!(replay.calculation_context(), before.calculation_context());
    let schemas: Vec<i32> = sqlx::query_scalar("SELECT schema_version FROM document_operations WHERE document_id=$1 ORDER BY committed_version").bind(uuid(original.document_id().as_bytes())).fetch_all(&db.pool).await?;
    assert_eq!(&schemas[schemas.len() - 3..], &[37, 38, 38]);
    Ok(())
}
