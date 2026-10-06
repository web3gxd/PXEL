use super::*;
use spreadsheet_application::journal::OperationJournal;
use spreadsheet_domain::workbook::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[sqlx::test(migrations = false)]
async fn csv_format_keeps_legacy_permissions_and_authenticates_codec_version(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, viewer, doc) = fixture(pool).await?;
    assert!(db.can_export_csv(owner, doc, None).await?);
    assert!(!db.can_export_csv(viewer, doc, None).await?);
    let rules = DisclosureRules {
        allow_copy: true,
        export_formats: vec![DisclosureFormat::Xlsx, DisclosureFormat::Pxel],
        allow_participant_export: false,
        require_anonymization: false,
    };
    let scope = Uuid::from_bytes(*doc.as_bytes());
    let bytes = Zeroizing::new(serde_json::to_vec(&rules)?);
    let old = db
        .seal_record_guarded(doc, 1, "disclosure-policy", scope, 1, &bytes, 4096)
        .await?;
    sqlx::query(
        "INSERT INTO disclosure_policies(scope_id,document_id,revision,payload) VALUES($1,$1,1,$2)",
    )
    .bind(scope)
    .bind(old)
    .execute(&db.pool)
    .await?;
    let legacy = db.disclosure_policy(owner, doc, None).await?;
    assert_eq!(legacy.schema_version, 2);
    assert_eq!(legacy.rules, rules);
    assert!(!db.can_export_csv(owner, doc, None).await?);
    let denied = db
        .authorize_disclosure(owner, doc, None, DisclosureAction::ExportCsv, None)
        .await?;
    assert_eq!(denied.reason, DisclosureReason::FormatDenied);
    let csv = DisclosureRules {
        export_formats: vec![DisclosureFormat::Csv],
        allow_participant_export: true,
        ..rules
    };
    let saved = db
        .set_disclosure_policy(owner, doc, None, 1, csv.clone())
        .await?;
    assert_eq!(saved.revision, 2);
    assert!(db.can_export_csv(owner, doc, None).await?);
    assert!(db.can_export_csv(viewer, doc, None).await?);
    assert!(
        !db.authorize_disclosure(owner, doc, None, DisclosureAction::ExportXlsx, None)
            .await?
            .allowed
    );
    assert_eq!(
        db.authorize_disclosure(
            owner,
            doc,
            None,
            DisclosureAction::ExportCsv,
            Some(denied.stamp)
        )
        .await?
        .reason,
        DisclosureReason::PolicyChanged
    );
    assert!(matches!(
        db.set_disclosure_policy(
            owner,
            doc,
            None,
            2,
            DisclosureRules {
                export_formats: vec![DisclosureFormat::Csv, DisclosureFormat::Csv],
                ..csv
            }
        )
        .await,
        Err(DisclosureError::InvalidInput)
    ));
    let codec: i32 =
        sqlx::query_scalar("SELECT codec_version FROM disclosure_policies WHERE scope_id=$1")
            .bind(scope)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(codec, 2);
    sqlx::query("UPDATE disclosure_policies SET codec_version=1 WHERE scope_id=$1")
        .bind(scope)
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        db.disclosure_policy(owner, doc, None).await,
        Err(DisclosureError::Corrupt)
    ));
    sqlx::query("UPDATE disclosure_policies SET codec_version=2 WHERE scope_id=$1")
        .bind(scope)
        .execute(&db.pool)
        .await?;
    assert!(
        db.disclosure_policy(owner, doc, None)
            .await?
            .rules
            .export_formats
            .contains(&DisclosureFormat::Csv)
    );
    let audit = db.disclosure_audit(owner, doc, None).await?;
    assert_eq!(audit.schema_version, 2);
    assert!(audit.events.iter().any(|e| e.format == "csv" && !e.allowed));
    sqlx::query("DROP TABLE disclosure_audit_events")
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        db.authorize_disclosure(owner, doc, None, DisclosureAction::ExportCsv, None)
            .await,
        Err(DisclosureError::Unavailable)
    ));
    Ok(())
}
async fn fixture(
    pool: sqlx::PgPool,
) -> Result<
    (
        PgDatabase,
        AuthenticatedActor,
        AuthenticatedActor,
        DocumentId,
    ),
    Box<dyn std::error::Error>,
> {
    let db = PgDatabase::for_test(pool)?;
    db.migrate().await?;
    let owner = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    db.issue_development_session([9; 32], owner.actor_id, owner.workspace_id)
        .await?;
    let viewer = AuthenticatedActor {
        actor_id: ActorId::from_bytes([4; 16])?,
        ..owner
    };
    sqlx::query("INSERT INTO users(user_id) VALUES($1)")
        .bind(Uuid::from_bytes(*viewer.actor_id.as_bytes()))
        .execute(&db.pool)
        .await?;
    let document = DocumentId::from_bytes([3; 16])?;
    let operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([20; 16])?,
            name: "Private marker".into(),
            rows: vec![RowId::from_bytes([30; 16])?],
            columns: vec![ColumnId::from_bytes([40; 16])?],
        },
    };
    let book = Workbook::empty(document).candidate(&operation, DocumentVersion::new(1)?)?;
    db.create(
        owner,
        CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([50; 16])?,
            principal: EventPrincipal::User(owner.actor_id),
            version: book.version(),
            operation,
            state_hash: book.hash(),
        },
    )
    .await?;
    db.change_permission(
        owner,
        document,
        viewer.actor_id,
        AuthorizationChangeId::from_bytes([6; 16])?,
        Some(crate::PermissionRole::Viewer),
    )
    .await?;
    Ok((db, owner, viewer, document))
}

#[sqlx::test(migrations = false)]
async fn defaults_intersection_denials_and_policy_race_are_durable(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, viewer, doc) = fixture(pool).await?;
    let export = DisclosureAction::ExportXlsx;
    let start = db
        .authorize_disclosure(owner, doc, None, export, None)
        .await?;
    assert!(start.allowed);
    assert!(
        !db.authorize_disclosure(viewer, doc, None, export, None)
            .await?
            .allowed
    );
    assert!(
        db.authorize_disclosure(viewer, doc, None, DisclosureAction::Copy, None)
            .await?
            .allowed
    );
    assert!(
        !db.authorize_disclosure(viewer, doc, None, DisclosureAction::Cut, None)
            .await?
            .allowed
    );
    let policy = db.disclosure_policy(owner, doc, None).await?;
    let rules = DisclosureRules {
        allow_participant_export: true,
        ..policy.rules
    };
    let updated = db
        .set_disclosure_policy(owner, doc, None, 0, rules.clone())
        .await?;
    assert_eq!(updated.revision, 1);
    assert!(matches!(
        db.set_disclosure_policy(owner, doc, None, 0, rules.clone())
            .await,
        Err(DisclosureError::Conflict)
    ));
    assert!(matches!(
        db.set_disclosure_policy(viewer, doc, None, 1, rules.clone())
            .await,
        Err(DisclosureError::Unauthorized)
    ));
    assert!(
        db.authorize_disclosure(viewer, doc, None, export, None)
            .await?
            .allowed
    );
    let stale = db
        .authorize_disclosure(owner, doc, None, export, Some(start.stamp))
        .await?;
    assert!(!stale.allowed);
    assert_eq!(stale.reason, DisclosureReason::PolicyChanged);
    db.set_disclosure_policy(
        owner,
        doc,
        None,
        1,
        DisclosureRules {
            allow_copy: false,
            require_anonymization: true,
            ..rules
        },
    )
    .await?;
    assert!(
        !db.authorize_disclosure(owner, doc, None, DisclosureAction::Copy, None)
            .await?
            .allowed
    );
    assert_eq!(
        db.authorize_disclosure(owner, doc, None, export, None)
            .await?
            .reason,
        DisclosureReason::AnonymizationRequired
    );
    let page = db.disclosure_audit(owner, doc, None).await?;
    assert_eq!(page.events.len(), 8);
    assert!(page.events.iter().any(|e| !e.allowed));
    assert!(!serde_json::to_string(&page)?.contains("Private marker"));
    assert!(matches!(
        db.disclosure_audit(viewer, doc, None).await,
        Err(DisclosureError::Unauthorized)
    ));
    assert!(
        sqlx::query("DELETE FROM disclosure_audit_events")
            .execute(&db.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE disclosure_audit_events SET allowed=true")
            .execute(&db.pool)
            .await
            .is_err()
    );
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT payload FROM disclosure_policies WHERE scope_id=$1")
            .bind(Uuid::from_bytes(*doc.as_bytes()))
            .fetch_one(&db.pool)
            .await?;
    assert!(!String::from_utf8_lossy(&stored).contains("allow_copy"));
    sqlx::query("DROP TABLE disclosure_audit_events")
        .execute(&db.pool)
        .await?;
    assert!(matches!(
        db.authorize_disclosure(owner, doc, None, DisclosureAction::Copy, None)
            .await,
        Err(DisclosureError::Unavailable)
    ));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn guest_policy_intersection_revokes_sessions_without_granting_edit(
    pool: sqlx::PgPool,
) -> TestResult {
    use crate::{
        GuestSessionSeed, ShareAttemptOutcome, ShareLifetime, ShareRole, ShareSecretKind,
        ShareSecretPhc,
    };
    use sha2::{Digest, Sha256};
    let (db, owner, _, doc) = fixture(pool).await?;
    let phc=ShareSecretPhc::new(ShareSecretKind::Pin,Zeroizing::new("$argon2id$v=19$m=65536,t=3,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()))?;
    let created = db
        .create_share(
            owner,
            doc,
            Uuid::now_v7(),
            ShareRole::View,
            ShareLifetime::Hour,
            phc,
            [7; 32],
        )
        .await?;
    let link = created.link.link_id;
    let hash: [u8; 32] =
        Sha256::digest(created.token.as_ref().ok_or("token missing")?.as_slice()).into();
    let db_ref = &db;
    let login = |value: u8| async move {
        let seed = GuestSessionSeed {
            session_id: Uuid::now_v7(),
            actor_id: ActorId::from_bytes([value; 16])?,
            token_hash: [value; 32],
        };
        match db_ref
            .begin_share_attempt(link, hash)
            .await?
            .complete(true, Some(seed))
            .await?
        {
            ShareAttemptOutcome::Accepted(session) => Ok::<_, Box<dyn std::error::Error>>(session),
            _ => Err("login refused".into()),
        }
    };
    let first = login(70).await?;
    assert!(!db.can_export_csv(first.identity, doc, Some(link)).await?);
    assert!(
        db.authorize_disclosure(
            first.identity,
            doc,
            Some(link),
            DisclosureAction::Copy,
            None
        )
        .await?
        .allowed
    );
    assert!(
        !db.authorize_disclosure(
            first.identity,
            doc,
            Some(link),
            DisclosureAction::ExportXlsx,
            None
        )
        .await?
        .allowed
    );
    let book = db.disclosure_policy(owner, doc, None).await?;
    db.set_disclosure_policy(
        owner,
        doc,
        None,
        0,
        DisclosureRules {
            allow_participant_export: true,
            ..book.rules
        },
    )
    .await?;
    assert!(db.guest_session(link, [70; 32]).await?.is_none());
    let second = login(71).await?;
    assert!(!db.can_export_csv(second.identity, doc, Some(link)).await?);
    assert!(
        !db.authorize_disclosure(
            second.identity,
            doc,
            Some(link),
            DisclosureAction::ExportXlsx,
            None
        )
        .await?
        .allowed
    );
    let policy = db.disclosure_policy(owner, doc, Some(link)).await?;
    db.set_disclosure_policy(
        owner,
        doc,
        Some(link),
        0,
        DisclosureRules {
            export_formats: vec![DisclosureFormat::Xlsx, DisclosureFormat::Csv],
            allow_participant_export: true,
            ..policy.rules
        },
    )
    .await?;
    assert!(db.guest_session(link, [71; 32]).await?.is_none());
    let third = login(72).await?;
    assert!(db.can_export_csv(third.identity, doc, Some(link)).await?);
    assert!(
        db.authorize_disclosure(
            third.identity,
            doc,
            Some(link),
            DisclosureAction::ExportXlsx,
            None
        )
        .await?
        .allowed
    );
    assert!(
        !db.authorize_disclosure(third.identity, doc, Some(link), DisclosureAction::Cut, None)
            .await?
            .allowed
    );
    assert!(
        !db.authorize_disclosure(third.identity, doc, None, DisclosureAction::Copy, None)
            .await?
            .allowed
    );
    let mut rules = db.disclosure_policy(owner, doc, Some(link)).await?.rules;
    rules.allow_copy = false;
    db.set_disclosure_policy(owner, doc, Some(link), 1, rules)
        .await?;
    let fourth = login(73).await?;
    assert!(
        !db.authorize_disclosure(
            fourth.identity,
            doc,
            Some(link),
            DisclosureAction::Copy,
            None
        )
        .await?
        .allowed
    );
    assert!(
        db.authorize_disclosure(
            fourth.identity,
            doc,
            Some(link),
            DisclosureAction::ExportXlsx,
            None
        )
        .await?
        .allowed
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn policy_change_waits_for_durable_delivery_lease(pool: sqlx::PgPool) -> TestResult {
    let (db, owner, _, doc) = fixture(pool).await?;
    let policy = db.disclosure_policy(owner, doc, None).await?;
    let initial = db
        .authorize_disclosure(owner, doc, None, DisclosureAction::ExportXlsx, None)
        .await?;
    let lease = db.lease_delivery_access(owner, doc).await?;
    let mutation = db.set_disclosure_policy(
        owner,
        doc,
        None,
        0,
        DisclosureRules {
            allow_copy: false,
            ..policy.rules
        },
    );
    tokio::pin!(mutation);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut mutation)
            .await
            .is_err()
    );
    lease.release().await?;
    mutation.await?;
    assert!(
        !db.authorize_disclosure(
            owner,
            doc,
            None,
            DisclosureAction::ExportXlsx,
            Some(initial.stamp)
        )
        .await?
        .allowed
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn preparation_failure_is_durable_and_preserves_policy_denials(
    pool: sqlx::PgPool,
) -> TestResult {
    let (db, owner, _, doc) = fixture(pool).await?;
    let action = DisclosureAction::ExportCsv;
    let admission = db
        .authorize_disclosure(owner, doc, None, action, None)
        .await?;
    assert!(admission.allowed);
    db.record_export_preparation_failure(owner, doc, None, action, admission.stamp)
        .await?;
    let audit = db.disclosure_audit(owner, doc, None).await?;
    assert!(
        audit
            .events
            .iter()
            .any(|event| !event.allowed && event.reason == "preparation_failed")
    );
    let policy = db.disclosure_policy(owner, doc, None).await?;
    db.set_disclosure_policy(
        owner,
        doc,
        None,
        policy.revision,
        DisclosureRules {
            export_formats: vec![],
            ..policy.rules
        },
    )
    .await?;
    db.record_export_preparation_failure(owner, doc, None, action, admission.stamp)
        .await?;
    let audit = db.disclosure_audit(owner, doc, None).await?;
    assert!(
        audit
            .events
            .iter()
            .any(|event| !event.allowed && event.reason == "policy_changed")
    );
    Ok(())
}
