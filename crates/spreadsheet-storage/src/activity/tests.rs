use super::*;
use spreadsheet_application::journal::{CommitOutcome, CommitRequest, OperationJournal};
use spreadsheet_domain::workbook::*;
use std::sync::Arc;
#[sqlx::test(migrations = false)]
async fn frozen_activity_pages_verify_ciphertext_and_current_owner(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let identity = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    db.issue_development_session([9; 32], identity.actor_id, identity.workspace_id)
        .await?;
    let doc = DocumentId::from_bytes([3; 16])?;
    let sheet = SheetId::from_bytes([4; 16])?;
    let row = RowId::from_bytes([5; 16])?;
    let column = ColumnId::from_bytes([6; 16])?;
    let op = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: sheet,
            name: "Activity".into(),
            rows: vec![row],
            columns: vec![column],
        },
    };
    let mut book = Workbook::empty(doc).candidate(&op, DocumentVersion::new(1)?)?;
    db.create(
        identity,
        CommittedEvent {
            document_id: doc,
            operation_id: OperationId::from_bytes([7; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: book.version(),
            operation: op,
            state_hash: book.hash(),
        },
    )
    .await?;
    let address = CellAddress { sheet, row, column };
    for sequence in 2u128..=103 {
        let value = CellValue::Number(sequence as f64);
        let op = Operation::CellWrite {
            address,
            previous: book.cell_content(address)?.cloned(),
            content: Some(value.clone().into()),
            kind: Some(CellWriteKind::SetValue),
            undoes: None,
            undoes_version: None,
        };
        let next = DocumentVersion::new(sequence as u64)?;
        let candidate = book.candidate(&op, next)?;
        let request = CommitRequest {
            identity,
            expected_head: book.version(),
            expected_access: db.access(identity, doc).await?,
            inverse_projection: Some(Arc::new(book.clone())),
            command: Command {
                document_id: doc,
                operation_id: OperationId::from_bytes(sequence.to_be_bytes())?,
                actor_id: identity.actor_id,
                base_version: book.version(),
                client_sequence: sequence as u64,
                operation: CommandPayload::SetCellValue { address, value },
            },
            event: CommittedEvent {
                document_id: doc,
                operation_id: OperationId::from_bytes(sequence.to_be_bytes())?,
                principal: EventPrincipal::User(identity.actor_id),
                version: next,
                operation: op,
                state_hash: candidate.hash(),
            },
        };
        assert!(matches!(
            db.commit(&request).await?,
            CommitOutcome::Committed(_)
        ));
        book = candidate;
    }
    let page = db.activity_page(identity, doc, 102, None).await?;
    assert_eq!(page.through_version, 102);
    assert_eq!(page.entries.len(), 100);
    assert_eq!(page.entries[0].event.version.get(), 102);
    assert_eq!(page.entries[99].event.version.get(), 3);
    assert_eq!(page.next_before, Some(3));
    assert!(page.entries.iter().all(|e| e.created_at > 0));
    let older = db
        .activity_page(identity, doc, 102, page.next_before)
        .await?;
    assert_eq!(older.entries.len(), 2);
    assert_eq!(older.entries[1].event.version.get(), 1);
    assert_eq!(older.next_before, None);
    assert!(matches!(
        db.activity_page(identity, doc, 102, Some(104)).await,
        Err(JournalError::Conflict)
    ));
    sqlx::query(
        "UPDATE document_permissions SET role='editor' WHERE document_id=$1 AND user_id=$2",
    )
    .bind(Uuid::from_bytes(*doc.as_bytes()))
    .bind(Uuid::from_bytes(*identity.actor_id.as_bytes()))
    .execute(&db.pool)
    .await?;
    assert!(matches!(
        db.activity_page(identity, doc, 102, None).await,
        Err(JournalError::Unauthorized)
    ));
    assert!(matches!(
        db.lease_owner_delivery_access(identity, doc).await,
        Err(JournalError::Unauthorized)
    ));
    sqlx::query("UPDATE document_permissions SET role='owner' WHERE document_id=$1 AND user_id=$2")
        .bind(Uuid::from_bytes(*doc.as_bytes()))
        .bind(Uuid::from_bytes(*identity.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    let (head,count):(i64,i64)=sqlx::query_as("SELECT head_version,(SELECT count(*) FROM document_operations WHERE document_id=$1) FROM documents WHERE document_id=$1").bind(Uuid::from_bytes(*doc.as_bytes())).fetch_one(&db.pool).await?;
    assert_eq!((head, count), (103, 103));
    // This privileged test transaction simulates disk/DB corruption. Production
    // journal writes remain protected by the append-only trigger.
    let mut tamper = db.pool.begin().await?;
    sqlx::query("SET LOCAL session_replication_role='replica'")
        .execute(&mut *tamper)
        .await?;
    sqlx::query("UPDATE document_operations SET payload=set_byte(payload,0,get_byte(payload,0)#1) WHERE document_id=$1 AND committed_version=102").bind(Uuid::from_bytes(*doc.as_bytes())).execute(&mut *tamper).await?;
    tamper.commit().await?;
    assert!(matches!(
        db.activity_page(identity, doc, 102, None).await,
        Err(JournalError::Corrupt)
    ));
    Ok(())
}
