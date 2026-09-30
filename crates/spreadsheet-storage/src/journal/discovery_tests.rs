use super::*;
use spreadsheet_application::history::{HistoryEntryKind, HistoryTarget, HistoryTruncation};
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn address(book: &Workbook) -> TestResult<CellAddress> {
    let sheet = book.sheets().first().ok_or("fixture sheet")?;
    Ok(CellAddress {
        sheet: sheet.id(),
        row: *sheet.rows().first().ok_or("fixture row")?,
        column: *sheet.columns().first().ok_or("fixture column")?,
    })
}

async fn request(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
    operation: Operation,
    payload: CommandPayload,
) -> TestResult<CommitRequest> {
    let next = DocumentVersion::new(book.version().get() + 1)?;
    let candidate = book.candidate(&operation, next)?;
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
            version: next,
            operation,
            state_hash: candidate.hash(),
        },
    })
}
async fn scalar(
    db: &PgDatabase,
    identity: AuthenticatedActor,
    book: &Workbook,
    id: u128,
) -> TestResult<CommitRequest> {
    let address = address(book)?;
    let value = CellValue::Number(id as f64);
    request(
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
async fn commit(db: &PgDatabase, book: &mut Workbook, request: &CommitRequest) -> TestResult {
    assert!(matches!(
        db.commit(request).await?,
        CommitOutcome::Committed(_)
    ));
    book.apply_event(&request.event)?;
    Ok(())
}
async fn foreign(
    db: &PgDatabase,
    owner: AuthenticatedActor,
    document: DocumentId,
) -> TestResult<AuthenticatedActor> {
    let actor_id = ActorId::from_bytes([45; 16])?;
    sqlx::query("INSERT INTO users(user_id) VALUES($1)")
        .bind(uuid(actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
        .bind(uuid(owner.workspace_id.as_bytes()))
        .bind(uuid(actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')")
        .bind(uuid(owner.workspace_id.as_bytes())).bind(uuid(document.as_bytes())).bind(uuid(actor_id.as_bytes())).execute(&db.pool).await?;
    Ok(AuthenticatedActor {
        actor_id,
        workspace_id: owner.workspace_id,
    })
}

#[sqlx::test(migrations = false)]
async fn history_discovery_keeps_foreign_and_system_records_and_own_barriers(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let authored = scalar(&db, identity, &book, 2000).await?;
    commit(&db, &mut book, &authored).await?;
    let other = foreign(&db, identity, book.document_id()).await?;
    let foreign_write = scalar(&db, other, &book, 2001).await?;
    commit(&db, &mut book, &foreign_write).await?;
    let operation = Operation::AdvanceCalculationEpoch {
        context: CalculationContext::new(
            CalculationContextId::from_bytes([46; 16])?,
            1,
            0,
            0,
            0,
            [47; 32],
            spreadsheet_formula::calculation::ENGINE_VERSION.into(),
            spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
            CalculationReason::Explicit,
        )?,
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(book.version().get() + 1)?)?;
    let system = SystemCommitRequest {
        identity,
        expected_access: db.access(identity, book.document_id()).await?,
        expected_head: book.version(),
        event: CommittedEvent {
            document_id: book.document_id(),
            operation_id: OperationId::from_bytes([48; 16])?,
            principal: EventPrincipal::System,
            version: candidate.version(),
            operation,
            state_hash: candidate.hash(),
        },
    };
    assert!(matches!(
        db.commit_system(&system).await?,
        CommitOutcome::Committed(_)
    ));
    book.apply_event(&system.event)?;
    let window = discovery::scan(
        &db,
        identity,
        book.document_id(),
        db.access(identity, book.document_id()).await?,
    )
    .await?;
    assert_eq!(window.entries.len(), 4);
    assert_eq!(window.entries[0].kind, HistoryEntryKind::Barrier);
    assert_eq!(window.entries[1].kind, HistoryEntryKind::Original);
    assert_eq!(window.entries[2].kind, HistoryEntryKind::Ignore);
    assert_eq!(window.entries[3].kind, HistoryEntryKind::Ignore);
    let history = db.undo_history(identity, book.document_id()).await?;
    assert_eq!(
        history.undo,
        vec![HistoryTarget {
            operation_id: authored.event.operation_id,
            committed_version: authored.event.version
        }]
    );
    assert!(history.redo.is_empty());
    let address = address(&book)?;
    let legacy = request(
        &db,
        identity,
        &book,
        2002,
        Operation::SetCellValue {
            address,
            value: CellValue::Number(1.),
        },
        CommandPayload::SetCellValue {
            address,
            value: CellValue::Number(1.),
        },
    )
    .await?;
    commit(&db, &mut book, &legacy).await?;
    assert!(
        db.undo_history(identity, book.document_id())
            .await?
            .undo
            .is_empty()
    );
    let authored = scalar(&db, identity, &book, 2003).await?;
    commit(&db, &mut book, &authored).await?;
    let Operation::CellWrite {
        content, previous, ..
    } = &authored.event.operation
    else {
        return Err("authored fixture".into());
    };
    let inverse = request(
        &db,
        identity,
        &book,
        2004,
        Operation::CellWrite {
            address,
            content: previous.clone(),
            previous: content.clone(),
            kind: None,
            undoes: Some(authored.event.operation_id),
            undoes_version: Some(authored.event.version),
        },
        CommandPayload::UndoCell {
            target_operation_id: authored.event.operation_id,
        },
    )
    .await?;
    commit(&db, &mut book, &inverse).await?;
    let history = db.undo_history(identity, book.document_id()).await?;
    assert!(history.undo.is_empty());
    assert_eq!(
        history.redo,
        vec![HistoryTarget {
            operation_id: inverse.event.operation_id,
            committed_version: inverse.event.version
        }]
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_frozen_head_excludes_later_durable_commit(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    for id in 2100..2230 {
        let write = scalar(&db, identity, &book, id).await?;
        commit(&db, &mut book, &write).await?;
    }
    let frozen = db.access(identity, book.document_id()).await?;
    let later = scalar(&db, identity, &book, 2230).await?;
    let (window, committed) = tokio::join!(
        discovery::scan(&db, identity, book.document_id(), frozen),
        db.commit(&later),
    );
    assert!(matches!(committed?, CommitOutcome::Committed(_)));
    book.apply_event(&later.event)?;
    let window = window?;
    assert_eq!(
        discovery::scan(&db, identity, book.document_id(), frozen).await?,
        window,
    );
    assert_eq!(window.through_version.get(), 131);
    assert_eq!(window.entries.len(), 131);
    assert_eq!(
        window
            .entries
            .last()
            .ok_or("history tail")?
            .target
            .committed_version
            .get(),
        131
    );
    assert!(
        window
            .entries
            .iter()
            .all(|entry| entry.target.operation_id != later.event.operation_id)
    );
    assert_eq!(
        db.undo_history(identity, book.document_id())
            .await?
            .through_version,
        book.version()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_viewer_cross_tenant_and_changed_epochs_are_denied(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, book) = super::tests::fixture(pool).await?;
    let initial = db.access(identity, book.document_id()).await?;
    let cross = AuthenticatedActor {
        workspace_id: WorkspaceId::from_bytes([49; 16])?,
        ..identity
    };
    assert!(matches!(
        db.undo_history(cross, book.document_id()).await,
        Err(JournalError::Unauthorized | JournalError::NotFound)
    ));
    sqlx::raw_sql("UPDATE document_permissions SET role='viewer'")
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        db.undo_history(identity, book.document_id()).await,
        Err(JournalError::Unauthorized)
    ));
    assert!(matches!(
        discovery::scan(&db, identity, book.document_id(), initial).await,
        Err(JournalError::Unauthorized)
    ));
    sqlx::raw_sql("UPDATE document_permissions SET role='editor'; UPDATE documents SET access_epoch=access_epoch+1").execute(&db.pool).await?;
    assert!(matches!(
        discovery::scan(&db, identity, book.document_id(), initial).await,
        Err(JournalError::Unauthorized)
    ));
    let initial = db.access(identity, book.document_id()).await?;
    sqlx::raw_sql("UPDATE memberships SET access_epoch=access_epoch+1")
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        discovery::scan(&db, identity, book.document_id(), initial).await,
        Err(JournalError::Unauthorized)
    ));
    let initial = db.access(identity, book.document_id()).await?;
    sqlx::raw_sql("DELETE FROM document_permissions")
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        discovery::scan(&db, identity, book.document_id(), initial).await,
        Err(JournalError::Unauthorized)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_corrupt_payload_and_missing_oldest_record_fail(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let write = scalar(&db, identity, &book, 2300).await?;
    commit(&db, &mut book, &write).await?;
    sqlx::raw_sql("ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable; UPDATE document_operations SET payload_checksum=decode(repeat('00',32),'hex') WHERE committed_version=1; ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable").execute(&db.pool).await?;
    assert!(matches!(
        db.undo_history(identity, book.document_id()).await,
        Err(JournalError::Corrupt)
    ));
    sqlx::raw_sql("ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable; DELETE FROM document_operations WHERE committed_version=1; ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable").execute(&db.pool).await?;
    assert!(matches!(
        db.undo_history(identity, book.document_id()).await,
        Err(JournalError::Corrupt)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_unknown_schema_fails_atomically(pool: sqlx::PgPool) -> TestResult {
    let (db, identity, book) = super::tests::fixture(pool).await?;
    // Isolated damaged-storage fixture bypasses append-only and schema constraints.
    sqlx::raw_sql("ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable; ALTER TABLE document_operations DROP CONSTRAINT document_operations_principal_matches_actor; UPDATE document_operations SET schema_version=999; ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable").execute(&db.pool).await?;
    assert!(matches!(
        db.undo_history(identity, book.document_id()).await,
        Err(JournalError::UnsupportedSchema)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_version_window_includes_exact_10000_tail_boundary(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    for id in 10_000..20_000 {
        let write = scalar(&db, identity, &book, id).await?;
        commit(&db, &mut book, &write).await?;
    }
    let full = discovery::scan(
        &db,
        identity,
        book.document_id(),
        db.access(identity, book.document_id()).await?,
    )
    .await?;
    assert_eq!(full.entries.len(), 10_001);
    assert_eq!(full.earliest_scanned_version.get(), 1);
    assert_eq!(full.truncation, None);
    let write = scalar(&db, identity, &book, 20_000).await?;
    commit(&db, &mut book, &write).await?;
    let bounded = discovery::scan(
        &db,
        identity,
        book.document_id(),
        db.access(identity, book.document_id()).await?,
    )
    .await?;
    assert_eq!(bounded.entries.len(), 10_001);
    assert_eq!(bounded.earliest_scanned_version.get(), 2);
    assert_eq!(bounded.through_version.get(), 10_002);
    assert_eq!(bounded.truncation, Some(HistoryTruncation::VersionWindow));
    assert_eq!(
        db.undo_history(identity, book.document_id())
            .await?
            .undo
            .len(),
        100
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn history_discovery_byte_budget_returns_exact_newest_verified_suffix(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, mut book) = super::tests::fixture(pool).await?;
    let sheet = SheetSeed {
        id: SheetId::from_bytes([50; 16])?,
        name: "Payload budget".into(),
        rows: vec![RowId::from_bytes([51; 16])?],
        columns: (0..16u128)
            .map(|index| ColumnId::from_bytes((30_000 + index).to_be_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let addresses = sheet
        .columns
        .iter()
        .map(|column| CellAddress {
            sheet: sheet.id,
            row: sheet.rows[0],
            column: *column,
        })
        .collect::<Vec<_>>();
    let added = request(
        &db,
        identity,
        &book,
        30_100,
        Operation::AddSheet {
            sheet: sheet.clone(),
        },
        CommandPayload::AddSheet { sheet },
    )
    .await?;
    commit(&db, &mut book, &added).await?;
    for index in 0..65u128 {
        let writes = addresses
            .iter()
            .map(|address| {
                Ok(GuardedContentWrite {
                    address: *address,
                    content: Some(CellContent::Value(CellValue::Text("x".repeat(32 * 1024)))),
                    previous: book.cell_content(*address)?.cloned(),
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        let payload = CommandPayload::SetCellValues {
            writes: writes
                .iter()
                .map(|write| {
                    Ok(CellWrite {
                        address: write.address,
                        value: match &write.content {
                            Some(CellContent::Value(value)) => value.clone(),
                            _ => return Err("value fixture"),
                        },
                    })
                })
                .collect::<Result<_, _>>()?,
        };
        let next = request(
            &db,
            identity,
            &book,
            30_200 + index,
            Operation::RangeWrite {
                writes,
                kind: Some(RangeWriteKind::SetValues),
                undoes: None,
                undoes_version: None,
            },
            payload,
        )
        .await?;
        commit(&db, &mut book, &next).await?;
    }
    let sizes = sqlx::query_as::<_, (i64, i32)>(
        "SELECT committed_version,octet_length(payload) FROM document_operations WHERE document_id=$1 ORDER BY committed_version DESC",
    ).bind(uuid(book.document_id().as_bytes())).fetch_all(&db.pool).await?;
    let total = sizes
        .iter()
        .map(|(_, bytes)| i64::from(*bytes))
        .sum::<i64>();
    assert!(total > MAX_UNDO_TAIL_BYTES as i64);
    let mut included_bytes = 0usize;
    let mut included = 0usize;
    for (_, length) in &sizes {
        let length = usize::try_from(*length)?;
        if included_bytes + length > MAX_UNDO_TAIL_BYTES {
            break;
        }
        included_bytes += length;
        included += 1;
    }
    assert!(included > 0 && included < sizes.len());
    assert!(included_bytes + usize::try_from(sizes[included].1)? > MAX_UNDO_TAIL_BYTES);
    let window = discovery::scan(
        &db,
        identity,
        book.document_id(),
        db.access(identity, book.document_id()).await?,
    )
    .await?;
    assert_eq!(window.truncation, Some(HistoryTruncation::ByteBudget));
    assert_eq!(window.entries.len(), included);
    assert_eq!(
        window.earliest_scanned_version.get(),
        u64::try_from(sizes[included - 1].0)?
    );
    assert_eq!(window.through_version, book.version());
    assert_eq!(
        db.undo_history(identity, book.document_id())
            .await?
            .undo
            .len(),
        included
    );
    // The excluded old history is deliberately outside this bounded response.
    sqlx::raw_sql("ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable; UPDATE document_operations SET payload_checksum=decode(repeat('00',32),'hex') WHERE committed_version=1; ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable").execute(&db.pool).await?;
    assert_eq!(
        db.undo_history(identity, book.document_id())
            .await?
            .truncation,
        Some(HistoryTruncation::ByteBudget)
    );
    Ok(())
}
