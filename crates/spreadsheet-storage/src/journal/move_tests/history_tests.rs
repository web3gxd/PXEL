use super::*;

fn inverse(target: &CommittedEvent) -> TestResult<Operation> {
    let (source, destination, rows, columns, writes) = match &target.operation {
        Operation::MoveRange {
            source,
            destination,
            rows,
            columns,
            writes,
        }
        | Operation::MoveCompensation {
            source,
            destination,
            rows,
            columns,
            writes,
            ..
        } => (*source, *destination, *rows, *columns, writes),
        _ => return Err("not a move history target".into()),
    };
    Ok(Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes: writes
            .iter()
            .map(|write| GuardedContentWrite {
                address: write.address,
                content: write.previous.clone(),
                previous: write.content.clone(),
            })
            .collect(),
        undoes: target.operation_id,
        undoes_version: target.version,
    })
}
async fn foreign(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    book: &Workbook,
) -> TestResult<AuthenticatedActor> {
    let other = AuthenticatedActor {
        actor_id: ActorId::from_bytes([80; 16])?,
        workspace_id: owner.workspace_id,
    };
    sqlx::query("INSERT INTO users(user_id) VALUES($1)")
        .bind(uuid(other.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
        .bind(uuid(other.workspace_id.as_bytes()))
        .bind(uuid(other.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')")
        .bind(uuid(other.workspace_id.as_bytes())).bind(uuid(book.document_id().as_bytes())).bind(uuid(other.actor_id.as_bytes())).execute(&db.pool).await?;
    Ok(other)
}
async fn authored_move(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    book: &mut Workbook,
    a: [CellAddress; 3],
) -> TestResult<CommitRequest> {
    let target = prepare(db, owner, book, 40, moved(book, a[0], a[1])?).await?;
    commit(db, book, &target).await?;
    Ok(target)
}

#[sqlx::test(migrations = false)]
async fn move_history_commits_schema20_undo_redo_receipt_and_verified_replay(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book, a) = fixture(pool).await?;
    let target = authored_move(&db, owner, &mut book, a).await?;
    let undo = prepare(&db, owner, &book, 41, inverse(&target.event)?).await?;
    commit(&db, &mut book, &undo).await?;
    assert_eq!(
        book.cell_content(a[0])?,
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    assert!(book.cell_content(a[1])?.is_none());
    let schema: i32 = sqlx::query_scalar(
        "SELECT schema_version FROM document_operations WHERE document_id=$1 AND operation_id=$2",
    )
    .bind(uuid(book.document_id().as_bytes()))
    .bind(uuid(undo.event.operation_id.as_bytes()))
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(schema, 20);
    let redo = prepare(&db, owner, &book, 42, inverse(&undo.event)?).await?;
    commit(&db, &mut book, &redo).await?;
    let CommitOutcome::AlreadyCommitted(receipt) = db.commit(&undo).await? else {
        return Err("late undo retry recommitted".into());
    };
    assert_eq!(receipt.committed_version, undo.event.version);
    assert_eq!(receipt.current_head_version, redo.event.version);
    assert_eq!(
        book.cell_content(a[1])?,
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    let mut replay = Workbook::empty(book.document_id());
    loop {
        let page = db
            .load_events(owner, book.document_id(), replay.version(), 2)
            .await?;
        if page.is_empty() {
            break;
        }
        for event in page {
            replay.apply_event(&event)?;
        }
    }
    assert_eq!(replay.hash(), book.hash());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn move_history_foreign_same_value_and_other_actor_do_not_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book, a) = fixture(pool).await?;
    let other = foreign(&db, owner, &book).await?;
    let target = authored_move(&db, owner, &mut book, a).await?;
    assert!(matches!(
        db.load_undo_target(other, book.document_id(), target.event.operation_id)
            .await,
        Err(JournalError::NotUndoable)
    ));
    let equal = prepare(
        &db,
        other,
        &book,
        43,
        Operation::SetCellValue {
            address: a[1],
            value: CellValue::Number(42.0),
        },
    )
    .await?;
    commit(&db, &mut book, &equal).await?;
    let denied = prepare(&db, owner, &book, 44, inverse(&target.event)?).await?;
    assert!(matches!(
        db.commit(&denied).await,
        Err(JournalError::UndoConflict)
    ));
    assert!(db.receipt(owner, &denied.command).await?.is_none());
    assert_eq!(
        db.access(owner, book.document_id()).await?.head,
        book.version()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn move_history_new_range_reference_and_wrong_projection_are_guarded(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book, a) = fixture(pool).await?;
    let other = foreign(&db, owner, &book).await?;
    let target = authored_move(&db, owner, &mut book, a).await?;
    let mut missing = prepare(&db, owner, &book, 45, inverse(&target.event)?).await?;
    missing.move_inverse_projection = None;
    assert!(matches!(
        db.commit(&missing).await,
        Err(JournalError::Corrupt)
    ));
    let mut wrong = prepare(&db, owner, &book, 46, inverse(&target.event)?).await?;
    let bad = book.candidate(
        &Operation::SetCellValue {
            address: a[2],
            value: CellValue::Number(99.0),
        },
        DocumentVersion::new(book.version().get() + 1)?,
    )?;
    wrong.move_inverse_projection = Some(Arc::new(bad));
    assert!(matches!(
        db.commit(&wrong).await,
        Err(JournalError::UndoConflict)
    ));
    let sheet = &book.sheets()[0];
    let corrupt_projection = Workbook::hydrate_sheets(
        book.document_id(),
        book.version(),
        [SheetSeed {
            id: sheet.id(),
            name: sheet.name().into(),
            rows: sheet.rows().to_vec(),
            columns: sheet.columns().to_vec(),
        }],
        [(a[2], CellContent::Value(CellValue::Number(99.0)))],
    )?;
    wrong.move_inverse_projection = Some(Arc::new(corrupt_projection));
    assert!(matches!(
        db.commit(&wrong).await,
        Err(JournalError::UndoConflict)
    ));
    let wrong_document = Workbook::hydrate_sheets(
        DocumentId::from_bytes([79; 16])?,
        book.version(),
        [SheetSeed {
            id: sheet.id(),
            name: sheet.name().into(),
            rows: sheet.rows().to_vec(),
            columns: sheet.columns().to_vec(),
        }],
        sheet
            .cells()
            .iter()
            .map(|(&address, content)| (address, content.clone())),
    )?;
    wrong.move_inverse_projection = Some(Arc::new(wrong_document));
    assert!(matches!(
        db.commit(&wrong).await,
        Err(JournalError::UndoConflict)
    ));
    let formula = spreadsheet_formula::binder::bind("=SUM(A1:C1)", &book, a[2])?;
    let new_ref = prepare(
        &db,
        other,
        &book,
        47,
        Operation::SetCellFormula {
            address: a[2],
            formula,
        },
    )
    .await?;
    commit(&db, &mut book, &new_ref).await?;
    let denied = prepare(&db, owner, &book, 48, inverse(&target.event)?).await?;
    assert!(matches!(
        db.commit(&denied).await,
        Err(JournalError::UndoConflict)
    ));
    assert!(db.receipt(owner, &denied.command).await?.is_none());
    assert_eq!(
        db.access(owner, book.document_id()).await?.head,
        book.version()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn move_history_forged_family_and_geometry_are_not_inverse_targets(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book, a) = fixture(pool).await?;
    let target = authored_move(&db, owner, &mut book, a).await?;
    let Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes,
        undoes,
        undoes_version,
    } = inverse(&target.event)?
    else {
        return Err("wrong fixture".into());
    };
    let forged = Operation::MoveCompensation {
        source: destination,
        destination: source,
        rows,
        columns,
        writes: writes.clone(),
        undoes,
        undoes_version,
    };
    let command = prepare(&db, owner, &book, 52, forged).await?;
    assert!(matches!(
        db.commit(&command).await,
        Err(JournalError::UndoConflict)
    ));
    let smaller = Operation::RangeWrite {
        writes,
        kind: None,
        undoes: Some(undoes),
        undoes_version: Some(undoes_version),
    };
    let command = prepare(&db, owner, &book, 53, smaller).await?;
    assert!(matches!(
        db.commit(&command).await,
        Err(JournalError::UndoConflict)
    ));
    assert_eq!(
        db.access(owner, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(owner, &command.command).await?.is_none());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn move_history_large_inverse_exceeds_schema18_capacity_without_widening_it(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book) = crate::journal::tests::fixture(pool).await?;
    let seed = SheetSeed {
        id: SheetId::from_bytes([90; 16])?,
        name: "Large move".into(),
        rows: (10_000u128..11_000)
            .map(|n| RowId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
        columns: (20_000u128..20_002)
            .map(|n| ColumnId::from_bytes(n.to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let source = CellAddress {
        sheet: seed.id,
        row: seed.rows[0],
        column: seed.columns[0],
    };
    let destination = CellAddress {
        column: seed.columns[1],
        ..source
    };
    let added = prepare(
        &db,
        owner,
        &book,
        54,
        Operation::AddSheet {
            sheet: seed.clone(),
        },
    )
    .await?;
    commit(&db, &mut book, &added).await?;
    let filled = prepare(
        &db,
        owner,
        &book,
        55,
        Operation::SetCellValues {
            writes: seed
                .rows
                .iter()
                .enumerate()
                .map(|(index, &row)| CellWrite {
                    address: CellAddress { row, ..source },
                    value: CellValue::Number(index as f64),
                })
                .collect(),
        },
    )
    .await?;
    commit(&db, &mut book, &filled).await?;
    let plan = plan_move(
        &book,
        MoveRequest {
            source,
            destination,
            rows: 1000,
            columns: 1,
        },
    )?;
    assert_eq!(plan.writes.len(), 2000);
    let target = prepare(
        &db,
        owner,
        &book,
        56,
        Operation::MoveRange {
            source,
            destination,
            rows: 1000,
            columns: 1,
            writes: plan.writes,
        },
    )
    .await?;
    commit(&db, &mut book, &target).await?;
    let undo = prepare(&db, owner, &book, 57, inverse(&target.event)?).await?;
    assert!(
        matches!(&undo.event.operation, Operation::MoveCompensation { writes, .. } if writes.len() == 2000)
    );
    commit(&db, &mut book, &undo).await?;
    assert_eq!(
        book.cell_content(source)?,
        Some(&CellContent::Value(CellValue::Number(0.0)))
    );
    assert_eq!(
        book.cell_content(CellAddress {
            row: seed.rows[999],
            ..source
        })?,
        Some(&CellContent::Value(CellValue::Number(999.0)))
    );
    assert!(book.cell_content(destination)?.is_none());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn move_history_neutralized_own_reference_allows_inverse(pool: sqlx::PgPool) -> TestResult {
    let (db, owner, mut book, a) = fixture(pool).await?;
    let target = authored_move(&db, owner, &mut book, a).await?;
    let formula = spreadsheet_formula::binder::bind("=B1+1", &book, a[2])?;
    let own_ref = prepare(
        &db,
        owner,
        &book,
        49,
        Operation::CellWrite {
            address: a[2],
            content: Some(CellContent::Formula(formula)),
            previous: None,
            kind: Some(CellWriteKind::SetFormula),
            undoes: None,
            undoes_version: None,
        },
    )
    .await?;
    commit(&db, &mut book, &own_ref).await?;
    let undo_ref = prepare(
        &db,
        owner,
        &book,
        50,
        Operation::CellWrite {
            address: a[2],
            content: None,
            previous: book.cell_content(a[2])?.cloned(),
            kind: None,
            undoes: Some(own_ref.event.operation_id),
            undoes_version: Some(own_ref.event.version),
        },
    )
    .await?;
    commit(&db, &mut book, &undo_ref).await?;
    let undo_move = prepare(&db, owner, &book, 51, inverse(&target.event)?).await?;
    commit(&db, &mut book, &undo_move).await?;
    assert_eq!(
        book.cell_content(a[0])?,
        Some(&CellContent::Value(CellValue::Number(42.0)))
    );
    Ok(())
}
