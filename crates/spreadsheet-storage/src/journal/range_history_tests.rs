use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type TestValue<T> = Result<T, Box<dyn std::error::Error>>;

fn value(number: f64) -> Option<CellContent> {
    Some(CellContent::Value(CellValue::Number(number)))
}

async fn request(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    identifier: u128,
    operation: Operation,
) -> TestValue<CommitRequest> {
    let next = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, next)?;
    let payload = match &operation {
        Operation::AddSheet { sheet } => CommandPayload::AddSheet {
            sheet: sheet.clone(),
        },
        Operation::CellWrite {
            address,
            content,
            kind: Some(CellWriteKind::SetValue),
            ..
        } => {
            let Some(CellContent::Value(value)) = content else {
                return Err("invalid scalar fixture".into());
            };
            CommandPayload::SetCellValue {
                address: *address,
                value: value.clone(),
            }
        }
        Operation::CellWrite {
            undoes: Some(target),
            kind: None,
            ..
        }
        | Operation::RangeWrite {
            undoes: Some(target),
            kind: None,
            ..
        } => CommandPayload::UndoCell {
            target_operation_id: *target,
        },
        Operation::RangeWrite {
            writes,
            kind: Some(RangeWriteKind::SetValues),
            ..
        } => CommandPayload::SetCellValues {
            writes: writes
                .iter()
                .map(|write| {
                    Ok(CellWrite {
                        address: write.address,
                        value: match &write.content {
                            Some(CellContent::Value(value)) => value.clone(),
                            None => CellValue::Blank,
                            _ => return Err("invalid range fixture"),
                        },
                    })
                })
                .collect::<Result<_, _>>()?,
        },
        _ => return Err("unsupported range-history fixture operation".into()),
    };
    let operation_id = OperationId::from_bytes(identifier.to_be_bytes())?;
    Ok(CommitRequest {
        inverse_projection: None,
        identity,
        expected_head: book.version(),
        expected_access: db.access(identity, book.document_id()).await?,
        command: Command {
            operation_id,
            document_id: book.document_id(),
            actor_id: identity.actor_id,
            base_version: book.version(),
            client_sequence: identifier as u64,
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
    cells: usize,
) -> TestValue<(PgDatabase, AuthenticatedActor, Workbook, Vec<CellAddress>)> {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let seed = SheetSeed {
        id: SheetId::from_bytes([21; 16])?,
        name: "Range history".into(),
        rows: vec![RowId::from_bytes([22; 16])?],
        columns: (0..cells)
            .map(|index| ColumnId::from_bytes((10_000 + index as u128).to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let addresses = seed
        .columns
        .iter()
        .map(|&column| CellAddress {
            sheet: seed.id,
            row: seed.rows[0],
            column,
        })
        .collect();
    let added = request(
        &db,
        identity,
        &book,
        90_000,
        Operation::AddSheet { sheet: seed },
    )
    .await?;
    commit(&db, &mut book, &added).await?;
    Ok((db, identity, book, addresses))
}

fn range(
    book: &Workbook,
    addresses: &[CellAddress],
    number: f64,
) -> Result<Operation, DomainError> {
    Ok(Operation::RangeWrite {
        writes: addresses
            .iter()
            .map(|&address| {
                Ok(GuardedContentWrite {
                    address,
                    content: value(number),
                    previous: book.cell_content(address)?.cloned(),
                })
            })
            .collect::<Result<_, DomainError>>()?,
        kind: Some(RangeWriteKind::SetValues),
        undoes: None,
        undoes_version: None,
    })
}
fn scalar(book: &Workbook, address: CellAddress, number: f64) -> Result<Operation, DomainError> {
    Ok(Operation::CellWrite {
        address,
        content: value(number),
        previous: book.cell_content(address)?.cloned(),
        kind: Some(CellWriteKind::SetValue),
        undoes: None,
        undoes_version: None,
    })
}
fn inverse(target: &CommittedEvent) -> TestValue<Operation> {
    Ok(match &target.operation {
        Operation::CellWrite {
            address,
            content,
            previous,
            ..
        } => Operation::CellWrite {
            address: *address,
            content: previous.clone(),
            previous: content.clone(),
            kind: None,
            undoes: Some(target.operation_id),
            undoes_version: Some(target.version),
        },
        Operation::RangeWrite { writes, .. } => Operation::RangeWrite {
            writes: writes
                .iter()
                .map(|write| GuardedContentWrite {
                    address: write.address,
                    content: write.previous.clone(),
                    previous: write.content.clone(),
                })
                .collect(),
            kind: None,
            undoes: Some(target.operation_id),
            undoes_version: Some(target.version),
        },
        _ => return Err("invalid inverse fixture target".into()),
    })
}
async fn replay(db: &PgDatabase, identity: AuthenticatedActor, book: &Workbook) -> TestResult {
    let mut replay = Workbook::empty(book.document_id());
    while replay.version() < book.version() {
        let events = db
            .load_events(identity, book.document_id(), replay.version(), 2)
            .await?;
        assert!(!events.is_empty());
        assert!(events.len() <= 2);
        for event in events {
            replay.apply_event(&event)?;
        }
    }
    assert_eq!(replay.version(), book.version());
    assert_eq!(replay.hash(), book.hash());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn range_history_inverse_requires_exact_complete_ordered_footprint(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, addresses) = fixture(pool, 3).await?;
    let authored = request(
        &db,
        identity,
        &book,
        90_001,
        range(&book, &addresses[..2], 1.)?,
    )
    .await?;
    commit(&db, &mut book, &authored).await?;
    for mutation in 0..3 {
        let mut operation = inverse(&authored.event)?;
        if let Operation::RangeWrite { writes, .. } = &mut operation {
            match mutation {
                0 => {
                    writes.pop();
                }
                1 => writes.push(GuardedContentWrite {
                    address: addresses[2],
                    content: None,
                    previous: None,
                }),
                _ => writes.reverse(),
            }
        }
        let forged = request(&db, identity, &book, 90_010 + mutation, operation).await?;
        assert!(matches!(
            db.commit(&forged).await,
            Err(JournalError::UndoConflict)
        ));
        assert_eq!(
            db.access(identity, book.document_id()).await?.head,
            book.version()
        );
        assert!(db.receipt(identity, &forged.command).await?.is_none());
    }
    let valid = request(&db, identity, &book, 90_020, inverse(&authored.event)?).await?;
    commit(&db, &mut book, &valid).await?;
    for address in &addresses {
        assert_eq!(book.cell_content(*address)?, None);
    }
    replay(&db, identity, &book).await
}

#[sqlx::test(migrations = false)]
async fn range_history_scalar_range_pairs_cancel_and_late_receipts_replay_exactly(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, addresses) = fixture(pool, 2).await?;
    let authored = request(&db, identity, &book, 91_001, range(&book, &addresses, 1.)?).await?;
    commit(&db, &mut book, &authored).await?;
    let later_scalar = request(
        &db,
        identity,
        &book,
        91_002,
        scalar(&book, addresses[0], 2.)?,
    )
    .await?;
    commit(&db, &mut book, &later_scalar).await?;
    let undo_scalar = request(&db, identity, &book, 91_003, inverse(&later_scalar.event)?).await?;
    commit(&db, &mut book, &undo_scalar).await?;
    let later_range = request(&db, identity, &book, 91_004, range(&book, &addresses, 3.)?).await?;
    commit(&db, &mut book, &later_range).await?;
    let undo_range = request(&db, identity, &book, 91_005, inverse(&later_range.event)?).await?;
    commit(&db, &mut book, &undo_range).await?;
    let undo_authored = request(&db, identity, &book, 91_006, inverse(&authored.event)?).await?;
    commit(&db, &mut book, &undo_authored).await?;
    for &address in &addresses {
        assert_eq!(book.cell_content(address)?, None);
    }
    let redo = request(&db, identity, &book, 91_007, inverse(&undo_authored.event)?).await?;
    commit(&db, &mut book, &redo).await?;
    for &address in &addresses {
        assert_eq!(book.cell_content(address)?, value(1.).as_ref());
    }
    let CommitOutcome::AlreadyCommitted(receipt) = db.commit(&undo_authored).await? else {
        return Err("inverse retry committed twice".into());
    };
    assert_eq!(receipt.committed_version, undo_authored.event.version);
    assert_eq!(receipt.current_head_version, book.version());
    assert_eq!(
        receipt.canonical_event_hash,
        codec::event_hash(&undo_authored.event)?
    );
    let loaded = db
        .receipt(identity, &undo_authored.command)
        .await?
        .ok_or("inverse retry receipt missing")?;
    assert_eq!(loaded.operation_id, receipt.operation_id);
    assert_eq!(loaded.committed_version, receipt.committed_version);
    assert_eq!(loaded.current_head_version, receipt.current_head_version);
    assert_eq!(loaded.canonical_event_hash, receipt.canonical_event_hash);

    // The other direction: a later range pair can cancel inside a scalar target tail.
    let original_scalar = request(
        &db,
        identity,
        &book,
        91_008,
        scalar(&book, addresses[0], 4.)?,
    )
    .await?;
    commit(&db, &mut book, &original_scalar).await?;
    let later_range = request(&db, identity, &book, 91_009, range(&book, &addresses, 5.)?).await?;
    commit(&db, &mut book, &later_range).await?;
    let undo_range = request(&db, identity, &book, 91_010, inverse(&later_range.event)?).await?;
    commit(&db, &mut book, &undo_range).await?;
    let undo_scalar = request(
        &db,
        identity,
        &book,
        91_011,
        inverse(&original_scalar.event)?,
    )
    .await?;
    commit(&db, &mut book, &undo_scalar).await?;
    for &address in &addresses {
        assert_eq!(book.cell_content(address)?, value(1.).as_ref());
    }
    replay(&db, identity, &book).await
}

#[sqlx::test(migrations = false)]
async fn range_history_foreign_single_destination_pair_never_neutralizes_conflict(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, mut book, addresses) = fixture(pool, 2).await?;
    let authored = request(&db, owner, &book, 92_001, range(&book, &addresses, 1.)?).await?;
    commit(&db, &mut book, &authored).await?;
    let foreign = AuthenticatedActor {
        actor_id: ActorId::from_bytes([23; 16])?,
        workspace_id: owner.workspace_id,
    };
    sqlx::query("INSERT INTO users(user_id) VALUES($1)")
        .bind(uuid(foreign.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
        .bind(uuid(foreign.workspace_id.as_bytes()))
        .bind(uuid(foreign.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')")
        .bind(uuid(foreign.workspace_id.as_bytes())).bind(uuid(book.document_id().as_bytes())).bind(uuid(foreign.actor_id.as_bytes())).execute(&db.pool).await?;
    assert!(matches!(
        db.load_undo_target(foreign, book.document_id(), authored.event.operation_id)
            .await,
        Err(JournalError::NotUndoable)
    ));
    let equal = request(
        &db,
        foreign,
        &book,
        92_002,
        scalar(&book, addresses[0], 1.)?,
    )
    .await?;
    commit(&db, &mut book, &equal).await?;
    let foreign_inverse = request(&db, foreign, &book, 92_003, inverse(&equal.event)?).await?;
    commit(&db, &mut book, &foreign_inverse).await?;
    let inverse = request(&db, owner, &book, 92_004, inverse(&authored.event)?).await?;
    assert!(matches!(
        db.commit(&inverse).await,
        Err(JournalError::UndoConflict)
    ));
    assert_eq!(
        db.access(owner, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(owner, &inverse.command).await?.is_none());
    for &address in &addresses {
        assert_eq!(book.cell_content(address)?, value(1.).as_ref());
    }
    replay(&db, owner, &book).await
}

#[sqlx::test(migrations = false)]
async fn range_history_address_touch_budget_has_exact_boundary_and_no_partial_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, addresses) = fixture(pool, 1_000).await?;
    let authored = request(&db, identity, &book, 93_001, range(&book, &addresses, 1.)?).await?;
    commit(&db, &mut book, &authored).await?;
    for index in 0..100 {
        let equal = request(
            &db,
            identity,
            &book,
            93_010 + index,
            range(&book, &addresses, 1.)?,
        )
        .await?;
        commit(&db, &mut book, &equal).await?;
    }
    assert_eq!(MAX_UNDO_ADDRESS_TOUCHES, 100_000);
    let exact = request(&db, identity, &book, 94_001, inverse(&authored.event)?).await?;
    assert!(matches!(
        db.commit(&exact).await,
        Err(JournalError::UndoConflict)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(identity, &exact.command).await?.is_none());
    let excess = request(&db, identity, &book, 94_002, range(&book, &addresses, 1.)?).await?;
    commit(&db, &mut book, &excess).await?;
    let rejected = request(&db, identity, &book, 94_003, inverse(&authored.event)?).await?;
    assert!(matches!(
        db.commit(&rejected).await,
        Err(JournalError::UndoWindowExceeded)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(identity, &rejected.command).await?.is_none());
    replay(&db, identity, &book).await
}

#[sqlx::test(migrations = false)]
async fn range_history_independent_tail_byte_budget_rejects_without_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book, addresses) = fixture(pool, 17).await?;
    let authored = request(
        &db,
        identity,
        &book,
        95_001,
        scalar(&book, addresses[0], 1.)?,
    )
    .await?;
    commit(&db, &mut book, &authored).await?;
    let mut last_payload_bytes = 0i64;
    for index in 0..65 {
        let writes = addresses[1..]
            .iter()
            .map(|&address| {
                Ok(GuardedContentWrite {
                    address,
                    content: Some(CellContent::Value(CellValue::Text(
                        "x".repeat(MAX_TEXT_BYTES),
                    ))),
                    previous: book.cell_content(address)?.cloned(),
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        let next = request(
            &db,
            identity,
            &book,
            95_010 + index,
            Operation::RangeWrite {
                writes,
                kind: Some(RangeWriteKind::SetValues),
                undoes: None,
                undoes_version: None,
            },
        )
        .await?;
        commit(&db, &mut book, &next).await?;
        if index == 64 {
            last_payload_bytes = sqlx::query_scalar::<_, i64>(
                "SELECT octet_length(payload)::bigint FROM document_operations WHERE document_id=$1 AND operation_id=$2",
            ).bind(uuid(book.document_id().as_bytes()))
                .bind(uuid(next.event.operation_id.as_bytes()))
                .fetch_one(&db.pool).await?;
        }
    }
    let tail_bytes = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(octet_length(payload)),0)::bigint FROM document_operations WHERE document_id=$1 AND committed_version>$2",
    ).bind(uuid(book.document_id().as_bytes()))
        .bind(number(authored.event.version.get())?)
        .fetch_one(&db.pool).await?;
    assert_eq!(MAX_UNDO_TAIL_BYTES, 64 * 1024 * 1024);
    assert!(tail_bytes > MAX_UNDO_TAIL_BYTES as i64);
    assert!(tail_bytes - last_payload_bytes <= MAX_UNDO_TAIL_BYTES as i64);
    assert_eq!(book.cell_content(addresses[0])?, value(1.).as_ref());

    let rejected = request(&db, identity, &book, 96_001, inverse(&authored.event)?).await?;
    assert!(matches!(
        db.commit(&rejected).await,
        Err(JournalError::UndoWindowExceeded)
    ));
    assert_eq!(
        db.access(identity, book.document_id()).await?.head,
        book.version()
    );
    assert!(db.receipt(identity, &rejected.command).await?.is_none());
    // Every tail write is outside the target; the byte budget, not address
    // conflicts or address-touch work, is what rejects the otherwise valid undo.
    assert_eq!(book.cell_content(addresses[0])?, value(1.).as_ref());
    Ok(())
}
