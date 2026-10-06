use super::*;
use spreadsheet_application::journal::OperationJournal;
use spreadsheet_domain::workbook::{
    ActorId, ColumnId, CommittedEvent, DocumentVersion, EventPrincipal, Operation, OperationId,
    RowId, SheetId, SheetSeed, Workbook, WorkspaceId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn fixture(
    pool: sqlx::PgPool,
) -> Result<(PgDatabase, AuthenticatedActor, DocumentId, CommentAddress), Box<dyn std::error::Error>>
{
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let identity = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    db.issue_development_session([9; 32], identity.actor_id, identity.workspace_id)
        .await?;
    let document = DocumentId::from_bytes([3; 16])?;
    let sheet = SheetId::from_bytes([20; 16])?;
    let row = RowId::from_bytes([30; 16])?;
    let column = ColumnId::from_bytes([40; 16])?;
    let operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: sheet,
            name: "Comment fixture".into(),
            rows: vec![row],
            columns: vec![column],
        },
    };
    let workbook = Workbook::empty(document).candidate(&operation, DocumentVersion::new(1)?)?;
    db.create(
        identity,
        CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([50; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: workbook.version(),
            operation,
            state_hash: workbook.hash(),
        },
    )
    .await?;
    Ok((
        db,
        identity,
        document,
        CommentAddress {
            sheet_id: Uuid::from_bytes(*sheet.as_bytes()),
            row_id: Uuid::from_bytes(*row.as_bytes()),
            column_id: Uuid::from_bytes(*column.as_bytes()),
        },
    ))
}

#[sqlx::test(migrations = false)]
async fn encrypted_comment_journal_is_idempotent_separate_and_append_only(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, identity, document, address) = fixture(pool).await?;
    let request = Uuid::now_v7();
    let text = "Защищённый комментарий 🔒";
    db.add_comment(
        identity,
        document,
        request,
        address,
        Zeroizing::new(text.to_owned()),
    )
    .await?;
    db.add_comment(
        identity,
        document,
        request,
        address,
        Zeroizing::new(text.to_owned()),
    )
    .await?;
    assert!(matches!(
        db.add_comment(
            identity,
            document,
            request,
            address,
            Zeroizing::new("другой".into())
        )
        .await,
        Err(CommentError::IdempotencyConflict)
    ));
    let page = db
        .list_comments(identity, document, address, None, 100)
        .await?;
    assert_eq!(page.revision, 1);
    assert_eq!(page.comments.len(), 1);
    assert_eq!(&*page.comments[0].text, text);
    assert!(page.comments[0].can_delete);
    let workbook = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes(*address.sheet_id.as_bytes())?,
                name: "Comment fixture".into(),
                rows: vec![RowId::from_bytes(*address.row_id.as_bytes())?],
                columns: vec![ColumnId::from_bytes(*address.column_id.as_bytes())?],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    assert_eq!(workbook.version().get(), 1);
    let stored: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT payload FROM document_comment_events WHERE document_id=$1")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .fetch_all(&db.pool)
            .await?;
    assert_eq!(stored.len(), 1);
    assert!(
        !stored[0]
            .windows(text.len())
            .any(|window| window == text.as_bytes())
    );
    let deleted = db
        .delete_comment(identity, document, Uuid::now_v7(), request)
        .await?;
    assert_eq!(deleted, address);
    let after = db
        .list_comments(identity, document, address, None, 100)
        .await?;
    assert_eq!(after.revision, 2);
    assert!(after.comments.is_empty());
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_comment_events WHERE document_id=$1")
            .bind(Uuid::from_bytes(*document.as_bytes()))
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(events, 2);
    let rejected = sqlx::query("DELETE FROM document_comment_events WHERE document_id=$1")
        .bind(Uuid::from_bytes(*document.as_bytes()))
        .execute(&db.pool)
        .await;
    assert!(rejected.is_err());
    Ok(())
}
