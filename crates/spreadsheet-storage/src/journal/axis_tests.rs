use super::*;
use spreadsheet_domain::axis_edit::{prepare_axis_edit, prepare_axis_inverse};
use spreadsheet_formula::binder::bind;
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[sqlx::test(migrations = false)]
async fn axis_history_snapshot_object_recovery_preserves_activation_and_admission(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let restored_address = address(&book, sheet, 2, 0)?;
    let observer = address(&book, sheet, 4, 4)?;
    let initial = scalar(&db, identity, &book, 1200, restored_address, 7.).await?;
    commit(&db, &mut book, &initial).await?;
    let reference = formula(&db, identity, &book, 1201, observer, "=A3+SUM(A1:A5)").await?;
    commit(&db, &mut book, &reference).await?;
    let old_version = book.version();
    let original_formula = book.cell_content(observer)?.cloned();
    let deletion = authored_axis(
        &db,
        identity,
        &book,
        1202,
        Operation::DeleteRows {
            sheet,
            rows: vec![restored_address.row],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![restored_address.row],
        },
    )
    .await?;
    commit(&db, &mut book, &deletion).await?;
    let restoration = inverse(&db, identity, &book, 1203, &deletion.event).await?;
    commit(&db, &mut book, &restoration).await?;
    let snapshot_version = book.version();
    let temp = tempfile::tempdir()?;
    let blobs = crate::LocalFilesystemBlobStore::new(
        temp.path().join("blobs"),
        codec::MAX_SNAPSHOT_BYTES as u64,
    )
    .await?;
    db.publish_snapshot(
        &blobs,
        SnapshotId::from_bytes([81; 16])?,
        Arc::new(book.clone()),
        "engine-v1",
        "pxel-v1",
    )
    .await?;
    let independent = address(&book, sheet, 0, 3)?;
    let tail = scalar(&db, identity, &book, 1204, independent, 9.).await?;
    commit(&db, &mut book, &tail).await?;
    let recovery = Box::pin(db.recover_document(&blobs, identity, book.document_id())).await?;
    assert_eq!(recovery.snapshot_version, Some(snapshot_version));
    assert_eq!(recovery.replayed_events, 1);
    let mut recovered = recovery.workbook;
    assert_eq!(recovered.hash(), book.hash());
    assert_eq!(recovered.version(), book.version());
    assert_eq!(recovered.cell_content(observer)?.cloned(), original_formula);
    assert_eq!(
        recovered.cell_content(restored_address)?,
        book.cell_content(restored_address)?
    );
    assert_eq!(
        recovered.cell_content(independent)?,
        book.cell_content(independent)?
    );
    assert_eq!(
        recovered.row_activation_floor(restored_address.row),
        snapshot_version
    );
    assert_eq!(
        spreadsheet_domain::lifecycle::max_activation_version(&recovered),
        snapshot_version
    );
    let mut stale = scalar(&db, identity, &recovered, 1205, restored_address, 11.).await?;
    stale.command.base_version = old_version;
    assert!(matches!(
        db.commit(&stale).await,
        Err(JournalError::Conflict)
    ));
    unchanged(&db, identity, &recovered, &stale).await?;
    let current = scalar(&db, identity, &recovered, 1206, restored_address, 12.).await?;
    commit(&db, &mut recovered, &current).await?;
    assert_eq!(
        recovered.cell_content(restored_address)?,
        Some(&CellContent::Value(CellValue::Number(12.)))
    );
    Ok(())
}

async fn prepare(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    operation: Operation,
    payload: CommandPayload,
) -> TestResult<CommitRequest> {
    let version = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, version)?;
    let operation_id = OperationId::from_bytes(id.to_be_bytes())?;
    Ok(CommitRequest {
        inverse_projection: Some(Arc::new(book.clone())),
        identity,
        expected_head: book.version(),
        expected_access: db.access(identity, book.document_id()).await?,
        command: Command {
            document_id: book.document_id(),
            operation_id,
            actor_id: identity.actor_id,
            base_version: book.version(),
            client_sequence: id as u64,
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
    assert!(matches!(
        db.commit(request).await?,
        CommitOutcome::Committed(_)
    ));
    book.apply_event(&request.event)?;
    Ok(())
}
async fn fixture(
    pool: sqlx::PgPool,
) -> TestResult<(PgDatabase, AuthenticatedActor, Workbook, SheetId)> {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let sheet = SheetSeed {
        id: SheetId::from_bytes([40; 16])?,
        name: "Axes".into(),
        rows: (0..5u128)
            .map(|n| RowId::from_bytes((100 + n).to_be_bytes()))
            .collect::<Result<_, _>>()?,
        columns: (0..5u128)
            .map(|n| ColumnId::from_bytes((200 + n).to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let sheet_id = sheet.id;
    let added = prepare(
        &db,
        identity,
        &book,
        1000,
        Operation::AddSheet {
            sheet: sheet.clone(),
        },
        CommandPayload::AddSheet { sheet },
    )
    .await?;
    commit(&db, &mut book, &added).await?;
    Ok((db, identity, book, sheet_id))
}
fn address(book: &Workbook, sheet: SheetId, row: usize, column: usize) -> TestResult<CellAddress> {
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == sheet)
        .ok_or("missing axes fixture")?;
    Ok(CellAddress {
        sheet: sheet.id(),
        row: *sheet.rows().get(row).ok_or("missing row")?,
        column: *sheet.columns().get(column).ok_or("missing column")?,
    })
}
async fn authored_axis(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    legacy: Operation,
    payload: CommandPayload,
) -> TestResult<CommitRequest> {
    let operation = prepare_axis_edit(
        book,
        &legacy,
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    prepare(db, identity, book, id, operation, payload).await
}
async fn inverse(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    target: &CommittedEvent,
) -> TestResult<CommitRequest> {
    let operation = prepare_axis_inverse(
        book,
        target,
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    prepare(
        db,
        identity,
        book,
        id,
        operation,
        CommandPayload::UndoCell {
            target_operation_id: target.operation_id,
        },
    )
    .await
}
async fn scalar(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    address: CellAddress,
    number: f64,
) -> TestResult<CommitRequest> {
    let value = CellValue::Number(number);
    prepare(
        db,
        identity,
        book,
        id,
        Operation::CellWrite {
            address,
            content: Some(CellContent::Value(value.clone())),
            previous: book.cell_content(address)?.cloned(),
            kind: Some(CellWriteKind::SetValue),
            undoes: None,
            undoes_version: None,
        },
        CommandPayload::SetCellValue { address, value },
    )
    .await
}
async fn formula(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    address: CellAddress,
    source: &str,
) -> TestResult<CommitRequest> {
    let formula = bind(source, book, address)?;
    prepare(
        db,
        identity,
        book,
        id,
        Operation::CellWrite {
            address,
            content: Some(CellContent::Formula(formula)),
            previous: book.cell_content(address)?.cloned(),
            kind: Some(CellWriteKind::SetFormula),
            undoes: None,
            undoes_version: None,
        },
        CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new(source.into())?,
        },
    )
    .await
}
async fn unchanged(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    request: &CommitRequest,
) -> TestResult {
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(identity, &request.command).await?.is_none());
    Ok(())
}
async fn foreign(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    document: DocumentId,
) -> TestResult<AuthenticatedActor> {
    let actor = ActorId::from_bytes([45; 16])?;
    sqlx::query("INSERT INTO users(user_id) VALUES($1)")
        .bind(uuid(actor.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
        .bind(uuid(owner.workspace_id.as_bytes()))
        .bind(uuid(actor.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')").bind(uuid(owner.workspace_id.as_bytes())).bind(uuid(document.as_bytes())).bind(uuid(actor.as_bytes())).execute(&db.pool).await?;
    Ok(AuthenticatedActor {
        actor_id: actor,
        workspace_id: owner.workspace_id,
    })
}

#[sqlx::test(migrations = false)]
async fn axis_history_insert_undo_redo_preserves_independent_edits_retry_and_replay(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let original_rows = book
        .sheets()
        .iter()
        .find(|s| s.id() == sheet)
        .ok_or("sheet")?
        .rows()
        .to_vec();
    let before = original_rows[2];
    let inserted = RowId::from_bytes([46; 16])?;
    let authored = authored_axis(
        &db,
        identity,
        &book,
        1001,
        Operation::InsertRowsRebased {
            sheet,
            before,
            rows: vec![inserted],
        },
        CommandPayload::InsertRows {
            sheet,
            before,
            rows: vec![inserted],
        },
    )
    .await?;
    commit(&db, &mut book, &authored).await?;
    let independent = address(&book, sheet, 0, 4)?;
    let edit = scalar(&db, identity, &book, 1002, independent, 19.).await?;
    commit(&db, &mut book, &edit).await?;
    let undo = inverse(&db, identity, &book, 1003, &authored.event).await?;
    commit(&db, &mut book, &undo).await?;
    assert_eq!(
        book.sheets()
            .iter()
            .find(|s| s.id() == sheet)
            .ok_or("sheet")?
            .rows(),
        original_rows
    );
    assert_eq!(
        book.cell_content(independent)?,
        Some(&CellContent::Value(CellValue::Number(19.)))
    );
    let redo = inverse(&db, identity, &book, 1004, &undo.event).await?;
    commit(&db, &mut book, &redo).await?;
    let CommitOutcome::AlreadyCommitted(receipt) = db.commit(&undo).await? else {
        return Err("lost inverse receipt".into());
    };
    assert_eq!(receipt.committed_version, undo.event.version);
    assert_eq!(receipt.current_head_version, book.version());
    let marker: i64 =
        sqlx::query_scalar("SELECT axis_activation_version FROM documents WHERE document_id=$1")
            .bind(uuid(book.document_id().as_bytes()))
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(marker as u64, redo.event.version.get());
    let mut replay = Workbook::empty(book.document_id());
    while replay.version() < book.version() {
        let events = db
            .load_events(identity, book.document_id(), replay.version(), 2)
            .await?;
        assert!(!events.is_empty());
        for event in events {
            replay.apply_event(&event)?;
        }
    }
    assert_eq!(replay.hash(), book.hash());
    assert_eq!(
        spreadsheet_domain::lifecycle::max_activation_version(&replay),
        redo.event.version
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_delete_restores_content_scalar_ref_range_ast_and_retired_identity(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let deleted = address(&book, sheet, 2, 0)?;
    let set = scalar(&db, identity, &book, 1010, deleted, 7.).await?;
    commit(&db, &mut book, &set).await?;
    let observer = address(&book, sheet, 4, 4)?;
    let set_formula = formula(&db, identity, &book, 1011, observer, "=A3+SUM(A1:A5)").await?;
    commit(&db, &mut book, &set_formula).await?;
    let original_formula = book.cell_content(observer)?.cloned();
    let remove = authored_axis(
        &db,
        identity,
        &book,
        1012,
        Operation::DeleteRows {
            sheet,
            rows: vec![deleted.row],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![deleted.row],
        },
    )
    .await?;
    commit(&db, &mut book, &remove).await?;
    let undo = inverse(&db, identity, &book, 1013, &remove.event).await?;
    commit(&db, &mut book, &undo).await?;
    assert_eq!(
        book.cell_content(deleted)?,
        Some(&CellContent::Value(CellValue::Number(7.)))
    );
    assert_eq!(book.cell_content(observer)?.cloned(), original_formula);
    assert_eq!(book.row_activation_floor(deleted.row), undo.event.version);
    let snapshot = Workbook::restore_snapshot(book.snapshot_image()?, book.hash())?;
    assert_eq!(snapshot.hash(), book.hash());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_new_foreign_range_in_restored_interior_blocks_even_after_clear(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let other = foreign(&db, identity, book.document_id()).await?;
    let removed = address(&book, sheet, 2, 0)?.row;
    let remove = authored_axis(
        &db,
        identity,
        &book,
        1020,
        Operation::DeleteRows {
            sheet,
            rows: vec![removed],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![removed],
        },
    )
    .await?;
    commit(&db, &mut book, &remove).await?;
    let observer = address(&book, sheet, 3, 4)?;
    let new = formula(&db, other, &book, 1021, observer, "=SUM(A4:A1)").await?;
    commit(&db, &mut book, &new).await?;
    let inverse_request = inverse(&db, identity, &book, 1022, &remove.event).await?;
    assert!(matches!(
        db.commit(&inverse_request).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &inverse_request).await?;
    let clear = prepare(
        &db,
        other,
        &book,
        1023,
        Operation::CellWrite {
            address: observer,
            content: None,
            previous: book.cell_content(observer)?.cloned(),
            kind: Some(CellWriteKind::Clear),
            undoes: None,
            undoes_version: None,
        },
        CommandPayload::ClearCell { address: observer },
    )
    .await?;
    commit(&db, &mut book, &clear).await?;
    let retry = inverse(&db, identity, &book, 1024, &remove.event).await?;
    assert!(matches!(
        db.commit(&retry).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &retry).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_exact_own_new_reference_inverse_neutralizes(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let removed = address(&book, sheet, 2, 0)?.row;
    let remove = authored_axis(
        &db,
        identity,
        &book,
        1030,
        Operation::DeleteRows {
            sheet,
            rows: vec![removed],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![removed],
        },
    )
    .await?;
    commit(&db, &mut book, &remove).await?;
    let observer = address(&book, sheet, 3, 4)?;
    let new = formula(&db, identity, &book, 1031, observer, "=SUM(A1:A4)").await?;
    commit(&db, &mut book, &new).await?;
    let Operation::CellWrite {
        content, previous, ..
    } = &new.event.operation
    else {
        return Err("not cell".into());
    };
    let compensated = prepare(
        &db,
        identity,
        &book,
        1032,
        Operation::CellWrite {
            address: observer,
            content: previous.clone(),
            previous: content.clone(),
            kind: None,
            undoes: Some(new.event.operation_id),
            undoes_version: Some(new.event.version),
        },
        CommandPayload::UndoCell {
            target_operation_id: new.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &compensated).await?;
    let undo = inverse(&db, identity, &book, 1033, &remove.event).await?;
    commit(&db, &mut book, &undo).await?;
    assert!(
        book.sheets()
            .iter()
            .find(|s| s.id() == sheet)
            .ok_or("sheet")?
            .rows()
            .contains(&removed)
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_reactivated_none_context_and_delayed_value_are_rejected(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let revived = address(&book, sheet, 2, 0)?;
    let old_version = book.version();
    let remove = authored_axis(
        &db,
        identity,
        &book,
        1040,
        Operation::DeleteRows {
            sheet,
            rows: vec![revived.row],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![revived.row],
        },
    )
    .await?;
    commit(&db, &mut book, &remove).await?;
    let undo = inverse(&db, identity, &book, 1041, &remove.event).await?;
    commit(&db, &mut book, &undo).await?;
    let mut missing = scalar(&db, identity, &book, 1042, revived, 1.).await?;
    missing.inverse_projection = None;
    assert!(matches!(
        db.commit(&missing).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &missing).await?;
    let mut delayed = scalar(&db, identity, &book, 1043, revived, 2.).await?;
    delayed.command.base_version = old_version;
    assert!(matches!(
        db.commit(&delayed).await,
        Err(JournalError::Conflict)
    ));
    unchanged(&db, identity, &book, &delayed).await?;
    let independent = address(&book, sheet, 0, 4)?;
    let mut allowed = scalar(&db, identity, &book, 1044, independent, 3.).await?;
    allowed.command.base_version = old_version;
    commit(&db, &mut book, &allowed).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_stale_copy_with_revived_interior_is_rejected(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let source = address(&book, sheet, 0, 0)?;
    let destination = address(&book, sheet, 0, 3)?;
    let removed = address(&book, sheet, 2, 0)?.row;
    let old_version = book.version();
    let deletion = authored_axis(
        &db,
        identity,
        &book,
        1090,
        Operation::DeleteRows {
            sheet,
            rows: vec![removed],
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: vec![removed],
        },
    )
    .await?;
    commit(&db, &mut book, &deletion).await?;
    let restore = inverse(&db, identity, &book, 1091, &deletion.event).await?;
    commit(&db, &mut book, &restore).await?;
    let writes = (0..5)
        .map(|row| {
            Ok(GuardedContentWrite {
                address: address(&book, sheet, row, 3)?,
                content: None,
                previous: None,
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let mut copy = prepare(
        &db,
        identity,
        &book,
        1092,
        Operation::RangeWrite {
            writes,
            kind: Some(RangeWriteKind::Copy),
            undoes: None,
            undoes_version: None,
        },
        CommandPayload::CopyRange {
            source,
            destination,
            rows: 5,
            columns: 1,
            repeat: None,
        },
    )
    .await?;
    copy.command.base_version = old_version;
    assert!(matches!(
        db.commit(&copy).await,
        Err(JournalError::Conflict)
    ));
    unchanged(&db, identity, &book, &copy).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_forged_original_and_inverse_projection_fail_atomically(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let before = address(&book, sheet, 2, 0)?.row;
    let inserted = RowId::from_bytes([48; 16])?;
    let mut request = authored_axis(
        &db,
        identity,
        &book,
        1050,
        Operation::InsertRowsRebased {
            sheet,
            before,
            rows: vec![inserted],
        },
        CommandPayload::InsertRows {
            sheet,
            before,
            rows: vec![inserted],
        },
    )
    .await?;
    request.event.state_hash = [0; 32];
    assert!(matches!(
        db.commit(&request).await,
        Err(JournalError::Corrupt)
    ));
    unchanged(&db, identity, &book, &request).await?;
    request.event.state_hash = book
        .candidate(&request.event.operation, request.event.version)?
        .hash();
    commit(&db, &mut book, &request).await?;
    let mut undo = inverse(&db, identity, &book, 1051, &request.event).await?;
    undo.inverse_projection = None;
    assert!(matches!(
        db.commit(&undo).await,
        Err(JournalError::UndoConflict) | Err(JournalError::Corrupt)
    ));
    unchanged(&db, identity, &book, &undo).await?;
    undo.inverse_projection = request.inverse_projection.clone();
    assert!(matches!(
        db.commit(&undo).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &undo).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_own_noncontiguous_axis_pair_allows_earlier_axis_inverse(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let before = address(&book, sheet, 1, 0)?.row;
    let inserted = RowId::from_bytes([49; 16])?;
    let first = authored_axis(
        &db,
        identity,
        &book,
        1060,
        Operation::InsertRowsRebased {
            sheet,
            before,
            rows: vec![inserted],
        },
        CommandPayload::InsertRows {
            sheet,
            before,
            rows: vec![inserted],
        },
    )
    .await?;
    commit(&db, &mut book, &first).await?;
    let removed = vec![
        address(&book, sheet, 4, 0)?.row,
        address(&book, sheet, 2, 0)?.row,
    ];
    let second = authored_axis(
        &db,
        identity,
        &book,
        1061,
        Operation::DeleteRows {
            sheet,
            rows: removed.clone(),
        },
        CommandPayload::DeleteRows {
            sheet,
            rows: removed,
        },
    )
    .await?;
    commit(&db, &mut book, &second).await?;
    let undo_second = inverse(&db, identity, &book, 1062, &second.event).await?;
    commit(&db, &mut book, &undo_second).await?;
    let undo_first = inverse(&db, identity, &book, 1063, &first.event).await?;
    commit(&db, &mut book, &undo_first).await?;
    assert!(
        !book
            .sheets()
            .iter()
            .find(|s| s.id() == sheet)
            .ok_or("sheet")?
            .rows()
            .contains(&inserted)
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_foreign_value_and_its_own_inverse_never_neutralize_inserted_axis_touch(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let other = foreign(&db, identity, book.document_id()).await?;
    let before = address(&book, sheet, 2, 0)?.row;
    let inserted = RowId::from_bytes([50; 16])?;
    let authored = authored_axis(
        &db,
        identity,
        &book,
        1070,
        Operation::InsertRowsRebased {
            sheet,
            before,
            rows: vec![inserted],
        },
        CommandPayload::InsertRows {
            sheet,
            before,
            rows: vec![inserted],
        },
    )
    .await?;
    commit(&db, &mut book, &authored).await?;
    let target = CellAddress {
        sheet,
        row: inserted,
        column: address(&book, sheet, 0, 0)?.column,
    };
    let set = scalar(&db, other, &book, 1071, target, 1.).await?;
    commit(&db, &mut book, &set).await?;
    let Operation::CellWrite {
        content, previous, ..
    } = &set.event.operation
    else {
        return Err("not cell".into());
    };
    let foreign_inverse = prepare(
        &db,
        other,
        &book,
        1072,
        Operation::CellWrite {
            address: target,
            content: previous.clone(),
            previous: content.clone(),
            kind: None,
            undoes: Some(set.event.operation_id),
            undoes_version: Some(set.event.version),
        },
        CommandPayload::UndoCell {
            target_operation_id: set.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &foreign_inverse).await?;
    assert!(book.cell_content(target)?.is_none());
    let attempted = inverse(&db, identity, &book, 1073, &authored.event).await?;
    assert!(matches!(
        db.commit(&attempted).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &attempted).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_column_restore_cross_sheet_range_interior_is_guarded(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let other = foreign(&db, identity, book.document_id()).await?;
    let removed = address(&book, sheet, 0, 2)?.column;
    let remove = authored_axis(
        &db,
        identity,
        &book,
        1080,
        Operation::DeleteColumns {
            sheet,
            columns: vec![removed],
        },
        CommandPayload::DeleteColumns {
            sheet,
            columns: vec![removed],
        },
    )
    .await?;
    commit(&db, &mut book, &remove).await?;
    let first = book.sheets().first().ok_or("sheet")?;
    let observer = CellAddress {
        sheet: first.id(),
        row: first.rows()[0],
        column: first.columns()[0],
    };
    let authored = formula(&db, other, &book, 1081, observer, "=SUM('Axes'!D1:A1)").await?;
    commit(&db, &mut book, &authored).await?;
    let attempted = inverse(&db, identity, &book, 1082, &remove.event).await?;
    assert!(matches!(
        db.commit(&attempted).await,
        Err(JournalError::UndoConflict)
    ));
    unchanged(&db, identity, &book, &attempted).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_oversized_removed_preimages_refuse_original_before_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let seed = SheetSeed {
        id: SheetId::from_bytes([55; 16])?,
        name: "Quota".into(),
        rows: vec![RowId::from_bytes([56; 16])?],
        columns: (0..18u128)
            .map(|n| ColumnId::from_bytes((600 + n).to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let added = prepare(
        &db,
        identity,
        &book,
        1090,
        Operation::AddSheet {
            sheet: seed.clone(),
        },
        CommandPayload::AddSheet {
            sheet: seed.clone(),
        },
    )
    .await?;
    commit(&db, &mut book, &added).await?;
    for (id, range) in [(1091, 0..16), (1092, 16..17)] {
        let writes = range
            .map(|index| GuardedContentWrite {
                address: CellAddress {
                    sheet: seed.id,
                    row: seed.rows[0],
                    column: seed.columns[index],
                },
                content: Some(CellContent::Value(CellValue::Text("x".repeat(32 * 1024)))),
                previous: None,
            })
            .collect::<Vec<_>>();
        let payload = CommandPayload::SetCellValues {
            writes: writes
                .iter()
                .map(|w| CellWrite {
                    address: w.address,
                    value: CellValue::Text("x".repeat(32 * 1024)),
                })
                .collect(),
        };
        let set = prepare(
            &db,
            identity,
            &book,
            id,
            Operation::RangeWrite {
                writes,
                kind: Some(RangeWriteKind::SetValues),
                undoes: None,
                undoes_version: None,
            },
            payload,
        )
        .await?;
        commit(&db, &mut book, &set).await?;
    }
    let removed = seed.columns[..17].to_vec();
    let legacy = Operation::DeleteColumns {
        sheet: seed.id,
        columns: removed.clone(),
    };
    let next = DocumentVersion::new(book.version().get() + 1)?;
    assert!(matches!(
        prepare_axis_edit(&book, &legacy, next),
        Err(spreadsheet_domain::axis_edit::AxisEditError::Domain(
            DomainError::LimitExceeded
        ))
    ));
    // Submit deliberately oversized frozen preimages from the real committed
    // workbook. Journal derivation must reject before inserting any event.
    let delta = AxisDelta::Columns {
        removed: removed.clone(),
        inserted: vec![],
    };
    let candidate = book.candidate(&legacy, next)?;
    let change = AxisChange {
        sheet: seed.id,
        before_order_hash: spreadsheet_domain::axis_edit::axis_order_hash(&book, seed.id, &delta)?,
        after_order_hash: spreadsheet_domain::axis_edit::axis_order_hash(
            &candidate, seed.id, &delta,
        )?,
        inverse_delta: AxisDelta::Columns {
            removed: vec![],
            inserted: removed
                .iter()
                .map(|column| ColumnPlacement {
                    column: *column,
                    before: Some(seed.columns[17]),
                })
                .collect(),
        },
        delta,
        writes: removed
            .iter()
            .map(|column| {
                let address = CellAddress {
                    sheet: seed.id,
                    row: seed.rows[0],
                    column: *column,
                };
                Ok(GuardedContentWrite {
                    address,
                    content: None,
                    previous: book.cell_content(address)?.cloned(),
                })
            })
            .collect::<Result<_, DomainError>>()?,
        style_writes: vec![],
    };
    let operation_id = OperationId::from_bytes(1093u128.to_be_bytes())?;
    let request = CommitRequest {
        inverse_projection: Some(Arc::new(book.clone())),
        identity,
        expected_head: book.version(),
        expected_access: db.access(identity, book.document_id()).await?,
        command: Command {
            document_id: book.document_id(),
            actor_id: identity.actor_id,
            operation_id,
            base_version: book.version(),
            client_sequence: 1093,
            operation: CommandPayload::DeleteColumns {
                sheet: seed.id,
                columns: removed,
            },
        },
        event: CommittedEvent {
            document_id: book.document_id(),
            operation_id,
            principal: EventPrincipal::User(identity.actor_id),
            version: next,
            operation: Operation::AxisEdit { change },
            state_hash: candidate.hash(),
        },
    };
    assert!(matches!(
        db.commit(&request).await,
        Err(JournalError::PayloadTooLarge)
    ));
    unchanged(&db, identity, &book, &request).await?;
    assert_eq!(
        book.sheets()
            .iter()
            .find(|s| s.id() == seed.id)
            .ok_or("sheet")?
            .columns()
            .len(),
        18
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_history_neutralized_axes_allow_earlier_formula_undo_with_new_offset_extras(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, sheet) = fixture(pool).await?;
    let original_observer = address(&book, sheet, 4, 4)?;
    let authored_formula = formula(&db, identity, &book, 1100, original_observer, "=A1+1").await?;
    commit(&db, &mut book, &authored_formula).await?;
    let before = address(&book, sheet, 1, 0)?.row;
    let new_observer = address(&book, sheet, 3, 3)?;
    let inserted = RowId::from_bytes([57; 16])?;
    let axis = authored_axis(
        &db,
        identity,
        &book,
        1101,
        Operation::InsertRowsRebased {
            sheet,
            before,
            rows: vec![inserted],
        },
        CommandPayload::InsertRows {
            sheet,
            before,
            rows: vec![inserted],
        },
    )
    .await?;
    commit(&db, &mut book, &axis).await?;
    let introduced = formula(&db, identity, &book, 1102, new_observer, "=A1+2").await?;
    commit(&db, &mut book, &introduced).await?;
    let inverse_axis = inverse(&db, identity, &book, 1103, &axis.event).await?;
    let Operation::AxisCompensation { change, .. } = &inverse_axis.event.operation else {
        return Err("not axisinverse".into());
    };
    assert!(
        change.writes.iter().any(|w| w.address == new_observer),
        "canonical inverse must include the new independent formula offset patch"
    );
    commit(&db, &mut book, &inverse_axis).await?;
    let Operation::CellWrite {
        content, previous, ..
    } = &authored_formula.event.operation
    else {
        return Err("not cell".into());
    };
    let inverse_formula = prepare(
        &db,
        identity,
        &book,
        1104,
        Operation::CellWrite {
            address: original_observer,
            content: previous.clone(),
            previous: content.clone(),
            kind: None,
            undoes: Some(authored_formula.event.operation_id),
            undoes_version: Some(authored_formula.event.version),
        },
        CommandPayload::UndoCell {
            target_operation_id: authored_formula.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &inverse_formula).await?;
    assert!(book.cell_content(original_observer)?.is_none());
    assert!(matches!(
        book.cell_content(new_observer)?,
        Some(CellContent::Formula(_))
    ));
    Ok(())
}
