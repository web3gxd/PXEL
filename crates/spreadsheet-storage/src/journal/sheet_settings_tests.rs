use super::*;
use spreadsheet_collab::{
    DocumentSession, InProcessSessionDirectory, SessionDirectory, Submission,
};
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
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
async fn metadata_and_copy_replay_after_restart_deduplicate_and_reject_forged_patch(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, initial) = super::tests::fixture(pool).await?;
    let directory = InProcessSessionDirectory::new(db.clone());
    let session = directory.session(identity, initial.document_id()).await?;
    let source = initial.sheets()[0].id();
    let row = RowId::from_bytes([11; 16])?;
    write(
        &session,
        identity,
        20,
        CommandPayload::ExtendRows {
            sheet: source,
            rows: vec![row],
        },
    )
    .await?;
    let before = session.read(identity).await?;
    let patch = SheetSettingsPatch {
        name: Some("Renamed".into()),
        default_column_width: Some(180),
        tab_color: Some(Some(0x123456)),
        visibility: vec![(AxisSizeTarget::Row(row), true)],
        ..Default::default()
    };
    let intent = command(
        identity,
        &before,
        21,
        CommandPayload::SetSheetSettings {
            sheet: source,
            changes: patch,
        },
    )?;
    assert!(matches!(
        session.submit(identity, intent.clone()).await?,
        Submission::Committed { .. }
    ));
    assert!(matches!(
        session.submit(identity, intent.clone()).await?,
        Submission::AlreadyCommitted(_)
    ));
    let changed = session.read(identity).await?;
    assert_eq!(changed.version().get(), before.version().get() + 1);
    let copyseed = SheetSeed {
        id: SheetId::from_bytes([12; 16])?,
        name: "Copied".into(),
        rows: vec![RowId::from_bytes([13; 16])?, RowId::from_bytes([14; 16])?],
        columns: vec![ColumnId::from_bytes([15; 16])?],
    };
    write(
        &session,
        identity,
        22,
        CommandPayload::CopySheet {
            source,
            sheet: copyseed.clone(),
            index: 0,
        },
    )
    .await?;
    let copied = session.read(identity).await?;
    assert_eq!(copied.sheets()[0].id(), copyseed.id);
    assert_eq!(copied.sheets()[0].settings().default_column_width, 180);
    assert!(
        copied.sheets()[0]
            .settings()
            .hidden_rows
            .contains(&copyseed.rows[1])
    );
    let restarted = InProcessSessionDirectory::new(db.clone())
        .session(identity, initial.document_id())
        .await?
        .read(identity)
        .await?;
    assert_eq!(restarted.hash(), copied.hash());
    let access = db.access(identity, initial.document_id()).await?;
    let actual = command(
        identity,
        &copied,
        23,
        CommandPayload::SetSheetSettings {
            sheet: source,
            changes: SheetSettingsPatch {
                tab_color: Some(None),
                ..Default::default()
            },
        },
    )?;
    let op = spreadsheet_domain::sheet_settings::prepare(
        &copied,
        source,
        &SheetSettingsPatch {
            default_column_width: Some(190),
            ..Default::default()
        },
    )?;
    let version = DocumentVersion::new(copied.version().get() + 1)?;
    let fake = copied.candidate(&op, version)?;
    let request = CommitRequest {
        identity,
        command: actual,
        event: CommittedEvent {
            document_id: copied.document_id(),
            operation_id: OperationId::from_bytes([23; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version,
            operation: op,
            state_hash: fake.hash(),
        },
        expected_head: copied.version(),
        expected_access: access,
        inverse_projection: Some(std::sync::Arc::new(copied.clone())),
    };
    assert!(matches!(
        db.commit(&request).await,
        Err(JournalError::Corrupt)
    ));
    assert_eq!(
        db.access(identity, initial.document_id()).await?.head,
        copied.version()
    );
    Ok(())
}
#[sqlx::test(migrations = false)]
async fn cell_shift_is_one_durable_version_with_guarded_undo_redo(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, initial) = super::tests::fixture(pool).await?;
    let session = InProcessSessionDirectory::new(db.clone())
        .session(identity, initial.document_id())
        .await?;
    let sheet = initial.sheets()[0].id();
    let original = CellAddress {
        sheet,
        row: initial.sheets()[0].rows()[0],
        column: initial.sheets()[0].columns()[0],
    };
    let row = RowId::from_bytes([11; 16])?;
    write(
        &session,
        identity,
        20,
        CommandPayload::ExtendRows {
            sheet,
            rows: vec![row],
        },
    )
    .await?;
    write(
        &session,
        identity,
        21,
        CommandPayload::SetCellValue {
            address: original,
            value: CellValue::Number(7.),
        },
    )
    .await?;
    let before = session.read(identity).await?;
    let intent = command(
        identity,
        &before,
        22,
        CommandPayload::ShiftCells {
            source: original,
            rows: 1,
            columns: 1,
            axis: CellShiftAxis::Row,
            insert: true,
        },
    )?;
    session.submit(identity, intent.clone()).await?;
    let after = session.read(identity).await?;
    assert_eq!(after.version().get(), before.version().get() + 1);
    assert_eq!(after.cell_content(original)?, None);
    assert_eq!(
        after.cell_content(CellAddress { row, ..original })?,
        Some(&CellContent::Value(CellValue::Number(7.)))
    );
    assert!(matches!(
        session.submit(identity, intent).await?,
        Submission::AlreadyCommitted(_)
    ));
    write(
        &session,
        identity,
        23,
        CommandPayload::UndoCell {
            target_operation_id: OperationId::from_bytes([22; 16])?,
        },
    )
    .await?;
    let restored = session.read(identity).await?;
    assert_eq!(restored.sheets()[0].cells(), before.sheets()[0].cells());
    write(
        &session,
        identity,
        24,
        CommandPayload::UndoCell {
            target_operation_id: OperationId::from_bytes([23; 16])?,
        },
    )
    .await?;
    let repeated = session.read(identity).await?;
    assert_eq!(repeated.sheets()[0].cells(), after.sheets()[0].cells());
    let replayed = InProcessSessionDirectory::new(db.clone())
        .session(identity, initial.document_id())
        .await?
        .read(identity)
        .await?;
    assert_eq!(replayed.hash(), repeated.hash());
    Ok(())
}
