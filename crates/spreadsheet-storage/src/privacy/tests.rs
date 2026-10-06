use super::*;
use spreadsheet_application::journal::OperationJournal;
use spreadsheet_domain::workbook::*;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[sqlx::test(migrations = false)]
async fn privacy_cas_encryption_audit_and_guest_invalidation_are_durable(
    pool: sqlx::PgPool,
) -> TestResult {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let owner = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    db.issue_development_session([9; 32], owner.actor_id, owner.workspace_id)
        .await?;
    let doc = DocumentId::from_bytes([3; 16])?;
    let operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([4; 16])?,
            name: "Privacy".into(),
            rows: vec![RowId::from_bytes([5; 16])?],
            columns: vec![ColumnId::from_bytes([6; 16])?],
        },
    };
    let book = Workbook::empty(doc).candidate(&operation, DocumentVersion::new(1)?)?;
    db.create(
        owner,
        CommittedEvent {
            document_id: doc,
            operation_id: OperationId::from_bytes([7; 16])?,
            principal: EventPrincipal::User(owner.actor_id),
            version: book.version(),
            operation,
            state_hash: book.hash(),
        },
    )
    .await?;
    assert!(!db.privacy_policy(owner, doc, None).await?.rules.enabled);
    let (_, _, _, old_stamp) = db.disclosure_capabilities(owner, doc, None).await?;
    let rules = PrivacyRules {
        enabled: true,
        types: vec!["email".into(), "inn".into()],
        exceptions: vec!["ab".repeat(32)],
    };
    let saved = db
        .set_privacy_policy(owner, doc, 1, 0, rules.clone())
        .await?;
    assert_eq!(saved.revision, 1);
    assert_eq!(db.privacy_policy(owner, doc, None).await?.rules, rules);
    assert!(matches!(
        db.set_privacy_policy(owner, doc, 1, 0, rules.clone()).await,
        Err(DisclosureError::Conflict)
    ));
    assert!(matches!(
        db.set_privacy_policy(owner, doc, 2, 1, rules.clone()).await,
        Err(DisclosureError::Conflict)
    ));
    let stale = db
        .authorize_disclosure(
            owner,
            doc,
            None,
            crate::DisclosureAction::Copy,
            Some(old_stamp),
        )
        .await?;
    assert!(!stale.allowed);
    let receipt = db
        .authorize_disclosure(
            owner,
            doc,
            None,
            crate::DisclosureAction::CopyAnonymized,
            None,
        )
        .await?;
    assert!(receipt.allowed && receipt.anonymized);
    db.record_disclosure_failure(
        owner,
        doc,
        None,
        crate::DisclosureAction::CopyAnonymized,
        receipt.stamp,
    )
    .await?;
    let payload: Vec<u8> =
        sqlx::query_scalar("SELECT payload FROM document_privacy_policies WHERE document_id=$1")
            .bind(id(doc))
            .fetch_one(&db.pool)
            .await?;
    assert!(!payload.windows(5).any(|w| w == b"email"));
    assert!(!payload.windows(64).any(|w| w == "ab".repeat(32).as_bytes()));
    let secret = || {
        crate::ShareSecretPhc::new(crate::ShareSecretKind::Pin,zeroize::Zeroizing::new("$argon2id$v=19$m=65536,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g".into()))
    };
    let request = Uuid::now_v7();
    let link = db
        .create_share_presented(
            owner,
            doc,
            request,
            crate::ShareRole::Comment,
            crate::ShareLifetime::Hour,
            secret()?,
            [1; 32],
            true,
        )
        .await?;
    assert!(link.link.anonymized_at_creation);
    assert!(
        db.disclosure_policy(owner, doc, Some(link.link.link_id))
            .await?
            .rules
            .require_anonymization
    );
    assert!(
        !db.create_share_presented(
            owner,
            doc,
            request,
            crate::ShareRole::Comment,
            crate::ShareLifetime::Hour,
            secret()?,
            [1; 32],
            true
        )
        .await?
        .newly_committed
    );
    assert!(matches!(
        db.create_share_presented(
            owner,
            doc,
            request,
            crate::ShareRole::Comment,
            crate::ShareLifetime::Hour,
            secret()?,
            [1; 32],
            false
        )
        .await,
        Err(crate::ShareError::IdempotencyConflict)
    ));
    assert!(
        db.create_share_presented(
            owner,
            doc,
            Uuid::now_v7(),
            crate::ShareRole::Edit,
            crate::ShareLifetime::Hour,
            secret()?,
            [1; 32],
            true
        )
        .await
        .is_err()
    );
    let token = link.token.ok_or("missing token")?;
    use sha2::{Digest, Sha256};
    let attempt = db
        .begin_share_attempt(link.link.link_id, Sha256::digest(token.as_slice()).into())
        .await?;
    let guest = match attempt
        .complete(
            true,
            Some(crate::GuestSessionSeed {
                session_id: Uuid::now_v7(),
                actor_id: ActorId::from_bytes([20; 16])?,
                token_hash: [21; 32],
            }),
        )
        .await?
    {
        crate::ShareAttemptOutcome::Accepted(v) => v,
        _ => return Err("admission failed".into()),
    };
    assert!(
        db.requires_anonymization(guest.identity, doc, Some(link.link.link_id))
            .await?
    );
    let mut tx = db.begin_guarded().await?;
    assert!(matches!(
        db.deny_anonymized_guest_write(&mut tx, guest.identity, doc)
            .await,
        Err(spreadsheet_application::journal::JournalError::Unauthorized)
    ));
    drop(tx);
    db.set_privacy_policy(owner, doc, 1, 1, rules).await?;
    assert!(db.access(guest.identity, doc).await.is_err());
    assert_eq!(db.access(owner, doc).await?.head.get(), 1);
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_operations WHERE document_id=$1")
            .bind(id(doc))
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(events, 1);
    let audit = db.disclosure_audit(owner, doc, None).await?;
    assert_eq!(audit.events.len(), 3);
    assert!(
        audit
            .events
            .iter()
            .any(|e| !e.allowed && e.reason == "privacy_analysis_failed")
    );
    Ok(())
}
