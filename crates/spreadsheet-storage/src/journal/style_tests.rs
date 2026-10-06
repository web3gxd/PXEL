use super::*;
use spreadsheet_domain::{style::*, style_write::prepare_style_write};
use std::sync::Arc;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn address(book: &Workbook) -> CellAddress {
    CellAddress {
        sheet: book.sheets()[0].id(),
        row: book.sheets()[0].rows()[0],
        column: book.sheets()[0].columns()[0],
    }
}
async fn request(
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
async fn style(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u8,
    change: StyleChange,
) -> TestResult<CommitRequest> {
    let a = address(book);
    let operation = prepare_style_write(
        book,
        &[a],
        std::slice::from_ref(&change),
        StyleWriteKind::Cell,
    )?;
    request(
        db,
        identity,
        book,
        id,
        operation,
        CommandPayload::SetCellStyle {
            address: a,
            changes: vec![change],
        },
    )
    .await
}
async fn inverse(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u8,
    target: &CommittedEvent,
) -> TestResult<CommitRequest> {
    let (kind, writes) = match &target.operation {
        Operation::StyleWrite { kind, writes }
        | Operation::StyleCompensation { kind, writes, .. } => (*kind, writes),
        _ => return Err("invalid test target".into()),
    };
    let operation = Operation::StyleCompensation {
        kind,
        writes: writes
            .iter()
            .map(|w| GuardedStyleWrite {
                address: w.address,
                previous: w.changes.clone(),
                changes: w.previous.clone(),
            })
            .collect(),
        undoes: target.operation_id,
        undoes_version: target.version,
    };
    request(
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
async fn commit(db: &PgDatabase, book: &mut Workbook, r: &CommitRequest) -> TestResult {
    assert!(matches!(db.commit(r).await?, CommitOutcome::Committed(_)));
    book.apply_event(&r.event)?;
    Ok(())
}

async fn collaborator(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    document: DocumentId,
) -> TestResult<AuthenticatedActor> {
    let other = AuthenticatedActor {
        actor_id: ActorId::from_bytes([90; 16])?,
        workspace_id: owner.workspace_id,
    };
    sqlx::query!(
        "INSERT INTO users(user_id) VALUES($1)",
        uuid(other.actor_id.as_bytes())
    )
    .execute(&db.pool)
    .await?;
    sqlx::query!(
        "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')",
        uuid(other.workspace_id.as_bytes()),
        uuid(other.actor_id.as_bytes())
    )
    .execute(&db.pool)
    .await?;
    sqlx::query!("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')",uuid(other.workspace_id.as_bytes()),uuid(document.as_bytes()),uuid(other.actor_id.as_bytes())).execute(&db.pool).await?;
    Ok(other)
}

#[sqlx::test(migrations = false)]
async fn atomic_cell_batch_commits_and_undoes_values_with_styles(pool: sqlx::PgPool) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let target = address(&book);
    let change = StyleChange::Horizontal(Change::Set(HorizontalAlignment::Center));
    let Operation::StyleWrite {
        writes: style_writes,
        ..
    } = prepare_style_write(
        &book,
        &[target],
        std::slice::from_ref(&change),
        StyleWriteKind::Cell,
    )?
    else {
        return Err("missing style preimage".into());
    };
    let authored = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes: vec![GuardedContentWrite {
            address: target,
            content: Some(CellContent::Value(CellValue::Text("header\nvalue".into()))),
            previous: book.cell_content(target)?.cloned(),
        }],
        style_writes,
        undoes: None,
        undoes_version: None,
    };
    let command = CommandPayload::SetCellBatch {
        writes: vec![CellWrite {
            address: target,
            value: CellValue::Text("header\nvalue".into()),
        }],
        styles: vec![(target, vec![change])],
    };
    let original = request(&db, identity, &book, 180, authored, command).await?;
    db.commit(&original)
        .await
        .map_err(|error| format!("authored batch commit failed: {error:?}"))?;
    book.apply_event(&original.event)?;
    assert!(book.sheets()[0].styles()[&target].horizontal() == HorizontalAlignment::Center);

    let Operation::CellBatchWrite {
        content_writes,
        style_writes,
        ..
    } = &original.event.operation
    else {
        return Err("wrong durable event".into());
    };
    let inverse = Operation::CellBatchWrite {
        intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
        content_writes: content_writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
        style_writes: style_writes
            .iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes.clone(),
                changes: write.previous.clone(),
            })
            .collect(),
        undoes: Some(original.event.operation_id),
        undoes_version: Some(original.event.version),
    };
    let undo = request(
        &db,
        identity,
        &book,
        181,
        inverse,
        CommandPayload::UndoCell {
            target_operation_id: original.event.operation_id,
        },
    )
    .await?;
    codec::encode(&undo.event)?;
    codec::event_hash(&undo.event)?;
    let mut tx = db.pool.begin().await?;
    let target_event = undo_target_locked(
        &db,
        &mut tx,
        book.document_id(),
        original.event.operation_id,
        identity.actor_id,
        book.version(),
    )
    .await?;
    style_guard::validate(&db, &mut tx, &undo, book.version(), Some(&target_event))
        .await
        .map_err(|error| format!("batch style undo guard failed: {error:?}"))?;
    guard_undo_tail(
        &db,
        &mut tx,
        book.document_id(),
        &target_event,
        book.version(),
    )
    .await
    .map_err(|error| format!("batch content undo guard failed: {error:?}"))?;
    tx.rollback().await?;
    let mut canonical_command = undo.command.clone();
    canonical_command.operation = CommandPayload::UndoCell {
        target_operation_id: original.event.operation_id,
    };
    assert_eq!(
        codec::command_fingerprint(&undo.command)?,
        codec::command_fingerprint(&canonical_command)?
    );
    let original_writes = target_writes(&target_event, identity.actor_id)?;
    let inverse_writes = target_writes(&undo.event, identity.actor_id)?;
    assert!(inverse_shape_matches(
        &target_event.operation,
        &undo.event.operation
    ));
    assert_eq!(original_writes.len(), inverse_writes.len());
    assert!(
        original_writes
            .iter()
            .zip(&inverse_writes)
            .all(|(a, b)| a.address == b.address
                && a.content == b.previous
                && a.previous == b.content)
    );
    db.commit(&undo)
        .await
        .map_err(|error| format!("batch undo commit failed: {error:?}"))?;
    book.apply_event(&undo.event)?;
    assert!(book.cell_content(target)?.is_none());
    assert!(book.sheets()[0].styles().is_empty());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn styled_range_copy_commits_and_undoes_content_and_all_style_leaves(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let sheet_id = book.sheets()[0].id();
    let added_row = RowId::from_bytes([188; 16])?;
    let extend = request(
        &db,
        identity,
        &book,
        181,
        Operation::ExtendRows {
            sheet: sheet_id,
            rows: vec![added_row],
        },
        CommandPayload::ExtendRows {
            sheet: sheet_id,
            rows: vec![added_row],
        },
    )
    .await?;
    commit(&db, &mut book, &extend).await?;
    let sheet = &book.sheets()[0];
    let source = CellAddress {
        sheet: sheet_id,
        row: sheet.rows()[0],
        column: sheet.columns()[0],
    };
    let destination = CellAddress {
        sheet: sheet_id,
        row: sheet.rows()[1],
        column: sheet.columns()[0],
    };
    let source_style_change = StyleChange::Horizontal(Change::Set(HorizontalAlignment::Right));
    let Operation::StyleWrite { writes, .. } = prepare_style_write(
        &book,
        &[source],
        std::slice::from_ref(&source_style_change),
        StyleWriteKind::Cell,
    )?
    else {
        return Err("missing source style".into());
    };
    let source_style_request = request(
        &db,
        identity,
        &book,
        182,
        Operation::StyleWrite {
            kind: StyleWriteKind::Cell,
            writes,
        },
        CommandPayload::SetCellStyle {
            address: source,
            changes: vec![source_style_change],
        },
    )
    .await?;
    commit(&db, &mut book, &source_style_request).await?;
    let destination_style_change = StyleChange::Fill(Change::Set(Rgb::new([10, 20, 30])));
    let Operation::StyleWrite { writes, .. } = prepare_style_write(
        &book,
        &[destination],
        std::slice::from_ref(&destination_style_change),
        StyleWriteKind::Cell,
    )?
    else {
        return Err("missing destination style".into());
    };
    let destination_style_request = request(
        &db,
        identity,
        &book,
        183,
        Operation::StyleWrite {
            kind: StyleWriteKind::Cell,
            writes,
        },
        CommandPayload::SetCellStyle {
            address: destination,
            changes: vec![destination_style_change],
        },
    )
    .await?;
    commit(&db, &mut book, &destination_style_request).await?;
    let value_request = request(
        &db,
        identity,
        &book,
        184,
        Operation::SetCellValue {
            address: source,
            value: CellValue::Text("rich source".into()),
        },
        CommandPayload::SetCellValue {
            address: source,
            value: CellValue::Text("rich source".into()),
        },
    )
    .await?;
    commit(&db, &mut book, &value_request).await?;
    let old_destination_request = request(
        &db,
        identity,
        &book,
        185,
        Operation::SetCellValue {
            address: destination,
            value: CellValue::Text("old destination".into()),
        },
        CommandPayload::SetCellValue {
            address: destination,
            value: CellValue::Text("old destination".into()),
        },
    )
    .await?;
    commit(&db, &mut book, &old_destination_request).await?;

    let properties = [
        StyleProperty::FontFamily,
        StyleProperty::FontSize,
        StyleProperty::FontBold,
        StyleProperty::FontItalic,
        StyleProperty::FontUnderline,
        StyleProperty::FontStrike,
        StyleProperty::FontColor,
        StyleProperty::Fill,
        StyleProperty::BorderTop,
        StyleProperty::BorderRight,
        StyleProperty::BorderBottom,
        StyleProperty::BorderLeft,
        StyleProperty::Horizontal,
        StyleProperty::Vertical,
        StyleProperty::TextOverflow,
        StyleProperty::NumberFormat,
    ];
    let default = CellStyle::default();
    let source_style = book.sheets()[0].styles().get(&source).unwrap_or(&default);
    let destination_style = book.sheets()[0]
        .styles()
        .get(&destination)
        .unwrap_or(&default);
    let content = book.cell_content(source)?.cloned();
    let copy = Operation::CellBatchWrite {
        intent: CellBatchIntent::CopyRange {
            source,
            destination,
            rows: 1,
            columns: 1,
        },
        content_writes: vec![GuardedContentWrite {
            address: destination,
            content,
            previous: book.cell_content(destination)?.cloned(),
        }],
        style_writes: vec![GuardedStyleWrite {
            address: destination,
            previous: properties
                .iter()
                .map(|property| destination_style.change(*property))
                .collect(),
            changes: properties
                .iter()
                .map(|property| source_style.change(*property))
                .collect(),
        }],
        undoes: None,
        undoes_version: None,
    };
    let copy_request = request(
        &db,
        identity,
        &book,
        186,
        copy,
        CommandPayload::CopyRange {
            source,
            destination,
            rows: 1,
            columns: 1,
            repeat: None,
        },
    )
    .await?;
    db.commit(&copy_request)
        .await
        .map_err(|error| format!("styled copy commit failed: {error:?}"))?;
    assert!(matches!(
        db.commit(&copy_request).await?,
        CommitOutcome::AlreadyCommitted(_)
    ));
    book.apply_event(&copy_request.event)?;
    assert_eq!(book.cell_content(destination)?, book.cell_content(source)?);
    assert_eq!(
        book.sheets()[0].styles().get(&destination),
        book.sheets()[0].styles().get(&source)
    );
    assert_eq!(codec::schema_version(&copy_request.event.operation), 30);

    let Operation::CellBatchWrite {
        intent,
        content_writes,
        style_writes,
        ..
    } = &copy_request.event.operation
    else {
        return Err("copy did not persist as composite batch".into());
    };
    let inverse = Operation::CellBatchWrite {
        intent: intent.clone(),
        content_writes: content_writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
        style_writes: style_writes
            .iter()
            .map(|write| GuardedStyleWrite {
                address: write.address,
                previous: write.changes.clone(),
                changes: write.previous.clone(),
            })
            .collect(),
        undoes: Some(copy_request.event.operation_id),
        undoes_version: Some(copy_request.event.version),
    };
    let undo = request(
        &db,
        identity,
        &book,
        187,
        inverse,
        CommandPayload::UndoCell {
            target_operation_id: copy_request.event.operation_id,
        },
    )
    .await?;
    db.commit(&undo)
        .await
        .map_err(|error| format!("styled copy undo failed: {error:?}"))?;
    book.apply_event(&undo.event)?;
    assert_eq!(
        book.cell_content(destination)?,
        Some(&CellContent::Value(CellValue::Text(
            "old destination".into()
        )))
    );
    assert!(
        book.sheets()[0]
            .styles()
            .get(&destination)
            .is_some_and(|style| style.fill() == Rgb::new([10, 20, 30]))
    );
    assert_eq!(codec::schema_version(&undo.event.operation), 31);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn foreign_equal_default_and_foreign_target_are_hard_barriers(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let original = style(
        &db,
        identity,
        &book,
        50,
        StyleChange::FontBold(Change::Reset),
    )
    .await?;
    commit(&db, &mut book, &original).await?;
    let other = collaborator(&db, identity, book.document_id()).await?;
    assert!(matches!(
        db.load_undo_target(other, book.document_id(), original.event.operation_id)
            .await,
        Err(JournalError::NotUndoable)
    ));
    let equal = style(
        &db,
        other,
        &book,
        51,
        StyleChange::FontBold(Change::Set(false)),
    )
    .await?;
    commit(&db, &mut book, &equal).await?;
    let own_inverse = inverse(&db, identity, &book, 52, &original.event).await?;
    assert!(matches!(
        db.commit(&own_inverse).await,
        Err(JournalError::UndoConflict)
    ));
    let other_inverse = inverse(&db, other, &book, 53, &equal.event).await?;
    commit(&db, &mut book, &other_inverse).await?;
    let own_inverse = inverse(&db, identity, &book, 54, &original.event).await?;
    assert!(matches!(
        db.commit(&own_inverse).await,
        Err(JournalError::UndoConflict)
    ));
    assert!(db.receipt(identity, &own_inverse.command).await?.is_none());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn own_exact_pairs_cancel_but_newer_same_leaf_write_blocks(pool: sqlx::PgPool) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let original = style(
        &db,
        identity,
        &book,
        60,
        StyleChange::FontBold(Change::Set(true)),
    )
    .await?;
    commit(&db, &mut book, &original).await?;
    let equal = style(
        &db,
        identity,
        &book,
        61,
        StyleChange::FontBold(Change::Set(true)),
    )
    .await?;
    commit(&db, &mut book, &equal).await?;
    let blocked = inverse(&db, identity, &book, 62, &original.event).await?;
    assert!(matches!(
        db.commit(&blocked).await,
        Err(JournalError::UndoConflict)
    ));
    let neutralize = inverse(&db, identity, &book, 63, &equal.event).await?;
    commit(&db, &mut book, &neutralize).await?;
    let original_inverse = inverse(&db, identity, &book, 64, &original.event).await?;
    commit(&db, &mut book, &original_inverse).await?;
    assert!(book.sheets()[0].styles().is_empty());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn style_projection_is_required_and_forged_command_cannot_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, book) = super::tests::fixture(pool).await?;
    let mut valid = style(
        &db,
        identity,
        &book,
        40,
        StyleChange::FontBold(Change::Set(true)),
    )
    .await?;
    let projection = valid.inverse_projection.take();
    assert!(db.commit(&valid).await.is_err());
    valid.inverse_projection = projection;
    let payload = valid.command.operation.clone();
    valid.command.operation = CommandPayload::SetCellStyle {
        address: address(&book),
        changes: vec![StyleChange::FontItalic(Change::Set(true))],
    };
    assert!(db.commit(&valid).await.is_err());
    valid.command.operation = payload;
    let state_hash = valid.event.state_hash;
    valid.event.state_hash = [0; 32];
    assert!(matches!(
        db.commit(&valid).await,
        Err(JournalError::Corrupt)
    ));
    valid.event.state_hash = state_hash;
    let fake_book = book.preview_candidate(
        &[Operation::SetCellValue {
            address: address(&book),
            value: CellValue::Number(99.0),
        }],
        book.version(),
    )?;
    valid.inverse_projection = Some(Arc::new(fake_book));
    assert!(matches!(
        db.commit(&valid).await,
        Err(JournalError::UndoConflict)
    ));
    valid.inverse_projection = Some(Arc::new(book.clone()));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(matches!(
        db.commit(&valid).await?,
        CommitOutcome::Committed(_)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn style_undo_commutes_with_content_and_other_leaf_and_replays(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let bold = style(
        &db,
        identity,
        &book,
        41,
        StyleChange::FontBold(Change::Set(true)),
    )
    .await?;
    commit(&db, &mut book, &bold).await?;
    let italic = style(
        &db,
        identity,
        &book,
        42,
        StyleChange::FontItalic(Change::Set(true)),
    )
    .await?;
    commit(&db, &mut book, &italic).await?;
    let a = address(&book);
    let value = request(
        &db,
        identity,
        &book,
        43,
        Operation::SetCellValue {
            address: a,
            value: CellValue::Number(5.0),
        },
        CommandPayload::SetCellValue {
            address: a,
            value: CellValue::Number(5.0),
        },
    )
    .await?;
    commit(&db, &mut book, &value).await?;
    let undo = inverse(&db, identity, &book, 44, &bold.event).await?;
    commit(&db, &mut book, &undo).await?;
    assert!(!book.sheets()[0].styles()[&a].font_bold());
    assert!(book.sheets()[0].styles()[&a].font_italic());
    assert!(matches!(
        db.commit(&bold).await?,
        CommitOutcome::AlreadyCommitted(_)
    ));
    let redo = inverse(&db, identity, &book, 45, &undo.event).await?;
    commit(&db, &mut book, &redo).await?;
    assert!(book.sheets()[0].styles()[&a].font_bold());
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
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn frozen_style_range_undo_redo_preserves_interior_inserted_row(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let first = address(&book);
    let second_row = RowId::from_bytes([121; 16])?;
    let extend = request(
        &db,
        identity,
        &book,
        110,
        Operation::ExtendRows {
            sheet: first.sheet,
            rows: vec![second_row],
        },
        CommandPayload::ExtendRows {
            sheet: first.sheet,
            rows: vec![second_row],
        },
    )
    .await?;
    commit(&db, &mut book, &extend).await?;
    let second = CellAddress {
        row: second_row,
        ..first
    };
    let changes = vec![StyleChange::FontBold(Change::Set(true))];
    let operation = prepare_style_write(&book, &[first, second], &changes, StyleWriteKind::Range)?;
    let authored = request(
        &db,
        identity,
        &book,
        111,
        operation,
        CommandPayload::SetRangeStyle {
            addresses: vec![first, second],
            changes,
        },
    )
    .await?;
    commit(&db, &mut book, &authored).await?;
    let inserted_row = RowId::from_bytes([122; 16])?;
    let operation = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::InsertRowsRebased {
            sheet: first.sheet,
            before: second_row,
            rows: vec![inserted_row],
        },
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    let insertion = request(
        &db,
        identity,
        &book,
        112,
        operation,
        CommandPayload::InsertRows {
            sheet: first.sheet,
            before: second_row,
            rows: vec![inserted_row],
        },
    )
    .await?;
    commit(&db, &mut book, &insertion).await?;
    let middle = CellAddress {
        row: inserted_row,
        ..first
    };
    // Preserve an unrelated property on the inserted row through both inverses.
    let patch = vec![StyleChange::FontItalic(Change::Set(true))];
    let operation = prepare_style_write(&book, &[middle], &patch, StyleWriteKind::Cell)?;
    let middle_style = request(
        &db,
        identity,
        &book,
        113,
        operation,
        CommandPayload::SetCellStyle {
            address: middle,
            changes: patch,
        },
    )
    .await?;
    commit(&db, &mut book, &middle_style).await?;
    let undo = inverse(&db, identity, &book, 114, &authored.event).await?;
    commit(&db, &mut book, &undo).await?;
    assert!(!book.sheets()[0].styles().contains_key(&first));
    assert!(!book.sheets()[0].styles().contains_key(&second));
    assert!(book.sheets()[0].styles()[&middle].font_italic());
    let redo = inverse(&db, identity, &book, 115, &undo.event).await?;
    commit(&db, &mut book, &redo).await?;
    assert!(book.sheets()[0].styles()[&first].font_bold());
    assert!(book.sheets()[0].styles()[&second].font_bold());
    assert!(book.sheets()[0].styles()[&middle].font_italic());
    assert!(!book.sheets()[0].styles()[&middle].font_bold());
    assert!(matches!(
        db.commit(&undo).await?,
        CommitOutcome::AlreadyCommitted(_)
    ));
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
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn removing_inserted_axis_cannot_erase_foreign_style_reset_history(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let a = address(&book);
    let new_row = RowId::from_bytes([91; 16])?;
    let axis = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::InsertRowsRebased {
            sheet: a.sheet,
            before: a.row,
            rows: vec![new_row],
        },
        DocumentVersion::new(2)?,
    )?;
    let axis = request(
        &db,
        identity,
        &book,
        70,
        axis,
        CommandPayload::InsertRows {
            sheet: a.sheet,
            before: a.row,
            rows: vec![new_row],
        },
    )
    .await?;
    commit(&db, &mut book, &axis).await?;
    let other = collaborator(&db, identity, book.document_id()).await?;
    let empty_style = style(&db, other, &book, 71, StyleChange::FontBold(Change::Reset)).await?;
    commit(&db, &mut book, &empty_style).await?;
    assert!(book.sheets()[0].styles().is_empty());
    let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &book,
        &axis.event,
        DocumentVersion::new(4)?,
    )?;
    let inverse = request(
        &db,
        identity,
        &book,
        72,
        inverse,
        CommandPayload::UndoCell {
            target_operation_id: axis.event.operation_id,
        },
    )
    .await?;
    assert!(matches!(
        db.commit(&inverse).await,
        Err(JournalError::UndoConflict)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn axis_inverse_allows_exact_own_style_pair_on_inserted_axis(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let a = address(&book);
    let new_row = RowId::from_bytes([92; 16])?;
    let operation = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::InsertRowsRebased {
            sheet: a.sheet,
            before: a.row,
            rows: vec![new_row],
        },
        DocumentVersion::new(2)?,
    )?;
    let axis = request(
        &db,
        identity,
        &book,
        73,
        operation,
        CommandPayload::InsertRows {
            sheet: a.sheet,
            before: a.row,
            rows: vec![new_row],
        },
    )
    .await?;
    commit(&db, &mut book, &axis).await?;
    let applied = style(
        &db,
        identity,
        &book,
        74,
        StyleChange::FontBold(Change::Set(true)),
    )
    .await?;
    commit(&db, &mut book, &applied).await?;
    let cleared = inverse(&db, identity, &book, 75, &applied.event).await?;
    commit(&db, &mut book, &cleared).await?;
    let operation = spreadsheet_domain::axis_edit::prepare_axis_inverse(
        &book,
        &axis.event,
        DocumentVersion::new(5)?,
    )?;
    let undo = request(
        &db,
        identity,
        &book,
        76,
        operation,
        CommandPayload::UndoCell {
            target_operation_id: axis.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &undo).await?;
    assert_eq!(book.sheets()[0].rows(), &[a.row]);
    Ok(())
}
