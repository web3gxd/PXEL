use super::*;
use spreadsheet_application::journal::OperationJournal;
use spreadsheet_domain::workbook::*;
#[sqlx::test(migrations = false)]
async fn catalog_encrypts_titles_preferences_and_checks_current_acl_with_revision_cas(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let workspace = WorkspaceId::from_bytes([2; 16])?;
    let owner = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: workspace,
    };
    let viewer = AuthenticatedActor {
        actor_id: ActorId::from_bytes([8; 16])?,
        workspace_id: workspace,
    };
    db.issue_development_session([9; 32], owner.actor_id, workspace)
        .await?;
    db.issue_development_session([8; 32], viewer.actor_id, WorkspaceId::from_bytes([10; 16])?)
        .await?;
    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')")
        .bind(Uuid::from_bytes(*workspace.as_bytes()))
        .bind(actor(viewer))
        .execute(&db.pool)
        .await?;
    let doc = DocumentId::from_bytes([3; 16])?;
    let op = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([4; 16])?,
            name: "Data".into(),
            rows: vec![RowId::from_bytes([5; 16])?],
            columns: vec![ColumnId::from_bytes([6; 16])?],
        },
    };
    let book = Workbook::empty(doc).candidate(&op, DocumentVersion::new(1)?)?;
    db.create(
        owner,
        CommittedEvent {
            document_id: doc,
            operation_id: OperationId::from_bytes([7; 16])?,
            principal: EventPrincipal::User(owner.actor_id),
            version: book.version(),
            operation: op,
            state_hash: book.hash(),
        },
    )
    .await?;
    sqlx::query("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'viewer')").bind(Uuid::from_bytes(*workspace.as_bytes())).bind(id(doc)).bind(actor(viewer)).execute(&db.pool).await?;
    let title = "Catalog encrypted fixture";
    assert_eq!(db.document_metadata(owner, doc).await?.revision, 0);
    let metadata = db
        .set_document_metadata(owner, doc, 0, title.into())
        .await?;
    assert_eq!(metadata.revision, 1);
    let encrypted: Vec<u8> =
        sqlx::query_scalar("SELECT payload FROM document_metadata WHERE document_id=$1")
            .bind(id(doc))
            .fetch_one(&db.pool)
            .await?;
    assert!(
        !encrypted
            .windows(title.len())
            .any(|p| p == title.as_bytes())
    );
    assert!(matches!(
        db.set_document_metadata(viewer, doc, 1, "Other".into())
            .await,
        Err(CatalogError::Unauthorized)
    ));
    let (a, b) = tokio::join!(
        db.set_document_metadata(owner, doc, 1, "A".into()),
        db.set_document_metadata(owner, doc, 1, "B".into())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(CatalogError::Conflict)) || matches!(b, Err(CatalogError::Conflict)));
    let favorite = db
        .set_library_preference(owner, doc, Some(0), Some(true))
        .await?;
    assert!(favorite.favorite);
    assert!(matches!(
        db.set_library_preference(owner, doc, Some(0), Some(false))
            .await,
        Err(CatalogError::Conflict)
    ));
    assert!(
        db.set_library_preference(owner, doc, None, None)
            .await?
            .opened_at
            > 0
    );
    let (page, lease) = db.catalog_page(viewer, None).await?;
    assert_eq!(page.documents.len(), 1);
    assert_eq!(page.documents[0].role, "viewer");
    assert!(!page.documents[0].preference.favorite);
    drop(lease);
    let (page, lease) = db.catalog_page(owner, None).await?;
    assert!(page.documents[0].preference.favorite);
    assert!(page.documents[0].preference.opened_at > 0);
    drop(lease);
    assert_eq!(
        db.set_library_layout(owner, 0, LibraryLayout::List)
            .await?
            .revision,
        1
    );
    assert!(matches!(
        db.set_library_layout(owner, 0, LibraryLayout::Cards).await,
        Err(CatalogError::Conflict)
    ));
    sqlx::query("DELETE FROM document_permissions WHERE document_id=$1 AND user_id=$2")
        .bind(id(doc))
        .bind(actor(viewer))
        .execute(&db.pool)
        .await?;
    let (page, lease) = db.catalog_page(viewer, None).await?;
    assert!(page.documents.is_empty());
    drop(lease);
    assert!(matches!(
        db.document_metadata(viewer, doc).await,
        Err(CatalogError::Unauthorized)
    ));
    assert_eq!(db.access(owner, doc).await?.head, book.version());
    assert_eq!(
        db.load_events(owner, doc, DocumentVersion::ZERO, 1).await?[0].state_hash,
        book.hash()
    );
    // A page boundary must not skip or duplicate a book, including legacy
    // books whose encrypted metadata row has not been written yet.
    for _ in 0..101 {
        let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
                name: "Page".into(),
                rows: vec![RowId::from_bytes(*Uuid::now_v7().as_bytes())?],
                columns: vec![ColumnId::from_bytes(*Uuid::now_v7().as_bytes())?],
            },
        };
        let candidate =
            Workbook::empty(document).candidate(&operation, DocumentVersion::new(1)?)?;
        db.create(
            owner,
            CommittedEvent {
                document_id: document,
                operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
                principal: EventPrincipal::User(owner.actor_id),
                version: candidate.version(),
                operation,
                state_hash: candidate.hash(),
            },
        )
        .await?;
    }
    let (first, lease) = db.catalog_page(owner, None).await?;
    assert_eq!(first.documents.len(), 100);
    let cursor = first.next_cursor.ok_or("missing catalog cursor")?;
    drop(lease);
    let (second, lease) = db.catalog_page(owner, Some(cursor)).await?;
    assert_eq!(second.documents.len(), 2);
    assert!(second.next_cursor.is_none());
    assert!(second.documents.iter().all(|entry| {
        !first
            .documents
            .iter()
            .any(|old| old.document_id == entry.document_id)
    }));
    drop(lease);
    Ok(())
}
